use std::collections::VecDeque;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use leviathan_core::{
    AutomationRow, CvFrame, EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain,
    SongPlayer, SongStatus, SoundLock, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{
    sampler::{SampleBank, SampleRegion},
    BenderParams, DelayMode, LaneChannel, LanePatch, MasterPeaks, SynthRack,
};
use leviathan_gpu::{AudioScopeSnapshot, AudioScopeTap};

use crate::midi::{MidiEvent, MidiKey, MidiReceiver, MidiTarget};
use crate::project::{Project, PATTERN_BANK_CAPACITY};

pub enum AudioCommand {
    SetPlaying {
        playing: bool,
        request: u64,
    },
    SetFill(bool),
    SetBpm(f64),
    SetMasterSend(f32),
    SetDelayMode(DelayMode),
    SetBenderParams(BenderParams),
    SetLaneAudibleMask(u8),
    SetLaneChannels([LaneChannel; NUM_LANES]),
    SetLanePatch { lane: usize, patch: LanePatch },
    Reset,
    
    Launch(LaunchTarget),
    SetChain(PatternChain),
    SetCell {
        pattern: usize,
        lane: usize,
        row: usize,
        cell: NoteCell,
    },
    SetAutomation {
        pattern: usize,
        row: usize,
        automation: AutomationRow,
    },
    SetPattern {
        index: usize,
        pattern: Arc<Pattern>,
    },
    ReplaceProject(Arc<AudioProject>),
    LoadSampleBank {
        bank: usize,
        samples: Arc<[i16]>,
        c4_rate_hz: f32,
    },
    ResetSampleBank {
        bank: usize,
    },
    SetSampleRegion {
        bank: usize,
        region: SampleRegion,
    },
    SetMidiTarget(MidiTarget),
}

impl AudioCommand {
    pub fn apply_transport(&self, player: &mut SongPlayer, pattern_count: usize) {
        match self {
            Self::SetPlaying { playing, .. } => player.evaluator.playing = *playing,
            Self::SetFill(fill) => player.evaluator.fill_active = *fill,
            Self::SetBpm(bpm) => player.evaluator.config.bpm = bpm.clamp(40.0, 240.0),
            Self::Reset => player.reset_transport(pattern_count),
            Self::Launch(target) => player.launch(*target, pattern_count),
            Self::SetChain(chain) => player.set_chain(*chain),
            Self::SetPattern { index, .. } => player.invalidate_pattern_timing(*index),
            Self::ReplaceProject(project) => {
                *player = project.player(player.evaluator.config.sample_rate);
            }
            _ => {}
        }
    }
}

pub struct AudioProject {
    generation: u64,
    bpm: f64,
    master_send: f32,
    delay_mode: DelayMode,
    bender_params: BenderParams,
    audible_mask: u8,
    lane_channels: [LaneChannel; NUM_LANES],
    lane_patches: [LanePatch; NUM_LANES],
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: PatternChain,
    sample_bank: SampleBank,
}

impl AudioProject {
    pub fn prepare(project: &Project, generation: u64) -> Self {
        Self {
            generation,
            bpm: project.bpm,
            master_send: project.master_send,
            delay_mode: project.delay_mode,
            bender_params: project.bender_params,
            audible_mask: project.lane_mix.audible_mask(),
            lane_channels: project.lane_mix.dsp_channels(),
            lane_patches: project.lane_patches,
            patterns: project.patterns.clone(),
            selected_pattern: project.selected_pattern,
            chain: project.chain,
            sample_bank: project.sample_bank(),
        }
    }

    fn player(&self, sample_rate: f64) -> SongPlayer {
        let count = self.patterns.len().min(PATTERN_BANK_CAPACITY);
        let mut player = SongPlayer::new(EngineConfig {
            bpm: self.bpm,
            sample_rate,
            ..EngineConfig::default()
        });
        player.set_chain(self.chain);
        if count > 0 {
            player.launch(
                LaunchTarget::Pattern(self.selected_pattern.min(count - 1)),
                count,
            );
        }
        player
    }
}

enum RetainedSnapshot {
    Pattern(Arc<Pattern>),
    Project(Arc<AudioProject>),
    Sample(Arc<[i16]>),
}

impl RetainedSnapshot {
    fn in_use(&self) -> bool {
        match self {
            Self::Pattern(pattern) => Arc::strong_count(pattern) > 1,
            Self::Project(project) => Arc::strong_count(project) > 1,
            Self::Sample(sample) => Arc::strong_count(sample) > 1,
        }
    }
}

pub struct AudioControl {
    tx: rtrb::Producer<AudioCommand>,
    pending: VecDeque<AudioCommand>,
    retained: Vec<RetainedSnapshot>,
}

impl AudioControl {
    pub fn new(tx: rtrb::Producer<AudioCommand>) -> Self {
        Self {
            tx,
            pending: VecDeque::new(),
            retained: Vec::new(),
        }
    }

    pub fn send(&mut self, command: AudioCommand) {
        let snapshot = match &command {
            AudioCommand::SetPattern { pattern, .. } => {
                Some(RetainedSnapshot::Pattern(Arc::clone(pattern)))
            }
            AudioCommand::ReplaceProject(project) => {
                Some(RetainedSnapshot::Project(Arc::clone(project)))
            }
            AudioCommand::LoadSampleBank { samples, .. } => {
                Some(RetainedSnapshot::Sample(Arc::clone(samples)))
            }
            _ => None,
        };
        self.retained.extend(snapshot);
        self.pending.push_back(command);
        self.flush();
    }

    pub fn flush(&mut self) {
        if self.tx.is_abandoned() {
            self.pending.clear();
        } else {
            while let Some(command) = self.pending.pop_front() {
                if let Err(rtrb::PushError::Full(command)) = self.tx.push(command) {
                    self.pending.push_front(command);
                    break;
                }
            }
        }
        // After handoff, the UI releases the final reference once the callback is done with it.
        self.retained.retain(RetainedSnapshot::in_use);
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[derive(Clone, Copy, Default)]
pub struct AudioTelemetry {
    pub generation: u64,
    pub cv: CvFrame,
    pub song: SongStatus,
    pub master: MasterPeaks,
    pub scope: AudioScopeSnapshot,
    pub playing: bool,
    pub play_request: u64,
}

#[derive(Clone, Copy)]
struct MidiPreview {
    key: Option<MidiKey>,
    pitch: f32,
    velocity: f32,
    lock: SoundLock,
    retrigger: bool,
}

struct RtAudioWorker {
    patterns: Box<[Pattern]>,
    pattern_count: usize,
    player: SongPlayer,
    synth: Box<SynthRack>,
    scope_tap: AudioScopeTap,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
    tel_divider: u32,
    last_reported_gates: [f32; NUM_LANES],
    generation: u64,
    play_request: u64,
    midi_events: Option<MidiReceiver>,
    midi_target: MidiTarget,
    midi_preview: [Option<MidiPreview>; NUM_LANES],
}

impl RtAudioWorker {
    fn new(
        initial_pattern: Pattern,
        sample_rate: f64,
        cmd_rx: rtrb::Consumer<AudioCommand>,
        tel_tx: rtrb::Producer<AudioTelemetry>,
        midi_events: Option<MidiReceiver>,
    ) -> Self {
        let mut patterns: Box<[Pattern]> = std::iter::repeat_with(Pattern::default)
            .take(PATTERN_BANK_CAPACITY)
            .collect();
        patterns[0] = initial_pattern;
        Self {
            patterns,
            pattern_count: 1,
            player: SongPlayer::new(EngineConfig {
                sample_rate,
                ..EngineConfig::default()
            }),
            // Allocate once during setup, before the worker reaches the callback.
            synth: Box::new(SynthRack::new(sample_rate as f32)),
            scope_tap: AudioScopeTap::new(sample_rate as f32),
            cmd_rx,
            tel_tx,
            tel_divider: 0,
            last_reported_gates: [0.0; NUM_LANES],
            generation: 0,
            play_request: 0,
            midi_events,
            midi_target: MidiTarget::default(),
            midi_preview: [None; NUM_LANES],
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..self.cmd_rx.slots() {
            let Ok(command) = self.cmd_rx.pop() else {
                break;
            };
            command.apply_transport(&mut self.player, self.pattern_count);
            match command {
                AudioCommand::SetMasterSend(send) => self.synth.set_master_send(send),
                AudioCommand::SetDelayMode(mode) => self.synth.set_delay_mode(mode),
                AudioCommand::SetBenderParams(params) => self.synth.set_bender_params(params),
                AudioCommand::SetLaneAudibleMask(mask) => self.synth.set_lane_audible_mask(mask),
                AudioCommand::SetLaneChannels(channels) => self.synth.set_lane_channels(channels),
                AudioCommand::SetLanePatch { lane, patch } => self.synth.set_lane_patch(lane, patch),
                AudioCommand::SetCell {
                    pattern,
                    lane,
                    row,
                    cell,
                } => {
                    if pattern < self.pattern_count && lane < NUM_LANES && row < ROWS_PER_PATTERN {
                        self.patterns[pattern].lanes[lane][row] = cell;
                    }
                }
                AudioCommand::SetAutomation {
                    pattern,
                    row,
                    automation,
                } => {
                    if pattern < self.pattern_count && row < ROWS_PER_PATTERN {
                        self.patterns[pattern].automation[row] = automation;
                    }
                }
                AudioCommand::SetPattern { index, pattern } => {
                    if index <= self.pattern_count && index < self.patterns.len() {
                        self.patterns[index].clone_from(&pattern);
                        self.pattern_count = self.pattern_count.max(index + 1);
                    }
                }
                AudioCommand::ReplaceProject(project) => {
                    for (dest, source) in self.patterns.iter_mut().zip(&project.patterns) {
                        dest.clone_from(source);
                    }
                    self.pattern_count = project.patterns.len().min(self.patterns.len());
                    self.synth.sample_bank.clone_from(&project.sample_bank);
                    self.synth.set_lane_audible_mask(project.audible_mask);
                    self.synth.set_lane_channels(project.lane_channels);
                    self.synth.set_lane_patches(project.lane_patches);
                    self.synth.reset_voices();
                    self.synth.set_delay_mode(project.delay_mode);
                    self.synth.set_bender_params(project.bender_params);
                    self.synth.reset_master(project.master_send);
                    self.generation = project.generation;
                    self.tel_divider = 255;
                    // Match the project-open cursor before the next MIDI batch.
                    self.midi_target = MidiTarget {
                        pattern: self.player.status().pattern,
                        ..MidiTarget::default()
                    };
                    self.midi_preview = [None; NUM_LANES];
                    if let Some(events) = &mut self.midi_events {
                        events.discard_pending();
                    }
                }
                AudioCommand::LoadSampleBank {
                    bank,
                    samples,
                    c4_rate_hz,
                } => {
                    self.synth
                        .sample_bank
                        .load_pcm_with_rate(bank, &samples, c4_rate_hz);
                }
                AudioCommand::ResetSampleBank { bank } => {
                    self.synth.sample_bank.reset_rom_bank(bank)
                }
                AudioCommand::SetSampleRegion { bank, region } => {
                    self.synth.sample_bank.set_region(bank, region);
                }
                AudioCommand::SetPlaying { request, .. } => self.play_request = request,
                AudioCommand::SetMidiTarget(target) => self.midi_target = target,
                _ => {}
            }
        }
        self.drain_midi();
    }

    fn drain_midi(&mut self) {
        let Some(events) = &mut self.midi_events else {
            return;
        };
        let (release, count) = events.begin_batch();
        if release {
            self.release_midi(None);
        }
        for _ in 0..count {
            let event = self.midi_events.as_mut().and_then(MidiReceiver::pop);
            match event {
                Some(MidiEvent::NoteOn { key, velocity }) => {
                    self.release_midi(Some(key));
                    let target = self.midi_target;
                    if target.pattern < self.pattern_count
                        && target.lane < NUM_LANES
                        && target.row < ROWS_PER_PATTERN
                    {
                        let cell = self.patterns[target.pattern].lanes[target.lane][target.row];
                        let config = &self.player.evaluator.config;
                        let semitones = i32::from(key.note) + config.key_offset - config.pitch_zero;
                        self.midi_preview[target.lane] = Some(MidiPreview {
                            key: Some(key),
                            pitch: semitones as f32 / 12.0,
                            velocity: f32::from(velocity) / 127.0 * config.v_high,
                            lock: cell.lock,
                            retrigger: true,
                        });
                    }
                }
                Some(MidiEvent::NoteOff(key)) => self.release_midi(Some(key)),
                Some(event) => {
                    if event.apply_transport(&mut self.player, self.pattern_count) {
                        self.tel_divider = 255;
                    }
                }
                None => {}
            }
        }
    }

    fn release_midi(&mut self, key: Option<MidiKey>) {
        for preview in self.midi_preview.iter_mut().flatten() {
            if key.is_none() || preview.key == key {
                preview.key = None;
            }
        }
    }

    fn render_with_midi(&mut self, mut cv: CvFrame, retrigger: bool) -> (f32, f32) {
        let mut lanes = if retrigger { u8::MAX } else { 0 };
        for (lane, slot) in self.midi_preview.iter_mut().enumerate() {
            let Some(preview) = slot else { continue };
            let bit = 1 << lane;
            let held = preview.key.is_some();
            let attack = std::mem::take(&mut preview.retrigger);
            if !held && !attack && cv.gate_v[lane] > 2.5 {
                *slot = None;
                lanes |= bit;
                continue;
            }
            // A held note keeps its own gate and attack through sequence launches.
            // After release, preserve its tail until the next sequenced gate.
            lanes &= !bit;
            cv.retrigger_mask &= !bit;
            if attack {
                lanes |= bit;
            }
            cv.pitch_v[lane] = preview.pitch;
            cv.velocity_v[lane] = preview.velocity;
            cv.locks[lane] = preview.lock;
            cv.gate_v[lane] = if held || attack {
                self.player.evaluator.config.v_high
            } else {
                0.0
            };
        }
        if lanes != 0 {
            self.synth.retrigger_lanes(lanes);
        }
        self.synth.render_stereo(&cv)
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        let (cv, retrigger) = self
            .player
            .step_sample(&self.patterns[..self.pattern_count]);
        let pair = if self.midi_preview.iter().any(Option::is_some) {
            self.render_with_midi(cv, retrigger)
        } else {
            if retrigger {
                self.synth.retrigger_gates();
            }
            self.synth.render_stereo(&cv)
        };
        self.scope_tap.push_stereo(pair.0, pair.1);
        self.tel_divider = self.tel_divider.wrapping_add(1);
        if retrigger
            || cv.retrigger_mask != 0
            || cv.gate_v != self.last_reported_gates
            || (self.tel_divider & 255) == 0
        {
            let telemetry = AudioTelemetry {
                generation: self.generation,
                cv,
                song: self.player.status(),
                master: self.synth.master_peaks(),
                scope: self.scope_tap.snapshot(),
                playing: self.player.evaluator.playing,
                play_request: self.play_request,
            };
            // Carry the interval's peaks forward if the UI hasn't drained its ring yet.
            if self.tel_tx.push(telemetry).is_ok() {
                self.last_reported_gates = cv.gate_v;
                self.synth.clear_master_peaks();
            }
        }
        pair
    }
}

pub fn try_start_audio(
    initial_pattern: Pattern,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
    midi_events: MidiReceiver,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return Err("Offline UI clock (no audio device)".to_string());
    };
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Audio config unavailable: {e}"))?;

    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let sample_rate = config.sample_rate as f64;
    let channels = (config.channels as usize).max(1);

    let mut worker = RtAudioWorker::new(
        initial_pattern,
        sample_rate,
        cmd_rx,
        tel_tx,
        Some(midi_events),
    );

    let err_fn = |err| eprintln!("leviathan audio stream error: {err}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    worker.drain_commands();
                    for frame in data.chunks_mut(channels) {
                        let (l, r) = worker.next_stereo();
                        frame[0] = l;
                        if channels > 1 {
                            frame[1] = r;
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0.0;
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Stream init failed: {e}"))?,
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    worker.drain_commands();
                    for frame in data.chunks_mut(channels) {
                        let (l, r) = worker.next_stereo();
                        frame[0] = (l.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        if channels > 1 {
                            frame[1] = (r.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = 0;
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Stream init failed: {e}"))?,
        cpal::SampleFormat::U16 => device
            .build_output_stream(
                &config,
                move |data: &mut [u16], _| {
                    worker.drain_commands();
                    for frame in data.chunks_mut(channels) {
                        let (l, r) = worker.next_stereo();
                        frame[0] = ((l.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16;
                        if channels > 1 {
                            frame[1] = ((r.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16;
                        }
                        for extra in frame.iter_mut().skip(2) {
                            *extra = u16::MAX / 2;
                        }
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| format!("Stream init failed: {e}"))?,
        other => return Err(format!("Unsupported sample format: {other:?}")),
    };

    stream
        .play()
        .map_err(|e| format!("Audio play failed: {e}"))?;
    Ok((
        stream,
        format!("cpal {:.0} Hz ({} ch)", sample_rate, channels),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportJob, ExportRequest, ExportSettings};
    use crate::lane_mix::LaneMix;
    use crate::midi::MidiControls;
    use crate::wav::{sample_peaks, DecodedWav, WavEncoding};
    use leviathan_core::PlaybackRate;

    #[test]
    fn saved_fs77_lane_and_step_lock_match_live_and_wav() {
        use leviathan_dsp::{default_lane_patches, MachineKind};

        let directory =
            std::env::temp_dir().join(format!("leviathan-fs77-parity-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("formants.leviathan.json");
        let destination = directory.join("stems");
        let mut pattern = Pattern {
            length: 4,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        let mut patches = default_lane_patches();
        patches[0] = LanePatch::for_machine(MachineKind::Fs77FormantFm);
        patches[0].params[3] = 0.0;
        patches[0].params[7] = 0.12;
        let note = NoteCell {
            active: true,
            midi: 48,
            velocity: 115,
            gate_fraction: 1.0,
            ..NoteCell::default()
        };
        pattern.lanes[0][0] = note;
        let mut lock = SoundLock {
            machine: 15,
            mask: (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 7),
            values: patches[0].params,
        };
        lock.values[2] = 0.85;
        lock.values[3] = 1.0;
        lock.values[4] = 0.75;
        lock.values[5] = 0.85;
        lock.values[7] = 0.85;
        let first_lock = lock;
        pattern.lanes[1][1] = NoteCell { lock, ..note };
        // A second vowel and DAC region on the same voice exercises the crossfade.
        lock.values[3] = 0.5;
        lock.values[7] = 0.15;
        pattern.lanes[1][2] = NoteCell { lock, ..note };
        crate::project::save(
            &path,
            Project {
                bpm: 120.0,
                master_send: 0.0,
                delay_mode: Default::default(),
                bender_params: Default::default(),
                lane_mix: LaneMix::default(),
                lane_patches: patches,
                patterns: vec![pattern],
                selected_pattern: 0,
                chain: PatternChain::default(),
                samples: Default::default(),
                sample_regions: Default::default(),
                custom_shader: None,
            },
        )
        .unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains(r#""machine":"fs77""#));
        assert!(saved.contains(r#""machine":15"#));
        let project = crate::project::load(&path).unwrap();
        assert_eq!(project.lane_patches, patches);
        assert_eq!(project.patterns[0].lanes[1][1].lock, first_lock);
        assert_eq!(project.patterns[0].lanes[1][2].lock, lock);

        let settings = ExportSettings {
            encoding: WavEncoding::Float32,
            stems: true,
            tail_seconds: 0.01,
            ..ExportSettings::default()
        };
        // Four rows at 8× and 120 BPM are 3,000 frames at 48 kHz.
        let music_frames = 3_000;
        let live = render_test_project_live(&project, music_frames, music_frames + 480);
        ExportJob::start(
            project,
            false,
            ExportRequest {
                destination: destination.clone(),
                settings,
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let master = std::fs::read(destination.join("master.wav")).unwrap();
        let lane = std::fs::read(destination.join("0-fs77.wav")).unwrap();
        let locked_lane =
            std::fs::read(destination.join(format!("1-{}.wav", patches[1].machine.slug()))).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        let data_start = master.len() - live.len() * 8;
        assert_eq!(&master[data_start - 8..data_start - 4], b"data");
        let read = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap());
        for stem in [&lane, &locked_lane] {
            assert_eq!(stem.len(), master.len());
            assert!(stem[data_start..].chunks_exact(4).any(|s| read(s).abs() > 0.01));
        }
        for (n, frame) in master[data_start..].chunks_exact(8).enumerate() {
            assert_eq!(live[n], (read(&frame[..4]), read(&frame[4..])), "frame {n}");
        }
    }

    #[test]
    fn saved_k26_lane_and_step_locks_match_live_and_wav() {
        use leviathan_dsp::{default_lane_patches, MachineKind};

        let directory =
            std::env::temp_dir().join(format!("leviathan-k26-parity-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("k26.leviathan.json");
        let destination = directory.join("stems");
        let mut pattern = Pattern {
            length: 4,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        let mut patches = default_lane_patches();
        patches[0] = LanePatch::for_machine(MachineKind::VastTriple);
        patches[0].params[1] = 0.85;
        patches[1] = LanePatch::for_machine(MachineKind::VastFunSynth);
        let note = NoteCell {
            active: true,
            midi: 48,
            velocity: 115,
            gate_fraction: 1.0,
            ..NoteCell::default()
        };
        pattern.lanes[0][0] = note;
        let mut lock = SoundLock {
            machine: 16,
            mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 7),
            values: LanePatch::for_machine(MachineKind::VastTriple).params,
        };
        lock.values[1] = 0.65;
        lock.values[3] = 0.20;
        let first_lock = lock;
        pattern.lanes[1][1] = NoteCell { lock, ..note };
        lock.values[3] = 0.92;
        lock.values[4] = 0.70;
        lock.values[7] = 0.90;
        pattern.lanes[1][2] = NoteCell { lock, ..note };
        crate::project::save(
            &path,
            Project {
                bpm: 120.0,
                master_send: 0.0,
                delay_mode: Default::default(),
                bender_params: Default::default(),
                lane_mix: LaneMix::default(),
                lane_patches: patches,
                patterns: vec![pattern],
                selected_pattern: 0,
                chain: PatternChain::default(),
                samples: Default::default(),
                sample_regions: Default::default(),
                custom_shader: None,
            },
        )
        .unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains(r#""machine":"k26""#));
        assert!(saved.contains(r#""machine":16"#));
        let project = crate::project::load(&path).unwrap();
        assert_eq!(project.lane_patches, patches);
        assert_eq!(project.patterns[0].lanes[1][1].lock, first_lock);
        assert_eq!(project.patterns[0].lanes[1][2].lock, lock);
        let music_frames = 3_000;
        let live = render_test_project_live(&project, music_frames, music_frames + 480);
        ExportJob::start(
            project,
            false,
            ExportRequest {
                destination: destination.clone(),
                settings: ExportSettings {
                    encoding: WavEncoding::Float32,
                    stems: true,
                    tail_seconds: 0.01,
                    ..ExportSettings::default()
                },
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let master = std::fs::read(destination.join("master.wav")).unwrap();
        let lane = std::fs::read(destination.join("0-k26.wav")).unwrap();
        let locked_lane = std::fs::read(destination.join("1-vast.wav")).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        let data_start = master.len() - live.len() * 8;
        assert_eq!(&master[data_start - 8..data_start - 4], b"data");
        let read = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap());
        assert_eq!(lane.len(), master.len());
        assert_eq!(locked_lane.len(), master.len());
        for (stem, first, last) in [
            (&lane, 0, 750),
            (&locked_lane, 750, 1_500),
            (&locked_lane, 1_500, 2_250),
        ] {
            let samples = &stem[data_start + first * 8..data_start + last * 8];
            assert!(samples.chunks_exact(4).any(|s| read(s).abs() > 0.01));
        }
        for (n, frame) in master[data_start..].chunks_exact(8).enumerate() {
            assert_eq!(live[n], (read(&frame[..4]), read(&frame[4..])), "frame {n}");
        }
    }

    fn render_test_project_live(
        project: &Project,
        music_frames: usize,
        frames: usize,
    ) -> Vec<(f32, f32)> {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        audio.send(AudioCommand::ReplaceProject(Arc::new(AudioProject::prepare(
            project, 1,
        ))));
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 1,
        });
        worker.drain_commands();
        let mut rendered = Vec::with_capacity(frames);
        for n in 0..frames {
            if n == music_frames {
                audio.send(AudioCommand::SetPlaying {
                    playing: false,
                    request: 2,
                });
                worker.drain_commands();
            }
            rendered.push(worker.next_stereo());
        }
        rendered
    }

    #[test]
    fn wav_master_matches_live_playback_with_bbus_otis_twin_delay_and_bend() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(32);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        let mut pattern = Pattern {
            length: 2,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        pattern.lanes[4][0] = NoteCell {
            active: true,
            gate_fraction: 0.8,
            retrig: 3,
            lock: SoundLock {
                mask: 1 << 3,
                ..SoundLock::default()
            },
            ..NoteCell::default()
        };
        pattern.lanes[0][0] = NoteCell {
            active: true,
            gate_fraction: 1.0,
            ..NoteCell::default()
        };
        pattern.lanes[0][1] = pattern.lanes[0][0];
        pattern.lanes[1][0] = pattern.lanes[0][0];
        pattern.automation[0].byte_word = 0xa4;
        pattern.automation[1].byte_override = true;
        let mut lane_patches = leviathan_dsp::default_lane_patches();
        lane_patches[0] = LanePatch::for_machine(leviathan_dsp::MachineKind::ByteBusR2r);
        lane_patches[1] = LanePatch::for_machine(leviathan_dsp::MachineKind::OtisTwin);
        let mut lane_mix = crate::lane_mix::LaneMix::default();
        lane_mix.channels[4].0 = LaneChannel {
            level: 1.0,
            pan: -1.0,
            delay_send: 0.75,
            fx_send: 0.8,
        };
        let mut sample_regions = [SampleRegion::default(); leviathan_dsp::sampler::NUM_BANKS];
        sample_regions[0] = SampleRegion {
            trim_start: 0.05,
            trim_end: 0.2,
            loop_enabled: true,
        };
        let bender_params = BenderParams {
            shift_norm: 0.27,
            feedback: 0.62,
            decay: 0.7,
            freeze: false,
        };
        let project = |freeze| Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: BenderParams {
                freeze,
                ..bender_params
            },
            lane_patches,
            lane_mix,
            patterns: vec![pattern.clone()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions,
            custom_shader: None,
        };
        let destination =
            std::env::temp_dir().join(format!("leviathan-mixer-parity-{}", std::process::id()));
        let settings = ExportSettings {
            encoding: WavEncoding::Float32,
            stems: true,
            tail_seconds: 0.07,
            ..ExportSettings::default()
        };
        let bounce = |name, stems, freeze| {
            ExportJob::start(
                project(freeze),
                false,
                ExportRequest {
                    settings: ExportSettings { stems, ..settings },
                    destination: destination.join(name),
                },
            )
            .unwrap()
            .finish()
            .unwrap();
        };
        std::fs::create_dir(&destination).unwrap();
        bounce("wet", true, false);
        bounce("wet.wav", false, false);
        bounce("frozen", true, true);

        let master = std::fs::read(destination.join("wet/master.wav")).unwrap();
        assert_eq!(std::fs::read(destination.join("wet.wav")).unwrap(), master);
        let stem = std::fs::read(destination.join("wet/4-smpl.wav")).unwrap();
        let bbus = std::fs::read(destination.join("wet/0-bbus.wav")).unwrap();
        let otis_twin = std::fs::read(destination.join("wet/1-otis2.wav")).unwrap();
        let bend = std::fs::read(destination.join("wet/bend.wav")).unwrap();
        let frozen_master = std::fs::read(destination.join("frozen/master.wav")).unwrap();
        let frozen_bend = std::fs::read(destination.join("frozen/bend.wav")).unwrap();
        assert_ne!(frozen_master, master);
        assert_eq!(
            std::fs::read(destination.join("frozen/4-smpl.wav")).unwrap(),
            stem
        );
        std::fs::remove_dir_all(destination).unwrap();
        let frames = 1500 + 3360;
        let data_offset = master.len() - frames * 8;
        for bytes in [&master, &stem, &bbus, &otis_twin, &bend, &frozen_bend] {
            assert_eq!(bytes.len(), master.len());
            assert_eq!(&bytes[data_offset - 8..data_offset - 4], b"data");
            let data_len = u32::from_le_bytes(bytes[data_offset - 4..data_offset].try_into().unwrap());
            assert_eq!(data_len as usize, frames * 8);
        }
        let read_channel = |bytes: &[u8], frame: usize, channel: usize| {
            let offset = data_offset + 8 * frame + 4 * channel;
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };
        for (generation, (freeze, expected)) in [(false, &master), (true, &frozen_master)]
            .into_iter()
            .enumerate()
        {
            let generation = generation as u64 + 1;
            audio.send(AudioCommand::ReplaceProject(Arc::new(AudioProject::prepare(
                &project(freeze),
                generation,
            ))));
            audio.send(AudioCommand::SetPlaying {
                playing: true,
                request: generation * 2,
            });
            worker.drain_commands();
            assert_eq!(worker.synth.sample_bank.regions[0], sample_regions[0]);
            for frame in 0..frames {
                if frame == 1500 {
                    audio.send(AudioCommand::SetPlaying {
                        playing: false,
                        request: generation * 2 + 1,
                    });
                    worker.drain_commands();
                }
                let (left, right) = worker.next_stereo();
                assert_eq!(read_channel(expected, frame, 0), left);
                assert_eq!(read_channel(expected, frame, 1), right);
            }
        }

        let frozen_tail_peak = (1500..frames)
            .map(|frame| read_channel(&frozen_bend, frame, 0).abs())
            .fold(0.0_f32, f32::max);
        assert!(frozen_tail_peak > 0.001, "{frozen_tail_peak}");

        let mut energy = [0.0; 5];
        for frame in 0..frames {
            let stem_right = read_channel(&stem, frame, 1);
            if frame < 1008 {
                assert_eq!(stem_right, 0.0);
                energy[0] += read_channel(&stem, frame, 0).abs();
            } else {
                energy[1] += stem_right.abs();
            }
            energy[2] += read_channel(&bend, frame, 0).abs() + read_channel(&bend, frame, 1).abs();
            energy[3] += read_channel(&bbus, frame, 0).abs();
            energy[4] += read_channel(&otis_twin, frame, 0).abs();
        }
        assert!(energy.iter().all(|&sum| sum > 0.1), "{energy:?}");
    }

    #[test]
    fn long_delay_matches_live_playback_and_crosses_in_the_exported_stem() {
        use leviathan_dsp::{default_lane_patches, MachineKind};

        let mut pattern = Pattern { length: 1, ..Pattern::default() };
        pattern.lanes[0][0] = NoteCell {
            active: true,
            midi: 60,
            velocity: 110,
            ..NoteCell::default()
        };
        let mut lane_mix = LaneMix::default();
        lane_mix.channels[0].0.pan = -1.0;
        lane_mix.channels[0].0.delay_send = 1.0;
        let project = Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: DelayMode::LongPingPong,
            bender_params: Default::default(),
            lane_mix,
            lane_patches: default_lane_patches(),
            patterns: vec![pattern.clone()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions: Default::default(),
            custom_shader: None,
        };
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(32);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(pattern, 48_000.0, rx, tel_tx, None);
        audio.send(AudioCommand::ReplaceProject(Arc::new(AudioProject::prepare(&project, 1))));
        audio.send(AudioCommand::SetPlaying { playing: true, request: 1 });
        worker.drain_commands();

        let dir = std::env::temp_dir().join(format!("leviathan-long-delay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let destination = dir.join("stems");
        ExportJob::start(
            project,
            false,
            ExportRequest {
                settings: ExportSettings {
                    encoding: WavEncoding::Float32,
                    stems: true,
                    tail_seconds: 0.35,
                    ..ExportSettings::default()
                },
                destination: destination.clone(),
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let master = std::fs::read(destination.join("master.wav")).unwrap();
        let stem = std::fs::read(destination.join(format!("0-{}.wav", MachineKind::KickDrum.slug())))
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();

        let music_frames = 6_000;
        let frames = music_frames + 16_800;
        let data_offset = master.len() - frames * 8;
        assert_eq!(master.len(), stem.len());
        let read_channel = |bytes: &[u8], frame: usize, channel: usize| {
            let offset = data_offset + 8 * frame + 4 * channel;
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };
        let mut right_peak = 0.0_f32;
        for frame in 0..frames {
            if frame == music_frames {
                audio.send(AudioCommand::SetPlaying { playing: false, request: 2 });
                worker.drain_commands();
            }
            let (left, right) = worker.next_stereo();
            assert_eq!(read_channel(&master, frame, 0), left);
            assert_eq!(read_channel(&master, frame, 1), right);
            let stem_right = read_channel(&stem, frame, 1);
            if frame < 8_000 {
                assert_eq!(stem_right, 0.0);
            }
            right_peak = right_peak.max(stem_right.abs());
        }
        assert!(right_peak > 0.01, "{right_peak}");
    }

    #[test]
    fn freeze_command_holds_audition_tail_through_lane_mute_and_releases_it() {
        let (mut midi, receiver) = MidiControls::new();
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx, Some(receiver));
        let mut channels = leviathan_dsp::DEFAULT_LANE_CHANNELS;
        channels[1].fx_send = 1.0;
        audio.send(AudioCommand::SetLaneChannels(channels));
        audio.send(AudioCommand::SetMidiTarget(MidiTarget {
            lane: 1,
            ..MidiTarget::default()
        }));
        midi.inject(MidiEvent::NoteOn {
            key: MidiKey {
                channel: 0,
                note: 57,
            },
            velocity: 110,
        });
        worker.drain_commands();
        let energy = |worker: &mut RtAudioWorker, frames| {
            (0..frames)
                .map(|_| {
                    let (l, r) = worker.next_stereo();
                    l * l + r * r
                })
                .sum::<f32>()
        };
        assert!(energy(&mut worker, 1000) > 0.1);
        audio.send(AudioCommand::SetBenderParams(BenderParams {
            freeze: true,
            ..BenderParams::default()
        }));
        audio.send(AudioCommand::SetLaneAudibleMask(0));
        worker.drain_commands();
        energy(&mut worker, 8_000);
        assert!(energy(&mut worker, 4_000) > 1e-4);
        audio.send(AudioCommand::SetBenderParams(BenderParams::default()));
        worker.drain_commands();
        energy(&mut worker, 8_000);
        assert!(energy(&mut worker, 4_000) < 1e-8);
        assert!(worker.midi_preview[1].unwrap().key.is_some());
    }

    #[test]
    fn full_command_ring_preserves_order_and_snapshot_ownership() {
        let (tx, mut rx) = rtrb::RingBuffer::new(1);
        let mut control = AudioControl::new(tx);
        control.send(AudioCommand::SetBpm(137.0));
        let pattern = Arc::new(Pattern::default());
        let weak = Arc::downgrade(&pattern);

        control.send(AudioCommand::SetPattern { index: 1, pattern });
        control.send(AudioCommand::Launch(LaunchTarget::Pattern(1)));
        assert!(control.has_pending());
        assert!(matches!(rx.pop(), Ok(AudioCommand::SetBpm(137.0))));
        control.flush();
        assert!(matches!(
            rx.pop(),
            Ok(AudioCommand::SetPattern { index: 1, .. })
        ));
        assert!(weak.upgrade().is_some());
        control.flush();
        assert!(weak.upgrade().is_none());
        assert!(matches!(
            rx.pop(),
            Ok(AudioCommand::Launch(LaunchTarget::Pattern(1)))
        ));
        assert!(!control.has_pending());
    }

    #[test]
    fn restored_pattern_and_sample_bank_reach_the_audio_callback_together() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx, None);

        let first = Pattern {
            length: 1,
            ..Pattern::default()
        };
        let mut second = Pattern {
            rate: PlaybackRate::Half,
            ..Pattern::default()
        };
        second.lanes[4][0] = NoteCell {
            active: true,
            midi: 72,
            ..NoteCell::default()
        };
        second.automation[0] = AutomationRow {
            z: 0.75,
            ..AutomationRow::default()
        };
        let mut chain = PatternChain::default();
        chain.push(0);
        chain.push(1);
        let pcm = [0, 4096, -4096, 0];
        let mut samples = core::array::from_fn(|_| None);
        samples[0] = Some(DecodedWav {
            name: "embedded.wav".to_string(),
            samples: Arc::from(pcm),
            c4_rate_hz: 8_000.0,
            original_frames: pcm.len(),
            original_sr: 8_000,
            peaks: sample_peaks(&pcm),
        });
        let project = Project {
            bpm: 120.0,
            master_send: 0.4,
            delay_mode: Default::default(),
            bender_params: BenderParams::default(),
            lane_patches: leviathan_dsp::default_lane_patches(),
            lane_mix: Default::default(),
            patterns: vec![first, second],
            selected_pattern: 1,
            chain,
            samples,
            sample_regions: Default::default(),
            custom_shader: None,
        };
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 0,
        });
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&project, 1),
        )));
        worker.drain_commands();
        assert!(!worker.player.evaluator.playing);
        assert_eq!(worker.player.status().pattern, 1);
        assert_eq!(&worker.synth.sample_bank.waves[0][..pcm.len()], &pcm);

        audio.send(AudioCommand::Launch(LaunchTarget::Chain));
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 0,
        });
        worker.drain_commands();
        for _ in 0..=1000 {
            worker.next_stereo();
        }
        let mut latest = AudioTelemetry::default();
        while let Ok(telemetry) = tel_rx.pop() {
            latest = telemetry;
        }
        assert_eq!(latest.generation, 1);
        assert_eq!(latest.song.chain_entry, Some(1));
        assert_eq!(latest.song.pattern, 1);
        assert_eq!(
            (
                latest.cv.beat,
                latest.cv.row,
                latest.cv.gate_v[4],
                latest.cv.z_v
            ),
            (0.25, 0, 10.0, 7.5)
        );
    }

    #[test]
    fn set_pattern_refreshes_active_timing_and_replace_project_clamps_bank_count() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 100.0, rx, tel_tx, None);

        let oversized = Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: BenderParams::default(),
            lane_patches: leviathan_dsp::default_lane_patches(),
            lane_mix: Default::default(),
            patterns: vec![Pattern::default(); PATTERN_BANK_CAPACITY + 4],
            selected_pattern: PATTERN_BANK_CAPACITY + 2,
            chain: PatternChain::default(),
            samples: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
        };
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&oversized, 2),
        )));
        worker.drain_commands();
        assert_eq!(worker.pattern_count, PATTERN_BANK_CAPACITY);
        assert_eq!(worker.player.status().pattern, PATTERN_BANK_CAPACITY - 1);

        audio.send(AudioCommand::Launch(LaunchTarget::Pattern(0)));
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 0,
        });
        worker.drain_commands();
        for _ in 0..50 {
            worker.next_stereo();
        }
        audio.send(AudioCommand::SetPlaying {
            playing: false,
            request: 0,
        });
        audio.send(AudioCommand::SetPattern {
            index: 0,
            pattern: Arc::new(Pattern {
                length: 4,
                rate: PlaybackRate::FourX,
                ..Pattern::default()
            }),
        });
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 0,
        });
        worker.drain_commands();
        worker.next_stereo();
        assert_eq!(worker.player.status().pattern, 0);
    }

    #[test]
    fn master_peaks_survive_a_full_telemetry_ring() {
        let (_tx, rx) = rtrb::RingBuffer::new(1);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(1);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        assert!(worker.tel_tx.push(AudioTelemetry::default()).is_ok());

        let lock = leviathan_core::SoundLock {
            machine: leviathan_dsp::MachineKind::AcidLead as i16,
            mask: 0b101,
            values: [0.98, 0.0, 0.88, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let cv = CvFrame {
            pitch_v: [-12, -8, -5, 0, 4, 7].map(|semitones| semitones as f32 / 12.0),
            gate_v: [10.0; NUM_LANES],
            velocity_v: [10.0; NUM_LANES],
            locks: [lock; NUM_LANES],
            ..CvFrame::default()
        };
        for _ in 0..512 {
            worker.synth.render_stereo(&cv);
        }
        let peaks = worker.synth.master_peaks();
        assert_eq!(peaks.clipped, [true, true]);

        worker.tel_divider = 255;
        worker.next_stereo();
        let _ = tel_rx.pop().unwrap();
        worker.tel_divider = 255;
        worker.next_stereo();
        let reported = tel_rx.pop().unwrap().master;
        assert_eq!(reported.clipped, peaks.clipped);
        assert!(reported.output[0] >= peaks.output[0] && reported.output[1] >= peaks.output[1]);
        assert_eq!(worker.synth.master_peaks(), MasterPeaks::default());
    }
    #[test]
    fn set_automation_updates_live_cv_and_bytebus_telemetry() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 100.0, rx, tel_tx, None);

        audio.send(AudioCommand::SetAutomation {
            pattern: 0,
            row: 0,
            automation: AutomationRow {
                z: 0.8,
                roll: -0.5,
                feedback: 0.6,
                byte_word: 0b0000_0101,
                byte_override: false,
            },
        });
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 0,
        });
        worker.drain_commands();
        worker.next_stereo();

        let telemetry = tel_rx.pop().unwrap();
        assert!((telemetry.cv.z_v - 8.0).abs() < 1e-4);
        assert!((telemetry.cv.roll_v - (-2.5)).abs() < 1e-4);
        assert!((telemetry.cv.feedback_v - 6.0).abs() < 1e-4);
        assert!(telemetry.cv.bytebus_v[0] > 5.0);
        assert!(telemetry.cv.bytebus_v[1] < 5.0);
        assert!(telemetry.cv.bytebus_v[2] > 5.0);
    }

    #[test]
    fn midi_auditions_when_stopped_and_releases_to_the_running_pattern() {
        let (mut midi, receiver) = MidiControls::new();
        let (mut tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(16);
        let mut pattern = Pattern::default();
        pattern.lanes[5][0] = NoteCell {
            active: true,
            midi: 48,
            ..NoteCell::default()
        };
        let mut worker = RtAudioWorker::new(pattern, 48_000.0, rx, tel_tx, Some(receiver));
        assert!(tx
            .push(AudioCommand::SetMidiTarget(MidiTarget {
                pattern: 0,
                lane: 5,
                row: 0
            }))
            .is_ok());
        let key = MidiKey {
            channel: 2,
            note: 72,
        };
        midi.inject(MidiEvent::NoteOn { key, velocity: 100 });
        worker.drain_commands();
        assert!(!worker.player.evaluator.playing);
        assert_eq!(worker.midi_preview[5].unwrap().pitch, 1.0);
        let energy: f32 = (0..256)
            .map(|_| {
                let (left, right) = worker.next_stereo();
                left.abs() + right.abs()
            })
            .sum();
        assert!(energy > 0.1);
        assert_eq!(tel_rx.pop().unwrap().cv.gate_v, [0.0; NUM_LANES]);

        midi.inject(MidiEvent::Start);
        midi.inject(MidiEvent::NoteOff(MidiKey { channel: 3, ..key }));
        worker.drain_commands();
        worker.next_stereo();
        assert!(worker.player.evaluator.playing);
        assert_eq!(worker.midi_preview[5].unwrap().key, Some(key));
        midi.inject(MidiEvent::NoteOff(key));
        worker.drain_commands();
        worker.next_stereo();
        assert!(worker.midi_preview[5].is_none());
    }

    #[test]
    fn held_midi_preview_follows_lane_mute_and_solo() {
        let (mut midi, receiver) = MidiControls::new();
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx, Some(receiver));
        let lane = 5;
        audio.send(AudioCommand::SetLanePatch {
            lane,
            patch: LanePatch::for_machine(leviathan_dsp::MachineKind::ByteBusR2r),
        });
        let key = MidiKey {
            channel: 0,
            note: 72,
        };
        audio.send(AudioCommand::SetMidiTarget(MidiTarget {
            lane,
            ..MidiTarget::default()
        }));
        midi.inject(MidiEvent::NoteOn { key, velocity: 100 });
        worker.drain_commands();

        let peak = |worker: &mut RtAudioWorker, frames| {
            (0..frames)
                .map(|_| {
                    let (left, right) = worker.next_stereo();
                    left.abs().max(right.abs())
                })
                .fold(0.0, f32::max)
        };
        assert!(peak(&mut worker, 64) > 0.01);

        let mut mix = LaneMix {
            solo_mask: 1,
            ..LaneMix::default()
        };
        audio.send(AudioCommand::SetLaneAudibleMask(mix.audible_mask()));
        worker.drain_commands();
        peak(&mut worker, 1024);
        assert!(peak(&mut worker, 64) < 1e-5);

        mix.solo_mask |= 1 << lane;
        mix.mute_mask = 1 << lane;
        audio.send(AudioCommand::SetLaneAudibleMask(mix.audible_mask()));
        worker.drain_commands();
        assert!(peak(&mut worker, 64) < 1e-5);
        assert_eq!(worker.midi_preview[lane].unwrap().key, Some(key));

        mix.mute_mask = 0;
        audio.send(AudioCommand::SetLaneAudibleMask(mix.audible_mask()));
        worker.drain_commands();
        assert!(peak(&mut worker, 128) > 0.01);

        mix.mute_mask = 1 << lane;
        audio.send(AudioCommand::SetLaneAudibleMask(mix.audible_mask()));
        midi.inject(MidiEvent::NoteOff(key));
        worker.drain_commands();
        assert_eq!(worker.midi_preview[lane].unwrap().key, None);
    }

    #[test]
    fn project_replacement_switches_midi_target_and_mix_together() {
        let (mut midi, receiver) = MidiControls::new();
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx, Some(receiver));
        let key = MidiKey {
            channel: 0,
            note: 72,
        };
        audio.send(AudioCommand::SetMidiTarget(MidiTarget {
            lane: 5,
            row: 2,
            ..MidiTarget::default()
        }));
        midi.inject(MidiEvent::NoteOn { key, velocity: 100 });
        worker.drain_commands();
        assert_eq!(worker.midi_preview[5].unwrap().key, Some(key));
        worker.next_stereo();

        let mut selected = Pattern::default();
        let lock = SoundLock {
            mask: 1,
            values: [0.7; 8],
            ..SoundLock::default()
        };
        selected.lanes[0][0].lock = lock;
        let project = Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: BenderParams::default(),
            lane_patches: leviathan_dsp::default_lane_patches(),
            lane_mix: LaneMix {
                mute_mask: 1,
                solo_mask: 0b11, ..LaneMix::default()
            },
            patterns: vec![Pattern::default(), selected],
            selected_pattern: 1,
            chain: PatternChain::default(),
            samples: core::array::from_fn(|
              _| None),
            sample_regions: Default::default(),
          custom_shader: None,
        };
        let snapshot = Arc::new(AudioProject::prepare(&project, 1));
        let retained = Arc::downgrade(&snapshot);
        midi.inject(MidiEvent::Start);
        audio.send(AudioCommand::ReplaceProject(snapshot));
        worker.drain_commands();
        assert!(worker.midi_preview.iter().all(Option::is_none));
        assert!(!worker.player.evaluator.playing);
        assert!(retained.upgrade().is_some());
        audio.flush();
        assert!(retained.upgrade().is_none());

        // A note can arrive before the next UI command reaches the worker.
        midi.inject(MidiEvent::NoteOn { key, velocity: 100 });
        worker.drain_commands();
        let preview = worker.midi_preview[0].expect("preview should use the restored cursor lane");
        assert_eq!(preview.lock, lock);
        for _ in 0..64 {
            assert_eq!(worker.next_stereo(), (0.0, 0.0));
        }
    }

    #[test]
    fn midi_note_on_and_off_in_one_batch_still_triggers_audition() {
        let (mut midi, receiver) = MidiControls::new();
        let (_tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut worker =
            RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, Some(receiver));
        let key = MidiKey {
            channel: 0,
            note: 60,
        };
        midi.inject(MidiEvent::NoteOn { key, velocity: 100 });
        midi.inject(MidiEvent::NoteOff(key));
        worker.drain_commands();
        assert!(worker.midi_preview[0].unwrap().key.is_none());
        let energy: f32 = (0..256)
            .map(|_| {
                let (left, right) = worker.next_stereo();
                left.abs() + right.abs()
            })
            .sum();
        assert!(energy > 0.1);
    }

    #[test]
    fn midi_continue_preserves_sample_position_and_start_resets_it() {
        let (mut midi, receiver) = MidiControls::new();
        let (mut tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(16);
        let mut worker =
            RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, Some(receiver));
        assert!(tx
            .push(AudioCommand::SetPlaying {
                playing: true,
                request: 7
            })
            .is_ok());
        worker.drain_commands();
        for _ in 0..480 {
            worker.next_stereo();
        }
        while tel_rx.pop().is_ok() {}
        midi.inject(MidiEvent::Stop);
        worker.drain_commands();
        for _ in 0..160 {
            worker.next_stereo();
        }
        let stopped = tel_rx.pop().unwrap();
        assert!(!stopped.playing);
        assert_eq!(stopped.play_request, 7);

        midi.inject(MidiEvent::Continue);
        worker.drain_commands();
        worker.next_stereo();
        let resumed = tel_rx.pop().unwrap();
        assert!(resumed.playing);
        assert_eq!(
            resumed.cv.beat,
            480.0 * EngineConfig::default().bpm / (60.0 * 48_000.0)
        );

        midi.inject(MidiEvent::Start);
        worker.drain_commands();
        worker.next_stereo();
        let started = tel_rx.pop().unwrap();
        assert!(started.playing);
        assert_eq!((started.cv.beat, started.cv.row), (0.0, 0));
    }
}
