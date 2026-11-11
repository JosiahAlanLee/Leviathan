use std::collections::VecDeque;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use leviathan_core::{
    AutomationRow, CvFrame, EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain,
    SongPlayer, SongStatus, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{sampler::SampleBank, SynthRack};

use crate::project::{Project, PATTERN_BANK_CAPACITY};

pub enum AudioCommand {
    SetPlaying(bool),
    SetFill(bool),
    SetBpm(f64),
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
}

impl AudioCommand {
    pub fn apply_transport(&self, player: &mut SongPlayer, pattern_count: usize) {
        match self {
            Self::SetPlaying(playing) => player.evaluator.playing = *playing,
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
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: PatternChain,
    sample_bank: SampleBank,
}

impl AudioProject {
    pub fn prepare(project: &Project, generation: u64) -> Self {
        let mut sample_bank = SampleBank::new();
        for (bank, sample) in project.samples.iter().enumerate() {
            if let Some(sample) = sample {
                sample_bank.load_pcm_with_rate(bank, &sample.samples, sample.c4_rate_hz);
            }
        }
        Self {
            generation,
            bpm: project.bpm,
            patterns: project.patterns.clone(),
            selected_pattern: project.selected_pattern,
            chain: project.chain,
            sample_bank,
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
}

struct RtAudioWorker {
    patterns: Box<[Pattern]>,
    pattern_count: usize,
    player: SongPlayer,
    synth: SynthRack,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
    tel_divider: u32,
    generation: u64,
}

impl RtAudioWorker {
    fn new(
        initial_pattern: Pattern,
        sample_rate: f64,
        cmd_rx: rtrb::Consumer<AudioCommand>,
        tel_tx: rtrb::Producer<AudioTelemetry>,
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
            synth: SynthRack::new(sample_rate as f32),
            cmd_rx,
            tel_tx,
            tel_divider: 0,
            generation: 0,
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..self.cmd_rx.slots() {
            let Ok(command) = self.cmd_rx.pop() else {
                break;
            };
            command.apply_transport(&mut self.player, self.pattern_count);
            match command {
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
                    self.synth.reset_voices();
                    self.generation = project.generation;
                    self.tel_divider = 255;
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
                _ => {}
            }
        }
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        let (cv, retrigger) = self
            .player
            .step_sample(&self.patterns[..self.pattern_count]);
        if retrigger {
            self.synth.retrigger_gates();
        }
        let pair = self.synth.render_stereo(&cv);
        self.tel_divider = self.tel_divider.wrapping_add(1);
        if retrigger || (self.tel_divider & 255) == 0 {
            let _ = self.tel_tx.push(AudioTelemetry {
                generation: self.generation,
                cv,
                song: self.player.status(),
            });
        }
        pair
    }
}

pub fn try_start_audio(
    initial_pattern: Pattern,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
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

    let mut worker = RtAudioWorker::new(initial_pattern, sample_rate, cmd_rx, tel_tx);

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
    use crate::wav::{sample_peaks, DecodedWav};
    use leviathan_core::PlaybackRate;

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
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx);

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
            patterns: vec![first, second],
            selected_pattern: 1,
            chain,
            samples,
        };
        audio.send(AudioCommand::SetPlaying(true));
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&project, 1),
        )));
        worker.drain_commands();
        assert!(!worker.player.evaluator.playing);
        assert_eq!(worker.player.status().pattern, 1);
        assert_eq!(&worker.synth.sample_bank.waves[0][..pcm.len()], &pcm);

        audio.send(AudioCommand::Launch(LaunchTarget::Chain));
        audio.send(AudioCommand::SetPlaying(true));
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
        let mut worker = RtAudioWorker::new(Pattern::default(), 100.0, rx, tel_tx);

        let oversized = Project {
            bpm: 120.0,
            patterns: vec![Pattern::default(); PATTERN_BANK_CAPACITY + 4],
            selected_pattern: PATTERN_BANK_CAPACITY + 2,
            chain: PatternChain::default(),
            samples: core::array::from_fn(|_| None),
        };
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&oversized, 2),
        )));
        worker.drain_commands();
        assert_eq!(worker.pattern_count, PATTERN_BANK_CAPACITY);
        assert_eq!(worker.player.status().pattern, PATTERN_BANK_CAPACITY - 1);

        audio.send(AudioCommand::Launch(LaunchTarget::Pattern(0)));
        audio.send(AudioCommand::SetPlaying(true));
        worker.drain_commands();
        for _ in 0..50 {
            worker.next_stereo();
        }
        audio.send(AudioCommand::SetPlaying(false));
        audio.send(AudioCommand::SetPattern {
            index: 0,
            pattern: Arc::new(Pattern {
                length: 4,
                rate: PlaybackRate::FourX,
                ..Pattern::default()
            }),
        });
        audio.send(AudioCommand::SetPlaying(true));
        worker.drain_commands();
        worker.next_stereo();
        assert_eq!(worker.player.status().pattern, 0);
    }

    #[test]
    fn set_automation_updates_live_cv_and_bytebus_telemetry() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 100.0, rx, tel_tx);

        audio.send(AudioCommand::SetAutomation {
            pattern: 0,
            row: 0,
            automation: AutomationRow {
                z: 0.8,
                roll: -0.5,
                feedback: 0.6,
                byte_word: 0b0000_0101,
            },
        });
        audio.send(AudioCommand::SetPlaying(true));
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
}
 leviathan_core::{
    CvFrame, EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain, SongPlayer, SongStatus,
    NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{sampler::SampleBank, SynthRack};

use crate::project::{Project, PATTERN_BANK_CAPACITY};

pub enum AudioCommand {
    SetPlaying(bool),
    SetFill(bool),
    SetBpm(f64),
    Reset,
    Launch(LaunchTarget),
    SetChain(PatternChain),
    SetCell {
        pattern: usize,
        lane: usize,
        row: usize,
        cell: NoteCell,
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
}

impl AudioCommand {
    pub fn apply_transport(&self, player: &mut SongPlayer, pattern_count: usize) {
        match self {
            Self::SetPlaying(playing) => player.evaluator.playing = *playing,
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
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: PatternChain,
    sample_bank: SampleBank,
}

impl AudioProject {
    pub fn prepare(project: &Project, generation: u64) -> Self {
        let mut sample_bank = SampleBank::new();
        for (bank, sample) in project.samples.iter().enumerate() {
            if let Some(sample) = sample {
                sample_bank.load_pcm_with_rate(bank, &sample.samples, sample.c4_rate_hz);
            }
        }
        Self {
            generation,
            bpm: project.bpm,
            patterns: project.patterns.clone(),
            selected_pattern: project.selected_pattern,
            chain: project.chain,
            sample_bank,
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
}

struct RtAudioWorker {
    patterns: Box<[Pattern]>,
    pattern_count: usize,
    player: SongPlayer,
    synth: SynthRack,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
    tel_divider: u32,
    generation: u64,
}

impl RtAudioWorker {
    fn new(
        initial_pattern: Pattern,
        sample_rate: f64,
        cmd_rx: rtrb::Consumer<AudioCommand>,
        tel_tx: rtrb::Producer<AudioTelemetry>,
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
            synth: SynthRack::new(sample_rate as f32),
            cmd_rx,
            tel_tx,
            tel_divider: 0,
            generation: 0,
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..self.cmd_rx.slots() {
            let Ok(command) = self.cmd_rx.pop() else {
                break;
            };
            command.apply_transport(&mut self.player, self.pattern_count);
            match command {
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
                    self.synth.reset_voices();
                    self.generation = project.generation;
                    self.tel_divider = 255;
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
                _ => {}
            }
        }
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        let (cv, retrigger) = self
            .player
            .step_sample(&self.patterns[..self.pattern_count]);
        if retrigger {
            self.synth.retrigger_gates();
        }
        let pair = self.synth.render_stereo(&cv);
        self.tel_divider = self.tel_divider.wrapping_add(1);
        if retrigger || (self.tel_divider & 255) == 0 {
            let _ = self.tel_tx.push(AudioTelemetry {
                generation: self.generation,
                cv,
                song: self.player.status(),
            });
        }
        pair
    }
}

pub fn try_start_audio(
    initial_pattern: Pattern,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
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

    let mut worker = RtAudioWorker::new(initial_pattern, sample_rate, cmd_rx, tel_tx);

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
    use crate::wav::{sample_peaks, DecodedWav};
    use leviathan_core::{AutomationRow, PlaybackRate};

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
        let mut worker = RtAudioWorker::new(Pattern::default(), 8_000.0, rx, tel_tx);

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
            patterns: vec![first, second],
            selected_pattern: 1,
            chain,
            samples,
        };
        audio.send(AudioCommand::SetPlaying(true));
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&project, 1),
        )));
        worker.drain_commands();
        assert!(!worker.player.evaluator.playing);
        assert_eq!(worker.player.status().pattern, 1);
        assert_eq!(&worker.synth.sample_bank.waves[0][..pcm.len()], &pcm);

        audio.send(AudioCommand::Launch(LaunchTarget::Chain));
        audio.send(AudioCommand::SetPlaying(true));
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
        let mut worker = RtAudioWorker::new(Pattern::default(), 100.0, rx, tel_tx);

        let oversized = Project {
            bpm: 120.0,
            patterns: vec![Pattern::default(); PATTERN_BANK_CAPACITY + 4],
            selected_pattern: PATTERN_BANK_CAPACITY + 2,
            chain: PatternChain::default(),
            samples: core::array::from_fn(|_| None),
        };
        audio.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&oversized, 2),
        )));
        worker.drain_commands();
        assert_eq!(worker.pattern_count, PATTERN_BANK_CAPACITY);
        assert_eq!(worker.player.status().pattern, PATTERN_BANK_CAPACITY - 1);

        audio.send(AudioCommand::Launch(LaunchTarget::Pattern(0)));
        audio.send(AudioCommand::SetPlaying(true));
        worker.drain_commands();
        for _ in 0..50 {
            worker.next_stereo();
        }
        audio.send(AudioCommand::SetPlaying(false));
        audio.send(AudioCommand::SetPattern {
            index: 0,
            pattern: Arc::new(Pattern {
                length: 4,
                rate: PlaybackRate::FourX,
                ..Pattern::default()
            }),
        });
        audio.send(AudioCommand::SetPlaying(true));
        worker.drain_commands();
        worker.next_stereo();
        assert_eq!(worker.player.status().pattern, 0);
    }
}
