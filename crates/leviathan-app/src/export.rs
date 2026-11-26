use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use leviathan_core::{EngineConfig, LaunchTarget, SongPlayer, NUM_LANES};
use leviathan_dsp::{echo, CaptureSamples, LanePatch, StereoDelay, SynthRack};

use crate::project::Project;
use crate::shader_bus::{ReturnFrame, CV_CHANNELS};
use crate::wav::{StereoWav, WavEncoding, WavWriter};

pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
const BENDER_STEM_NAME: &str = "bend.wav";
const SHADER_CV_NAME: &str = "shader-bytebus.wav";

fn stem_filename(lane: usize, patch: &LanePatch) -> String {
    format!("{lane}-{}.wav", patch.machine.slug())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportSource {
    Pattern,
    Chain,
}

impl ExportSource {
    pub fn target(self, selected_pattern: usize) -> LaunchTarget {
        match self {
            Self::Pattern => LaunchTarget::Pattern(selected_pattern),
            Self::Chain => LaunchTarget::Chain,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ExportSettings {
    pub source: ExportSource,
    pub encoding: WavEncoding,
    pub sample_rate: u32,
    pub loops: u32,
    pub tail_seconds: f64,
    pub stems: bool,
    pub shader_cv: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            source: ExportSource::Pattern,
            encoding: WavEncoding::Pcm16,
            sample_rate: 48_000,
            loops: 1,
            tail_seconds: 0.5,
            stems: false,
            shader_cv: false,
        }
    }
}

impl ExportSettings {
    pub fn uses_folder(self) -> bool {
        self.stems || self.shader_cv
    }

    pub fn max_frames(self) -> u32 {
        let master = self.encoding.max_frames();
        if self.shader_cv {
            master.min(WavEncoding::Float32.max_frames_for(CV_CHANNELS as u16))
        } else {
            master
        }
    }
}

pub struct ExportRequest {
    pub settings: ExportSettings,
    pub destination: PathBuf,
}

#[derive(Clone, Copy)]
struct RenderPlan {
    settings: ExportSettings,
    target: LaunchTarget,
    music_frames: u32,
    frames: u32,
}

impl RenderPlan {
    fn new(project: &Project, settings: ExportSettings) -> io::Result<Self> {
        if !SAMPLE_RATES.contains(&settings.sample_rate)
            || settings.loops == 0
            || !settings.tail_seconds.is_finite()
            || settings.tail_seconds < 0.0
            || !(40.0..=240.0).contains(&project.bpm)
        {
            return Err(invalid("Invalid WAV export settings"));
        }
        let target = settings.source.target(project.selected_pattern);
        let duration = match target {
            LaunchTarget::Pattern(index) => {
                let pattern = project
                    .patterns
                    .get(index)
                    .ok_or_else(|| invalid("Select a pattern to export"))?;
                pattern.duration_beats()
            }
            LaunchTarget::Chain => {
                let entries = project.chain.as_slice();
                if entries.is_empty() {
                    return Err(invalid("Add a pattern to the chain before exporting it"));
                }
                let mut duration = 0.0;
                for &index in entries {
                    let pattern = project
                        .patterns
                        .get(usize::from(index))
                        .ok_or_else(|| invalid("Pattern chain contains a missing pattern"))?;
                    duration += pattern.duration_beats();
                }
                duration
            }
        };
        if settings.shader_cv && project.instruments.shader_bus.take_for(target).is_none() {
            return Err(invalid("Select the pattern or chain of the recorded shader take"));
        }

        let beats = duration * f64::from(settings.loops);
        let rate = f64::from(settings.sample_rate);
        let estimate = (beats * (60.0 * rate) / project.bpm).ceil();
        let tail = (settings.tail_seconds * rate).round();
        let max_frames = settings.max_frames();
        if estimate + tail > f64::from(max_frames) {
            return Err(invalid("Export exceeds RIFF/WAV size"));
        }
        let mut music_frames = estimate as u32;
        // Match the live sample clock at integer boundaries, including floating-point rounding.
        let beat_at = |sample: u32| f64::from(sample) * project.bpm / (60.0 * rate);
        if music_frames > 0 && beat_at(music_frames - 1) >= beats {
            music_frames -= 1;
        }
        if beat_at(music_frames) < beats {
            music_frames += 1;
        }
        let frames = music_frames + tail as u32;
        if frames > max_frames {
            return Err(invalid("Export exceeds RIFF/WAV size"));
        }
        Ok(Self {
            settings,
            target,
            music_frames,
            frames,
        })
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub struct ExportJob {
    worker: JoinHandle<io::Result<PathBuf>>,
    written: Arc<AtomicU32>,
    total_frames: u32,
}

impl ExportJob {
    pub fn start(project: Project, fill: bool, request: ExportRequest) -> io::Result<Self> {
        let plan = RenderPlan::new(&project, request.settings)?;
        let written = Arc::new(AtomicU32::new(0));
        let progress = Arc::clone(&written);
        let worker = thread::Builder::new()
            .name("leviathan-wav-export".into())
            .spawn(move || {
                write_outputs(&project, fill, plan, &request.destination, &progress)?;
                Ok(request.destination)
            })?;
        Ok(Self {
            worker,
            written,
            total_frames: plan.frames,
        })
    }

    pub fn progress(&self) -> f32 {
        self.written.load(Ordering::Relaxed) as f32 / self.total_frames as f32
    }

    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    pub fn finish(self) -> io::Result<PathBuf> {
        self.worker
            .join()
            .map_err(|_| io::Error::other("WAV export worker stopped unexpectedly"))?
    }
}

fn render(
    project: &Project,
    fill: bool,
    plan: RenderPlan,
    progress: &AtomicU32,
    mut write: impl FnMut([f32; 2], &[[f32; 2]; NUM_LANES], [f32; 2], ReturnFrame) -> io::Result<()>,
) -> io::Result<()> {
    let mut player = SongPlayer::new(EngineConfig {
        bpm: project.bpm,
        sample_rate: f64::from(plan.settings.sample_rate),
        ..EngineConfig::default()
    });
    let gates = project.instruments.note_gates;
    player.evaluator.set_retrigger_held_notes(gates.enabled);
    player.set_chain(project.chain);
    player.launch(plan.target, project.patterns.len());
    player.evaluator.fill_active = fill;
    player.evaluator.playing = true;

    let mut rack = SynthRack::new(plan.settings.sample_rate as f32);
    rack.sample_bank = project.sample_bank();
    rack.set_lane_audible_mask(project.lane_mix.audible_mask());
    rack.set_lane_channels(project.lane_mix.dsp_channels());
    rack.set_lane_patches(project.lane_patches);
    rack.set_voice_calibration(project.voice_calibration);
    rack.set_note_release_lanes(gates.release_lanes());
    rack.set_bbus_grit_lanes(project.instruments.bbus.grit_lanes());
    rack.set_otis_warm_tables(project.instruments.otis.warm_tables);
    rack.set_alias_grit_lanes(project.instruments.oversample.grit_lanes());
    rack.set_routing(project.instruments.routing);
    rack.reset_voices();
    rack.set_bender_params(project.bender_params);
    rack.set_echo_params(project.instruments.echo);
    rack.set_tempo(project.bpm);
    rack.set_kit_controls(project.instruments.kit.params, project.instruments.kit.lanes);
    rack.set_delay_mode(project.delay_mode);
    rack.reset_master(project.master_send);
    let mut samples = CaptureSamples {
        kit: Some(&project.instruments.kit),
        current: project
            .instruments
            .take
            .as_deref()
            .map(crate::capture::Take::sample),
        ..CaptureSamples::default()
    };
    let mut shader = project.instruments.shader_bus.replay(plan.target);
    let ppqn = player.evaluator.config.ppqn;

    let mut echo_storage = if project.instruments.echo.echo_mix > 0.0 {
        vec![[0.0; 2]; echo::capacity(plan.settings.sample_rate as f32)]
    } else {
        Vec::new()
    };
    let mut stems = [[0.0; 2]; NUM_LANES];
    let mut stem_delays: Option<Box<[StereoDelay; NUM_LANES]>> = plan.settings.stems.then(|| {
        std::iter::repeat_with(|| {
            let mut delay = StereoDelay::new(plan.settings.sample_rate as f32);
            delay.set_mode(project.delay_mode);
            delay
        })
            .take(NUM_LANES)
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .ok()
            .unwrap()
    });
    for frame in 0..plan.frames {
        if frame == plan.music_frames {
            player.evaluator.playing = false;
        }
        let (cv, retrigger) = player.step_sample(&project.patterns);
        let return_frame = shader.step(&cv, ppqn);
        samples.ot2_frame = return_frame.address;
        if retrigger {
            rack.retrigger_gates();
        }
        let ((left, right), wet) = if let Some(delays) = &mut stem_delays {
            rack.render_session_with_echo_stems(&cv, &samples, &mut stems, delays, &mut echo_storage)
        } else {
            (rack.render_session_with_echo(&cv, &samples, None, &mut echo_storage).0, [0.0; 2])
        };
        write([left, right], &stems, wet, return_frame)?;
        if frame & 1023 == 1023 {
            progress.store(frame + 1, Ordering::Relaxed);
        }
    }
    progress.store(plan.frames, Ordering::Relaxed);
    Ok(())
}

fn write_outputs(
    project: &Project,
    fill: bool,
    plan: RenderPlan,
    destination: &Path,
    progress: &AtomicU32,
) -> io::Result<()> {
    if plan.settings.uses_folder() {
        require_new_folder(destination)?;
    }
    let stage = StagingDirectory::new(destination)?;
    let create_writer = |name: &str| {
        let file = File::create(stage.path.join(name))?;
        StereoWav::new(
            BufWriter::new(file),
            plan.settings.encoding,
            plan.settings.sample_rate,
            plan.frames,
        )
    };
    let mut master_writer = create_writer("master.wav")?;
    let mut lane_writers = Vec::new();
    let mut bender_writer = None;
    if plan.settings.stems {
        for (lane, patch) in project.lane_patches.iter().enumerate() {
            lane_writers.push(create_writer(&stem_filename(lane, patch))?);
        }
        if project.lane_mix.channels.iter().any(|channel| channel.0.fx_send > 0.0) {
            bender_writer = Some(create_writer(BENDER_STEM_NAME)?);
        }
    }
    let mut shader_cv_writer = if plan.settings.shader_cv {
        let file = File::create(stage.path.join(SHADER_CV_NAME))?;
        Some(WavWriter::<_, CV_CHANNELS>::new(
            BufWriter::new(file),
            WavEncoding::Float32,
            plan.settings.sample_rate,
            plan.frames,
        )?)
    } else {
        None
    };
    render(project, fill, plan, progress, |master, stems, wet, return_frame| {
        master_writer.write_frame(master)?;
        for (writer, &stem) in lane_writers.iter_mut().zip(stems) {
            writer.write_frame(stem)?;
        }
        if let Some(writer) = &mut bender_writer {
            writer.write_frame(wet)?;
        }
        if let Some(writer) = &mut shader_cv_writer {
            writer.write_frame(return_frame.cv_samples())?;
        }
        Ok(())
    })?;
    for writer in std::iter::once(master_writer).chain(lane_writers).chain(bender_writer) {
        writer.finish()?.get_ref().sync_all()?;
    }
    if let Some(writer) = shader_cv_writer {
        writer.finish()?.get_ref().sync_all()?;
    }
    if plan.settings.uses_folder() {
        stage.publish(destination)?;
    } else {
        fs::rename(stage.path.join("master.wav"), destination)?;
    }
    Ok(())
}

/// A private output directory for Android's provider-based export.
#[cfg(any(target_os = "android", test))]
pub(crate) struct MobileStage {
    path: PathBuf,
    bundle: bool,
}

#[cfg(any(target_os = "android", test))]
impl MobileStage {
    pub(crate) fn new(root: &Path, bundle: bool) -> io::Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{Duration, SystemTime};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(root)?;
        // Process death can strand a render. Only prune our private, old staging directories.
        for entry in fs::read_dir(root)?.flatten() {
            if entry.file_name().to_string_lossy().starts_with("export-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|time|
                    SystemTime::now().duration_since(time).is_ok_and(|age| age > Duration::from_secs(86_400)))
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("export-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, bundle }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn destination(&self) -> PathBuf {
        // For a bundle the renderer creates this as a directory, not a file.
        self.path.join("mix.wav")
    }

    pub(crate) fn root(&self) -> &Path {
        &self.path
    }

    pub(crate) fn is_bundle(&self) -> bool {
        self.bundle
    }
}

#[cfg(any(target_os = "android", test))]
impl Drop for MobileStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn require_new_folder(path: &Path) -> io::Result<()> {
    if path.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Choose a new folder for multiple WAV files",
        ));
    }
    Ok(())
}

// Staging beside the destination keeps the final rename on the same filesystem.
struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    fn new(destination: &Path) -> io::Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".leviathan-export-{}-{id}.tmp", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn publish(mut self, destination: &Path) -> io::Result<()> {
        require_new_folder(destination)?;
        fs::rename(&self.path, destination)?;
        self.path.clear();
        Ok(())
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane_mix::LaneMix;
    use crate::wav::decode_wav_bytes;
    use leviathan_core::{NoteCell, Pattern, PatternChain, PlaybackRate};
    use leviathan_dsp::{default_lane_patches, MachineKind};

    fn project() -> Project {
        let mut pattern = Pattern {
            length: 1,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        for lane in [0, 1, 5] {
            pattern.lanes[lane][0] = NoteCell {
                active: true,
                midi: 48 + lane as u8 * 3,
                velocity: 110,
                ..NoteCell::default()
            };
        }
        let second = Pattern {
            rate: PlaybackRate::FourX,
            ..pattern.clone()
        };
        let mut chain = PatternChain::default();
        chain.push(0);
        chain.push(1);
        Project {
            bpm: 120.0,
            master_send: 0.3,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_patches: default_lane_patches(),
            lane_mix: LaneMix::default(),
            patterns: vec![pattern, second],
            selected_pattern: 0,
            chain,
            samples: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        }
    }

    #[test]
    fn mobile_export_stages_the_shared_renderer_and_cleans_up() {
        let temp = StagingDirectory::new(&std::env::temp_dir().join("leviathan-mobile-test")).unwrap();
        let single = MobileStage::new(&temp.path, false).unwrap();
        let bundle = MobileStage::new(&temp.path, true).unwrap();
        assert_ne!(single.root(), bundle.root());
        assert!(!single.is_bundle());
        assert!(bundle.is_bundle());
        for stage in [&single, &bundle] {
            ExportJob::start(project(), false, ExportRequest {
                settings: ExportSettings { stems: stage.is_bundle(), ..Default::default() },
                destination: stage.destination(),
            }).unwrap().finish().unwrap();
        }
        assert_eq!(fs::read(single.destination()).unwrap(),
            fs::read(bundle.destination().join("master.wav")).unwrap());
        let root = single.root().to_owned();
        drop(single);
        assert!(!root.exists());
        assert!(bundle.destination().join("master.wav").is_file());
    }

    #[test]
    fn pattern_round_trips_in_both_formats_with_loop_and_tail_frames() {
        let project = project();
        for (encoding, sample_rate, expected_frames) in [
            (WavEncoding::Pcm16, 44_100, 1820),
            (WavEncoding::Float32, 48_000, 1980),
            (WavEncoding::Float32, 96_000, 3960),
        ] {
            let settings = ExportSettings {
                encoding,
                sample_rate,
                loops: 2,
                tail_seconds: 0.01,
                ..ExportSettings::default()
            };
            let plan = RenderPlan::new(&project, settings).unwrap();
            assert_eq!(plan.frames, expected_frames);
            let mut wav = StereoWav::new(Vec::new(), encoding, sample_rate, plan.frames).unwrap();
            let written = AtomicU32::new(0);
            render(&project, false, plan, &written, |master, _, _, _| {
                wav.write_frame(master)
            })
            .unwrap();
            let bytes = wav.finish().unwrap();
            assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8,
                bytes.len()
            );
            let decoded = decode_wav_bytes("bounce.wav", &bytes).unwrap();
            assert_eq!(decoded.original_frames, expected_frames as usize);
            assert_eq!(decoded.original_sr, sample_rate);
            assert!(decoded.peaks.iter().any(|&peak| peak > 0.01));
            assert_eq!(written.load(Ordering::Relaxed), expected_frames);
        }
    }

    #[test]
    fn shader_cv_bounce_matches_the_rack_bus_at_each_output_rate() {
        use crate::shader_bus::{Take, Target};
        use leviathan_dsp::{ByteBusReceiver, ByteBusSource};

        let temp = StagingDirectory::new(&std::env::temp_dir().join("leviathan-test")).unwrap();
        let mut snapshot = project();
        snapshot.lane_patches[0] = LanePatch::for_machine(MachineKind::OtisTwin);
        snapshot.patterns[0].rate = PlaybackRate::OneX;
        snapshot.patterns[0].length = 4;
        snapshot.patterns[1].rate = PlaybackRate::TwoX;
        snapshot.patterns[1].length = 3;
        let cv_settings = ExportSettings {
            source: ExportSource::Chain,
            shader_cv: true,
            ..Default::default()
        };
        assert!(RenderPlan::new(&snapshot, cv_settings).is_err());
        let patch = &mut snapshot.instruments.shader_bus;
        patch.route.lane = Some(0);
        patch.take = Some(Take {
            target: Target::Chain,
            lane: 0,
            words: vec![256, 0xa5, 0xa5, 0, 256, 0xff, 0x80],
        });
        let wrong_source = ExportSettings {
            source: ExportSource::Pattern,
            ..cv_settings
        };
        assert!(RenderPlan::new(&snapshot, wrong_source).is_err());

        for rate in SAMPLE_RATES {
            let settings = ExportSettings {
                sample_rate: rate,
                loops: 2,
                tail_seconds: 0.005,
                ..cv_settings
            };
            let plan = RenderPlan::new(&snapshot, settings).unwrap();
            let bundle = temp.path.join(format!("cv-{rate}"));
            write_outputs(&snapshot, false, plan, &bundle, &AtomicU32::new(0)).unwrap();
            let plain = temp.path.join(format!("master-{rate}.wav"));
            let plain_settings = ExportSettings { shader_cv: false, ..settings };
            let plain_plan = RenderPlan::new(&snapshot, plain_settings).unwrap();
            write_outputs(&snapshot, false, plain_plan, &plain, &AtomicU32::new(0)).unwrap();
            assert_eq!(
                fs::read(bundle.join("master.wav")).unwrap(),
                fs::read(plain).unwrap()
            );
            assert_eq!(fs::read_dir(&bundle).unwrap().count(), 2);

            let bytes = fs::read(bundle.join(SHADER_CV_NAME)).unwrap();
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            let read_u32 = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let read_u16 = |at| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
            assert_eq!(read_u32(4) as usize + 8, bytes.len());
            assert_eq!(read_u32(16), 40);
            assert_eq!(read_u16(20), 0xfffe);
            assert_eq!(read_u16(22), 10);
            assert_eq!(read_u32(24), rate);
            assert_eq!(read_u16(32), 40);
            assert_eq!(read_u16(34), 32);
            assert_eq!(read_u32(40), 0);
            assert_eq!(read_u32(44), 3);
            assert_eq!(&bytes[60..64], b"fact");
            assert_eq!(read_u32(68), plan.frames);
            assert_eq!(&bytes[72..76], b"data");
            assert_eq!(read_u32(76), plan.frames * 40);
            assert_eq!(bytes.len() - 80, plan.frames as usize * 40);

            // The loop wraps at 5.5 pulses. Keep the clock continuous and
            // carry the last word through the tail with the clock stopped.
            let mut receiver = ByteBusReceiver::default();
            let words = [
                None, Some(0xa5), Some(0xa5), Some(0), None, Some(0xff),
                Some(0x80), Some(0x80), Some(0x80), Some(0x80), Some(0x80),
            ];
            let mut pulse = 0;
            let mut expected = None;
            for (sample, frame) in bytes[80..].chunks_exact(40).enumerate() {
                let channels: [f32; 10] = std::array::from_fn(|index| {
                    f32::from_le_bytes(frame[index * 4..index * 4 + 4].try_into().unwrap())
                });
                let mut clock = false;
                let mut reset = false;
                if pulse < words.len() && sample == (pulse * rate as usize).div_ceil(8) {
                    expected = words[pulse];
                    clock = expected.is_some();
                    reset = pulse == 0 || pulse == 4;
                    pulse += 1;
                }
                let bits = expected.unwrap_or(0);
                for (bit, &value) in channels[..8].iter().enumerate() {
                    assert_eq!(value, f32::from((bits >> bit) & 1), "{rate} Hz bit {bit} at {sample}");
                }
                assert_eq!(channels[8], if clock { 1.0 } else { 0.0 }, "{rate} Hz clock at {sample}");
                assert_eq!(channels[9], if reset { 1.0 } else { 0.0 }, "{rate} Hz reset at {sample}");
                if reset {
                    let _ = receiver.step(ByteBusSource::Standalone, &[0.0; 9]);
                }
                let volts = std::array::from_fn(|index| channels[index] * 10.0);
                assert_eq!(receiver.step(ByteBusSource::Clocked, &volts), expected);
            }
            assert_eq!(pulse, words.len());
        }
    }

    #[test]
    fn chain_exports_aligned_stems_and_the_same_master_on_the_worker() {
        let temp = StagingDirectory::new(&std::env::temp_dir().join("leviathan-test")).unwrap();
        let stems = temp.path.join("stems");
        let mut snapshot = project();
        snapshot.lane_mix = LaneMix {
            mute_mask: 1 << 1,
            solo_mask: (1 << 0) | (1 << 1),
            ..LaneMix::default()
        };
        let settings = ExportSettings {
            source: ExportSource::Chain,
            stems: true,
            loops: 2,
            tail_seconds: 0.01,
            ..ExportSettings::default()
        };
        let request = ExportRequest {
            settings,
            destination: stems.clone(),
        };
        assert_eq!(
            ExportJob::start(snapshot, false, request)
                .unwrap()
                .finish()
                .unwrap(),
            stems
        );
        assert_eq!(fs::read_dir(&stems).unwrap().count(), NUM_LANES + 1);
    for (lane, name) in ["0-kick.wav", "1-fm4.wav", "2-sub.wav", "3-air.wav", "4-smpl.wav", "5-acid.wav"].iter().enumerate() {
            let bytes = fs::read(stems.join(name)).unwrap();
            let decoded = decode_wav_bytes(name, &bytes).unwrap();
            assert_eq!(decoded.original_frames, 4980);
            assert_eq!(decoded.original_sr, 48_000);
            if lane == 0 {
                assert!(decoded.peaks.iter().any(|&peak| peak > 0.01));
            } else {
                assert!(decoded.peaks.iter().all(|&peak| peak == 0.0));
            }
        }
        let master = fs::read(stems.join("master.wav")).unwrap();
        let mut snapshot = project();
        snapshot.lane_mix = LaneMix {
            mute_mask: 1 << 1,
            solo_mask: (1 << 0) | (1 << 1),
            ..LaneMix::default()
        };
        let file = temp.path.join("mix.wav");
        fs::write(&file, b"previous bounce").unwrap();
        let plan = RenderPlan::new(
            &snapshot,
            ExportSettings {
                stems: false,
                ..settings
            },
        )
        .unwrap();
        write_outputs(&snapshot, false, plan, &file, &AtomicU32::new(0)).unwrap();
        assert_eq!(fs::read(file).unwrap(), master);
        let plan = RenderPlan::new(&snapshot, settings).unwrap();
        assert_eq!(
            write_outputs(&snapshot, false, plan, &stems, &AtomicU32::new(0))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(stems.join("master.wav")).unwrap(), master);
    }
    #[test]
    fn stem_export_respects_dynamic_lane_patches_and_slug_names() {
        let temp =
            StagingDirectory::new(&std::env::temp_dir().join("leviathan-test-patches")).unwrap();
        let stems = temp.path.join("stems");
        let mut snapshot = project();
        snapshot.lane_patches[0].machine = MachineKind::FourOpFm;
        snapshot.lane_patches[0].params[1] = 0.05;
        snapshot.lane_patches[1].machine = MachineKind::FourOpFm;
        snapshot.lane_patches[1].params[1] = 0.95;
        snapshot.lane_patches[2].machine = MachineKind::ChordStab;

        let request = ExportRequest {
            settings: ExportSettings {
                stems: true,
                ..ExportSettings::default()
            },
            destination: stems.clone(),
        };

        ExportJob::start(snapshot, false, request)
            .unwrap()
            .finish()
            .unwrap();

        assert!(stems.join("0-fm4.wav").exists());
        assert!(stems.join("1-fm4.wav").exists());
        assert!(stems.join("2-chrd.wav").exists());
    }

    #[test]
    fn expansion_voices_export_audible_independent_stems() {
        let temp =
            StagingDirectory::new(&std::env::temp_dir().join("leviathan-test-expansion")).unwrap();
        let stems = temp.path.join("stems");
        let mut snapshot = project();
        let engines = [
            MachineKind::CzPhaseDist,
            MachineKind::PolivoksSub,
            MachineKind::VastFunSynth,
            MachineKind::ElectroPerc,
            MachineKind::WestCoastLpg,
            MachineKind::OtisTranswave,
        ];
        for (lane, engine) in engines.into_iter().enumerate() {
            snapshot.lane_patches[lane] = LanePatch::for_machine(engine);
            snapshot.patterns[0].lanes[lane][0] = NoteCell {
                active: true,
                midi: 53 + lane as u8 * 7,
                velocity: 127,
                gate_fraction: 0.95,
                ..NoteCell::default()
            };
        }
        let settings = ExportSettings {
            stems: true,
            tail_seconds: 0.01,
            ..ExportSettings::default()
        };
        ExportJob::start(
            snapshot,
            false,
            ExportRequest {
                settings,
                destination: stems.clone(),
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        for (lane, engine) in engines.into_iter().enumerate() {
            let name = stem_filename(lane, &LanePatch::for_machine(engine));
            let decoded = decode_wav_bytes(&name, &fs::read(stems.join(&name)).unwrap()).unwrap();
            assert_eq!(decoded.original_frames, 1230);
            assert!(decoded.peaks.iter().any(|&peak| peak > 0.015), "{name}");
        }
    }

    #[test]
    fn otis_twin_and_legacy_export_distinct_aligned_stems() {
        let temp =
            StagingDirectory::new(&std::env::temp_dir().join("leviathan-test-otis-twin")).unwrap();
        let stems = temp.path.join("stems");
        let mut snapshot = project();
        snapshot.patterns[0].lanes[5][0] = NoteCell::default();
        snapshot.lane_patches[0] = LanePatch::for_machine(MachineKind::OtisTranswave);
        snapshot.lane_patches[1] = LanePatch::for_machine(MachineKind::OtisTwin);
        snapshot.patterns[0].lanes[1][0] = snapshot.patterns[0].lanes[0][0];
        snapshot.lane_mix.channels[1] = snapshot.lane_mix.channels[0];

        let settings = ExportSettings {
            stems: true,
            tail_seconds: 0.04,
            ..ExportSettings::default()
        };
        let plan = RenderPlan::new(&snapshot, settings).unwrap();
        write_outputs(&snapshot, false, plan, &stems, &AtomicU32::new(0)).unwrap();
        let read_stem =
            |name| decode_wav_bytes(name, &fs::read(stems.join(name)).unwrap()).unwrap();
        let legacy = read_stem("0-otis.wav");
        let twin = read_stem("1-otis2.wav");
        assert_eq!(legacy.original_frames, plan.frames as usize);
        assert_eq!(twin.original_frames, plan.frames as usize);
        assert!(legacy.peaks.iter().any(|&peak| peak > 0.01));
        assert!(twin.peaks.iter().any(|&peak| peak > 0.01));
        assert_ne!(legacy.samples, twin.samples);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn wave7_default_echo_and_esp_preserve_the_legacy_pcm() {
        use crate::capture::Take;
        use leviathan_core::rack_routing::LaneInput;

        let mut fixture = project();
        let mut pattern = crate::starter_electro_pattern();
        pattern.length = 16;
        pattern.swing = 0.552;
        fixture.patterns = vec![pattern];
        fixture.chain = PatternChain::default();
        fixture.master_send = 0.0;
        fixture.instruments.routing.inputs[4] = LaneInput::Capture;
        fixture.instruments.take = Take::new(
            48_000,
            (0..2048)
                .map(|i| [
                    ((i * 751) % 21_817) as i16 - 10_000,
                    ((i * 491) % 18_983) as i16 - 9_000,
                ])
                .collect(),
        );
        fixture.lane_mix.channels[0].0.fx_send = 0.6;
        fixture.lane_mix.channels[4].0.fx_send = 0.5;
        fixture.bender_params.shift_norm = 0.613;

        // FNV-1a of stereo PCM rendered at 48 kHz by Wave 7 commit 1641232.
        let reference = [
            ("0-kick.wav", 0xb1a9_2c2f_f309_9701),
            ("1-fm4.wav", 0xb81c_f7e0_daa9_c9ec),
            ("2-sub.wav", 0x52d2_e653_68f7_58c0),
            ("3-air.wav", 0x35ca_2c08_1a1d_bdc5),
            ("4-smpl.wav", 0xdc5c_7d59_8211_96a9),
            ("5-acid.wav", 0xeb18_d9aa_8050_1a65),
            ("bend.wav", 0x690a_c3d4_b150_5acf),
            ("master.wav", 0x09f8_7680_2ce8_24e6),
        ];
        let temp = StagingDirectory::new(&std::env::temp_dir().join("leviathan-wave7")).unwrap();
        let stems = temp.path.join("stems");
        let settings = ExportSettings { stems: true, ..ExportSettings::default() };
        let plan = RenderPlan::new(&fixture, settings).unwrap();
        write_outputs(&fixture, false, plan, &stems, &AtomicU32::new(0)).unwrap();
        for (name, expected) in reference {
            let bytes = fs::read(stems.join(name)).unwrap();
            assert_eq!(&bytes[36..40], b"data");
            assert_eq!(bytes.len() - 44, plan.frames as usize * 4);
            let hash = bytes[44..].iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
            assert_eq!(hash, expected, "{name}");
        }
    }
}
