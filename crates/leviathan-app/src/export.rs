use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use leviathan_core::{EngineConfig, LaunchTarget, SongPlayer, NUM_LANES};
use leviathan_dsp::{LanePatch, StereoDelay, SynthRack};

use crate::project::Project;
use crate::wav::{StereoWav, WavEncoding};

pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
const BENDER_STEM_NAME: &str = "bend.wav";

fn stem_filename(lane: usize, patch: &LanePatch) -> String {
    format!("{lane}-{}.wav", patch.machine.slug())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportSource {
    Pattern,
    Chain,
}

#[derive(Clone, Copy)]
pub struct ExportSettings {
    pub source: ExportSource,
    pub encoding: WavEncoding,
    pub sample_rate: u32,
    pub loops: u32,
    pub tail_seconds: f64,
    pub stems: bool,
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
        let (target, duration) = match settings.source {
            ExportSource::Pattern => {
                let index = project.selected_pattern;
                let pattern = project
                    .patterns
                    .get(index)
                    .ok_or_else(|| invalid("Select a pattern to export"))?;
                (LaunchTarget::Pattern(index), pattern.duration_beats())
            }
            ExportSource::Chain => {
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
                (LaunchTarget::Chain, duration)
            }
        };

        let beats = duration * f64::from(settings.loops);
        let rate = f64::from(settings.sample_rate);
        let estimate = (beats * (60.0 * rate) / project.bpm).ceil();
        let tail = (settings.tail_seconds * rate).round();
        let max_frames = settings.encoding.max_frames();
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
    mut write: impl FnMut([f32; 2], &[[f32; 2]; NUM_LANES], [f32; 2]) -> io::Result<()>,
) -> io::Result<()> {
    let mut player = SongPlayer::new(EngineConfig {
        bpm: project.bpm,
        sample_rate: f64::from(plan.settings.sample_rate),
        ..EngineConfig::default()
    });
    player.set_chain(project.chain);
    player.launch(plan.target, project.patterns.len());
    player.evaluator.fill_active = fill;
    player.evaluator.playing = true;

    let mut rack = SynthRack::new(plan.settings.sample_rate as f32);
    rack.sample_bank = project.sample_bank();
    rack.set_lane_audible_mask(project.lane_mix.audible_mask());
    rack.set_lane_channels(project.lane_mix.dsp_channels());
    rack.set_lane_patches(project.lane_patches);
    rack.reset_voices();
    rack.set_bender_params(project.bender_params);
    rack.reset_master(project.master_send);

    let mut stems = [[0.0; 2]; NUM_LANES];
    let mut stem_delays: Option<Box<[StereoDelay; NUM_LANES]>> = plan.settings.stems.then(|| {
        std::iter::repeat_with(|| StereoDelay::new(plan.settings.sample_rate as f32))
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
        if retrigger {
            rack.retrigger_gates();
        }
        let ((left, right), wet) = if let Some(delays) = &mut stem_delays {
            rack.render_stereo_with_fx_stems(&cv, &mut stems, delays)
        } else {
            (rack.render_stereo(&cv), [0.0; 2])
        };
        write([left, right], &stems, wet)?;
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
    if plan.settings.stems {
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
    render(project, fill, plan, progress, |master, stems, wet| {
        master_writer.write_frame(master)?;
        for (writer, &stem) in lane_writers.iter_mut().zip(stems) {
            writer.write_frame(stem)?;
        }
        if let Some(writer) = &mut bender_writer {
            writer.write_frame(wet)?;
        }
        Ok(())
    })?;
    for writer in std::iter::once(master_writer).chain(lane_writers).chain(bender_writer) {
        writer.finish()?.get_ref().sync_all()?;
    }
    if plan.settings.stems {
        stage.publish(destination)?;
    } else {
        fs::rename(stage.path.join("master.wav"), destination)?;
    }
    Ok(())
}

fn require_new_folder(path: &Path) -> io::Result<()> {
    if path.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Choose a new folder for stem export",
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
            bender_params: Default::default(),
            lane_patches: default_lane_patches(),
            lane_mix: LaneMix::default(),
            patterns: vec![pattern, second],
            selected_pattern: 0,
            chain,
            samples: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
        }
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
            render(&project, false, plan, &written, |master, _, _| {
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
}
