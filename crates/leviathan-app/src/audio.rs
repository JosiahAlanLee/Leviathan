use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use leviathan_core::rack_routing::{ByteRoute, RackRouting};
use leviathan_core::{
    AutomationRow, CvFrame, EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain,
    SongPlayer, SongStatus, SoundLock, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{
    sampler::{SampleBank, SampleRegion},
    echo, otis_kit, BenderParams, DelayMode, LaneChannel, LanePatch, MasterPeaks, SynthRack,
};
use leviathan_gpu::{AudioScopeSnapshot, AudioScopeTap};

use crate::audition;
use crate::capture::{Recorder, Stage, Take, TakeBank};
use crate::kit::{Kit, KitBank, KitSample};
use crate::midi::{MidiEvent, MidiKey, MidiReceiver, MidiTarget, MIDI_QUEUE_CAPACITY};
use crate::project::{
    BbusSettings, NoteGateSettings, OtisSettings, OversampleSettings, Project, PATTERN_BANK_CAPACITY,
};
use crate::shader_bus;

pub enum AudioCommand {
    SetPlaying {
        playing: bool,
        request: u64,
    },
    PlayFromRow {
        pattern: usize,
        row: usize,
        request: u64,
    },
    SetFill(bool),
    SetBpm(f64),
    SetMasterSend(f32),
    SetDelayMode(DelayMode),
    SetBenderParams(BenderParams),
    SetEchoParams(echo::Params),
    Audition(Arc<audition::Request>),
    SetKit(Arc<Kit>),
    SetKitControls {
        params: [otis_kit::Slot; otis_kit::SLOTS],
        lanes: [otis_kit::Lane; NUM_LANES],
    },
    SetLaneAudibleMask(u8),
    SetLaneChannels([LaneChannel; NUM_LANES]),
    SetLanePatch { lane: usize, patch: LanePatch },
    SetVoiceCalibration(bool),
    SetNoteGates(NoteGateSettings),
    SetBbus(BbusSettings),
    SetOtis(OtisSettings),
    SetOversample(OversampleSettings),
    SetRackRouting(RackRouting),
    SetShaderRoute(shader_bus::Route),
    ShaderReply(shader_bus::Reply),
    ClearShaderField,
    RecordShaderTake(Arc<shader_bus::Recorder>),
    FinishShaderTake,
    InstallTake(Arc<Take>),
    ArmCapture,
    CancelCapture,
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
    SetMidiClockFollow(bool),
}

impl AudioCommand {
    pub fn apply_transport(&self, player: &mut SongPlayer, patterns: &[Pattern]) {
        let pattern_count = patterns.len();
        match self {
            Self::SetPlaying { playing, .. } => player.evaluator.playing = *playing,
            Self::PlayFromRow { pattern, row, .. } => {
                player.start_pattern_at_row(patterns, *pattern, *row);
            }
            Self::SetFill(fill) => player.evaluator.fill_active = *fill,
            Self::SetBpm(bpm) => player.evaluator.config.bpm = bpm.clamp(40.0, 240.0),
            Self::SetNoteGates(gates) => player.evaluator.set_retrigger_held_notes(gates.enabled),
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
    voice_calibration: bool,
    note_gates: NoteGateSettings,
    bbus: BbusSettings,
    otis: OtisSettings,
    oversample: OversampleSettings,
    routing: RackRouting,
    shader_route: shader_bus::Route,
    take: Option<Arc<Take>>,
    kit: Kit,
    echo: echo::Params,
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
            voice_calibration: project.voice_calibration,
            note_gates: project.instruments.note_gates,
            bbus: project.instruments.bbus,
            otis: project.instruments.otis,
            oversample: project.instruments.oversample,
            routing: project.instruments.routing,
            shader_route: project.instruments.shader_bus.route,
            take: project.instruments.take.clone(),
            kit: project.instruments.kit.clone(),
            echo: project.instruments.echo,
        }
    }

    fn player(&self, sample_rate: f64) -> SongPlayer {
        let count = self.patterns.len().min(PATTERN_BANK_CAPACITY);
        let mut player = SongPlayer::new(EngineConfig {
            bpm: self.bpm,
            sample_rate,
            ..EngineConfig::default()
        });
        player.evaluator.set_retrigger_held_notes(self.note_gates.enabled);
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
    Take(Arc<Take>),
    KitSample(Arc<KitSample>),
    Kit(Arc<Kit>),
    Audition(Arc<audition::Request>),
    ShaderTake(Arc<shader_bus::Recorder>),
}

impl RetainedSnapshot {
    fn in_use(&self) -> bool {
        match self {
            Self::Pattern(pattern) => Arc::strong_count(pattern) > 1,
            Self::Project(project) => Arc::strong_count(project) > 1,
            Self::Sample(sample) => Arc::strong_count(sample) > 1,
            Self::Take(take) => Arc::strong_count(take) > 1,
            Self::KitSample(sample) => Arc::strong_count(sample) > 1,
            Self::Kit(kit) => Arc::strong_count(kit) > 1,
            Self::Audition(audition) => Arc::strong_count(audition) > 1,
            Self::ShaderTake(take) => Arc::strong_count(take) > 1,
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
        let take = match &command {
            AudioCommand::InstallTake(take) => Some(take),
            AudioCommand::ReplaceProject(project) => project.take.as_ref(),
            _ => None,
        };
        if let Some(take) = take {
            if !self.retained.iter().any(|item| {
                matches!(item, RetainedSnapshot::Take(retained) if Arc::ptr_eq(retained, take))
            }) {
                self.retained.push(RetainedSnapshot::Take(Arc::clone(take)));
            }
        }
        let kit = match &command {
            AudioCommand::SetKit(kit) => Some(kit.as_ref()),
            AudioCommand::ReplaceProject(project) => Some(&project.kit),
            _ => None,
        };
        if let Some(kit) = kit {
            for sample in kit.samples.iter().flatten() {
                if !self.retained.iter().any(|item| {
                    matches!(item, RetainedSnapshot::KitSample(held) if Arc::ptr_eq(held, sample))
                }) {
                    self.retained.push(RetainedSnapshot::KitSample(Arc::clone(sample)));
                }
            }
        }
        let snapshot = match &command {
            AudioCommand::SetPattern { pattern, .. } => {
                Some(RetainedSnapshot::Pattern(Arc::clone(pattern)))
            }
            AudioCommand::SetKit(kit) => Some(RetainedSnapshot::Kit(Arc::clone(kit))),
            AudioCommand::Audition(request) => Some(RetainedSnapshot::Audition(Arc::clone(request))),
            AudioCommand::RecordShaderTake(take) => Some(RetainedSnapshot::ShaderTake(Arc::clone(take))),
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
    pub capture_stage: Stage,
    pub capture_beats_left: Option<f64>,
    pub transport_position: (f64, usize),
    pub routed_byte: Option<u8>,
    pub echo_timing: echo::Timing,
    pub kit_sounding: [u8; NUM_LANES],
    /// Tempo of an incoming MIDI clock while it is followed and running.
    pub midi_clock_bpm: Option<f32>,
    pub shader: shader_bus::Status,
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
    takes: TakeBank,
    kit: KitBank,
    echo_storage: Box<[[f32; 2]]>,
    audition: audition::Player,
    recorder: Option<Recorder>,
    last_playing_beat: Option<f64>,
    shader: shader_bus::Clock,
    shader_lookup: ByteRoute,
    /// Stamped MIDI events waiting for their sample within this block.
    pending_midi: Box<[(u32, MidiEvent)]>,
    pending_len: usize,
    pending_next: usize,
    frame_in_block: u32,
    last_block_at: Option<Instant>,
    sample_clock: u64,
    clock: MidiClock,
}

/// Follows an external MIDI clock: tempo from the tick interval, phase locked
/// to the ticks counted since Start or Continue.
struct MidiClock {
    follow: bool,
    last_tick: Option<u64>,
    interval: Option<f64>,
    ticks: Option<u64>,
    base_beat: f64,
    active_until: u64,
    armed: Option<MidiEvent>,
    bpm: Option<f32>,
}

impl Default for MidiClock {
    fn default() -> Self {
        Self {
            follow: true,
            last_tick: None,
            interval: None,
            ticks: None,
            base_beat: 0.0,
            active_until: 0,
            armed: None,
            bpm: None,
        }
    }
}

impl MidiClock {
    fn active(&self, now: u64) -> bool {
        self.follow && now < self.active_until
    }
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
            takes: TakeBank::default(),
            kit: KitBank::default(),
            echo_storage: vec![[0.0; 2]; echo::capacity(sample_rate as f32)].into_boxed_slice(),
            audition: audition::Player::default(),
            recorder: None,
            last_playing_beat: None,
            shader: shader_bus::Clock::default(),
            shader_lookup: ByteRoute::default(),
            pending_midi: vec![(0, MidiEvent::Stop); MIDI_QUEUE_CAPACITY].into_boxed_slice(),
            pending_len: 0,
            pending_next: 0,
            frame_in_block: 0,
            last_block_at: None,
            sample_clock: 0,
            clock: MidiClock::default(),
        }
    }

    /// Drain commands and MIDI before a block of `frames`, called at `now`.
    /// Stamped MIDI events are placed at their offset from the previous
    /// block's start: a constant one-block latency instead of jitter.
    fn begin_block(&mut self, frames: usize, now: Instant) {
        self.drain_commands_inner(Some((frames, now)));
        self.last_block_at = Some(now);
    }

    #[cfg(test)]
    fn drain_commands(&mut self) {
        self.drain_commands_inner(None);
    }

    fn drain_commands_inner(&mut self, block: Option<(usize, Instant)>) {
        for _ in 0..self.cmd_rx.slots() {
            let Ok(command) = self.cmd_rx.pop() else {
                break;
            };
            command.apply_transport(&mut self.player, &self.patterns[..self.pattern_count]);
            match command {
                AudioCommand::SetMasterSend(send) => self.synth.set_master_send(send),
                AudioCommand::SetDelayMode(mode) => self.synth.set_delay_mode(mode),
                AudioCommand::SetBenderParams(params) => self.synth.set_bender_params(params),
                AudioCommand::SetEchoParams(params) => self.synth.set_echo_params(params),
                AudioCommand::Audition(request) => self.audition.start(Some(request)),
                AudioCommand::SetBpm(bpm) => self.synth.set_tempo(bpm),
                AudioCommand::SetKit(kit) => {
                    self.synth.set_kit_controls(kit.params, kit.lanes);
                    self.kit.install(&kit, self.synth.kit_generations());
                }
                AudioCommand::SetKitControls { params, lanes } => {
                    self.synth.set_kit_controls(params, lanes);
                }
                AudioCommand::SetLaneAudibleMask(mask) => self.synth.set_lane_audible_mask(mask),
                AudioCommand::SetLaneChannels(channels) => self.synth.set_lane_channels(channels),
                AudioCommand::SetLanePatch { lane, patch } => self.synth.set_lane_patch(lane, patch),
                AudioCommand::SetVoiceCalibration(enabled) => {
                    self.synth.set_voice_calibration(enabled);
                }
                AudioCommand::SetNoteGates(gates) => {
                    self.synth.set_note_release_lanes(gates.release_lanes());
                }
                AudioCommand::SetBbus(bbus) => self.synth.set_bbus_grit_lanes(bbus.grit_lanes()),
                AudioCommand::SetOtis(otis) => self.synth.set_otis_warm_tables(otis.warm_tables),
                AudioCommand::SetOversample(o) => self.synth.set_alias_grit_lanes(o.grit_lanes()),
                AudioCommand::SetRackRouting(routing) => {
                    if self.synth.set_routing(routing) {
                        self.shader_lookup = routing.bytebus;
                    }
                }
                AudioCommand::SetShaderRoute(route) => {
                    self.shader.set_route(route);
                    self.tel_divider = 255;
                }
                AudioCommand::ShaderReply(reply) => self.shader.reply(reply),
                AudioCommand::ClearShaderField => {
                    self.shader.clear_field();
                    self.tel_divider = 255;
                }
                AudioCommand::RecordShaderTake(take) => self.shader.record(take),
                AudioCommand::FinishShaderTake
                | AudioCommand::Launch(_)
                | AudioCommand::SetChain(_) => {
                    self.shader.finish_recording();
                }
                AudioCommand::InstallTake(take) => {
                    self.takes
                        .install(Some(take), self.synth.capture_generations());
                }
                AudioCommand::ArmCapture => {
                    if let Some(recorder) = &mut self.recorder {
                        recorder.arm(self.generation);
                    }
                }
                AudioCommand::Reset => {
                    self.cancel_capture();
                    self.last_playing_beat = None;
                    self.synth.reset_transport_clocks();
                    self.shader.reset_transport();
                    self.tel_divider = 255;
                }
                AudioCommand::CancelCapture => self.cancel_capture(),
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
                    self.audition.start(None);
                    for (dest, source) in self.patterns.iter_mut().zip(&project.patterns) {
                        dest.clone_from(source);
                    }
                    self.pattern_count = project.patterns.len().min(self.patterns.len());
                    self.synth.sample_bank.clone_from(&project.sample_bank);
                    self.synth.set_lane_audible_mask(project.audible_mask);
                    self.synth.set_lane_channels(project.lane_channels);
                    self.synth.set_lane_patches(project.lane_patches);
                    self.synth.set_voice_calibration(project.voice_calibration);
                    self.synth.set_note_release_lanes(project.note_gates.release_lanes());
                    self.synth.set_bbus_grit_lanes(project.bbus.grit_lanes());
                    self.synth.set_otis_warm_tables(project.otis.warm_tables);
                    self.synth.set_alias_grit_lanes(project.oversample.grit_lanes());
                    self.synth.set_routing(project.routing);
                    self.shader.reset_transport();
                    self.shader.set_route(project.shader_route);
                    self.shader_lookup = project.routing.bytebus;
                    self.synth.reset_voices();
                    self.synth.reset_byte_route();
                    self.takes
                        .install(project.take.clone(), self.synth.capture_generations());
                    self.cancel_capture();
                    self.last_playing_beat = None;
                    self.synth.set_delay_mode(project.delay_mode);
                    self.synth.set_kit_controls(project.kit.params, project.kit.lanes);
                    self.kit.install(&project.kit, self.synth.kit_generations());
                    self.synth.set_bender_params(project.bender_params);
                    self.synth.set_echo_params(project.echo);
                    self.synth.set_tempo(project.bpm);
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
                AudioCommand::SetPlaying { request, playing } => {
                    self.play_request = request;
                    self.tel_divider = 255;
                    if !playing {
                        self.cancel_capture();
                        self.last_playing_beat = None;
                        self.shader.finish_recording();
                    }
                }
                AudioCommand::PlayFromRow {
                    pattern, request, ..
                } if pattern < self.pattern_count => {
                    self.play_request = request;
                    self.cancel_capture();
                    self.last_playing_beat = None;
                    self.synth.reset_transport_clocks();
                    self.shader.reset_transport();
                    self.tel_divider = 255;
                }
                AudioCommand::SetMidiTarget(target) => self.midi_target = target,
                AudioCommand::SetMidiClockFollow(follow) => {
                    self.clock.follow = follow;
                    if !follow {
                        self.clock = MidiClock { follow, ..MidiClock::default() };
                    }
                }
                _ => {}
            }
        }
        self.drain_midi(block);
        self.takes.release_unused(self.synth.capture_generations());
        self.kit.release_unused(self.synth.kit_generations());
        if let Some(recorder) = &mut self.recorder {
            recorder.flush();
        }
    }

    fn cancel_capture(&mut self) {
        if let Some(recorder) = &mut self.recorder {
            recorder.cancel();
        }
    }

    fn drain_midi(&mut self, block: Option<(usize, Instant)>) {
        let Some(events) = &mut self.midi_events else {
            return;
        };
        let (release, count) = events.begin_batch();
        if release {
            self.release_midi(None);
        }
        // Events left from the last block (none in normal running) play now.
        while self.pending_next < self.pending_len {
            let event = self.pending_midi[self.pending_next].1;
            self.pending_next += 1;
            self.handle_midi(event);
        }
        self.pending_len = 0;
        self.pending_next = 0;
        self.frame_in_block = 0;
        let rate = self.player.evaluator.config.sample_rate;
        for _ in 0..count {
            let Some((event, at)) = self.midi_events.as_mut().and_then(MidiReceiver::pop_timed)
            else {
                continue;
            };
            let offset = match (block, at, self.last_block_at) {
                (Some((frames, _)), Some(at), Some(previous)) if frames > 0 => {
                    let seconds = at.saturating_duration_since(previous).as_secs_f64();
                    ((seconds * rate).round() as usize).min(frames - 1) as u32
                }
                _ => 0,
            };
            if offset == 0 || self.pending_len == self.pending_midi.len() {
                self.handle_midi(event);
            } else {
                self.pending_midi[self.pending_len] = (offset, event);
                self.pending_len += 1;
            }
        }
    }

    /// Apply events whose sample in this block has come.
    #[inline]
    fn apply_due_midi(&mut self) {
        while self.pending_next < self.pending_len
            && self.pending_midi[self.pending_next].0 <= self.frame_in_block
        {
            let event = self.pending_midi[self.pending_next].1;
            self.pending_next += 1;
            self.handle_midi(event);
        }
        self.frame_in_block = self.frame_in_block.saturating_add(1);
    }

    fn handle_midi(&mut self, event: MidiEvent) {
        {
            match Some(event) {
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
                Some(MidiEvent::Clock) => self.on_clock(),
                Some(event @ (MidiEvent::Start | MidiEvent::Continue))
                    if self.clock.active(self.sample_clock) =>
                {
                    // Start on the next tick, which marks the downbeat.
                    if event == MidiEvent::Start {
                        self.player.evaluator.playing = false;
                    }
                    self.clock.armed = Some(event);
                }
                Some(event) => {
                    if event == MidiEvent::Stop {
                        self.clock.armed = None;
                        self.clock.ticks = None;
                    }
                    if event.apply_transport(&mut self.player, self.pattern_count) {
                        if matches!(event, MidiEvent::Start | MidiEvent::Stop) {
                            self.cancel_capture();
                            self.last_playing_beat = None;
                            self.shader.finish_recording();
                        }
                        if event == MidiEvent::Start {
                            self.synth.reset_transport_clocks();
                            self.shader.reset_transport();
                        }
                        self.tel_divider = 255;
                    }
                }
                None => {}
            }
        }
    }

    fn on_clock(&mut self) {
        let now = self.sample_clock;
        let rate = self.player.evaluator.config.sample_rate;
        if let Some(last) = self.clock.last_tick {
            let dt = now.saturating_sub(last) as f64;
            // 24 ticks per beat between 20 and 400 bpm; anything else restarts the estimate.
            let bpm = 60.0 * rate / (24.0 * dt.max(1.0));
            if (20.0..=400.0).contains(&bpm) {
                let interval = self.clock.interval.map_or(dt, |i| i + 0.1 * (dt - i));
                self.clock.interval = Some(interval);
            } else {
                self.clock.interval = None;
            }
        }
        self.clock.last_tick = Some(now);
        self.clock.active_until = now + (0.5 * rate) as u64;
        if !self.clock.follow {
            return;
        }
        if let Some(armed) = self.clock.armed.take() {
            let start = armed == MidiEvent::Start;
            if start {
                armed.apply_transport(&mut self.player, self.pattern_count);
                self.cancel_capture();
                self.last_playing_beat = None;
                self.synth.reset_transport_clocks();
                self.shader.reset_transport();
            } else {
                self.player.evaluator.playing = true;
            }
            self.clock.base_beat = self.player.cursor_position(&self.patterns[..self.pattern_count]).0;
            self.clock.ticks = Some(0);
            self.tel_divider = 255;
        } else if let Some(ticks) = &mut self.clock.ticks {
            if self.player.evaluator.playing {
                *ticks += 1;
            }
        }
        let Some(interval) = self.clock.interval else { return };
        let tempo = 60.0 * rate / (24.0 * interval);
        self.clock.bpm = Some(tempo as f32);
        let mut bpm = tempo;
        if let (true, Some(ticks)) = (self.player.evaluator.playing, self.clock.ticks) {
            // Pull the transport toward the tick count: half the phase
            // error per beat, at most 5 % of the tempo.
            let expected = self.clock.base_beat + ticks as f64 / 24.0;
            let actual = self.player.cursor_position(&self.patterns[..self.pattern_count]).0;
            bpm *= 1.0 + (0.5 * (expected - actual)).clamp(-0.05, 0.05);
        }
        let bpm = bpm.clamp(40.0, 240.0);
        self.player.evaluator.config.bpm = bpm;
        self.synth.set_tempo(bpm);
    }

    fn release_midi(&mut self, key: Option<MidiKey>) {
        for preview in self.midi_preview.iter_mut().flatten() {
            if key.is_none() || preview.key == key {
                preview.key = None;
            }
        }
    }

    fn midi_frame(&mut self, mut cv: CvFrame, retrigger: bool) -> CvFrame {
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
        cv
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        if self.pending_next < self.pending_len {
            self.apply_due_midi();
        } else {
            self.frame_in_block = self.frame_in_block.saturating_add(1);
        }
        self.sample_clock += 1;
        let (cv, retrigger) = self
            .player
            .step_sample(&self.patterns[..self.pattern_count]);
        let render_cv = if self.midi_preview.iter().any(Option::is_some) {
            self.midi_frame(cv, retrigger)
        } else {
            if retrigger {
                self.synth.retrigger_gates();
            }
            cv
        };
        let tap = self.recorder.as_mut().and_then(|recorder| {
            recorder.tap(
                cv.beat,
                self.last_playing_beat,
                self.player.evaluator.playing,
                self.player.evaluator.config.sample_rate as u32,
            )
        });
        self.last_playing_beat = self.player.evaluator.playing.then_some(cv.beat);
        let shader_edge =
            self.shader
                .step(&cv, self.player.evaluator.config.ppqn, self.shader_lookup);
        let mut samples = self.takes.samples();
        samples.kit = Some(&self.kit);
        samples.ot2_frame = self.shader.frame();
        let (pair, capture) = self.synth.render_session_with_echo(
            &render_cv, &samples, tap, &mut self.echo_storage,
        );
        if let Some(recorder) = self.recorder.as_mut().filter(|_| tap.is_some()) {
            recorder.push(capture);
        }
        let audition = self.audition.step(self.player.evaluator.config.sample_rate);
        let pair = if audition == [0.0; 2] { pair } else {
            ((pair.0 + audition[0]).clamp(-1.0, 1.0), (pair.1 + audition[1]).clamp(-1.0, 1.0))
        };
        self.scope_tap.push_stereo(pair.0, pair.1);
        self.tel_divider = self.tel_divider.wrapping_add(1);
        if shader_edge
            || retrigger
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
                capture_stage: self.recorder.as_ref().map_or(Stage::Idle, Recorder::stage),
                capture_beats_left: self
                    .recorder
                    .as_ref()
                    .and_then(|recorder| recorder.beats_left(cv.beat)),
                transport_position: if self.player.evaluator.playing {
                    (cv.beat, cv.row)
                } else {
                    self.player
                        .cursor_position(&self.patterns[..self.pattern_count])
                },
                routed_byte: self.synth.routed_byte(),
                echo_timing: self.synth.echo_timing(),
                kit_sounding: self.synth.kit_sounding(),
                midi_clock_bpm: self.clock.bpm.filter(|_| self.clock.active(self.sample_clock)),
                shader: self.shader.status(),
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

const PREFERRED_BLOCK_FRAMES: u32 = 256;

fn build_stream<T: cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    slot: &Arc<Mutex<Option<RtAudioWorker>>>,
    channels: usize,
    convert: fn(f32) -> T,
    silence: T,
) -> Result<cpal::Stream, String> {
    let slot = Arc::clone(slot);
    let mut worker: Option<RtAudioWorker> = None;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                if worker.is_none() {
                    worker = slot.try_lock().ok().and_then(|mut held| held.take());
                }
                let Some(worker) = worker.as_mut() else {
                    data.fill(silence);
                    return;
                };
                worker.begin_block(data.len() / channels, Instant::now());
                for frame in data.chunks_mut(channels) {
                    let (l, r) = worker.next_stereo();
                    frame[0] = convert(l);
                    if channels > 1 {
                        frame[1] = convert(r);
                    }
                    for extra in frame.iter_mut().skip(2) {
                        *extra = silence;
                    }
                }
            },
            |err| eprintln!("leviathan audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Stream init failed: {e}"))
}

pub fn try_start_audio(
    initial_pattern: Pattern,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<AudioTelemetry>,
    midi_events: MidiReceiver,
    recorder: Recorder,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        return Err("Offline UI clock (no audio device)".to_string());
    };
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Audio config unavailable: {e}"))?;

    let sample_format = supported.sample_format();
    // Ask for 256-frame blocks (about 5 ms at 48 kHz) where the device allows
    // it: MIDI plays one block late but on its exact sample, so a short block
    // keeps that latency small. Fall back to the device's default block.
    let fixed = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            (*min..=*max).contains(&PREFERRED_BLOCK_FRAMES).then_some(PREFERRED_BLOCK_FRAMES)
        }
        cpal::SupportedBufferSize::Unknown => Some(PREFERRED_BLOCK_FRAMES),
    };
    let mut config: cpal::StreamConfig = supported.into();
    let sample_rate = config.sample_rate as f64;
    let channels = (config.channels as usize).max(1);

    let mut worker = RtAudioWorker::new(
        initial_pattern,
        sample_rate,
        cmd_rx,
        tel_tx,
        Some(midi_events),
    );
    worker.recorder = Some(recorder);
    // The callback takes the worker on its first run, so a stream that fails
    // to build leaves it here for the fallback attempt.
    let slot = Arc::new(Mutex::new(Some(worker)));

    let build = |config: &cpal::StreamConfig| -> Result<cpal::Stream, String> {
        match sample_format {
            cpal::SampleFormat::F32 => build_stream(&device, config, &slot, channels, |v| v, 0.0f32),
            cpal::SampleFormat::I16 => build_stream(
                &device, config, &slot, channels,
                |v| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16, 0i16,
            ),
            cpal::SampleFormat::U16 => build_stream(
                &device, config, &slot, channels,
                |v| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16, u16::MAX / 2,
            ),
            other => Err(format!("Unsupported sample format: {other:?}")),
        }
    };
    let mut block = "device default".to_string();
    let stream = match fixed {
        Some(frames) => {
            config.buffer_size = cpal::BufferSize::Fixed(frames);
            match build(&config) {
                Ok(stream) => {
                    block = format!("{frames}-frame blocks");
                    stream
                }
                Err(_) => {
                    config.buffer_size = cpal::BufferSize::Default;
                    build(&config)?
                }
            }
        }
        None => build(&config)?,
    };

    stream
        .play()
        .map_err(|e| format!("Audio play failed: {e}"))?;
    Ok((
        stream,
        format!("cpal {:.0} Hz ({} ch, {block})", sample_rate, channels),
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
    fn captures_four_beats_in_stereo_then_loads_and_exports_the_same_take() {
        use leviathan_core::rack_routing::{ByteLookup, LaneInput};
        use leviathan_dsp::{default_lane_patches, CaptureTap, MachineKind};

        let mut pattern = Pattern {
            length: 4,
            ..Pattern::default()
        };
        pattern.lanes[0][0] = NoteCell {
            active: true,
            midi: 60,
            velocity: 127,
            ..NoteCell::default()
        };
        let mut lane_mix = LaneMix::default();
        lane_mix.channels[0].0.pan = -1.0;
        let mut project = Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_mix,
            lane_patches: default_lane_patches(),
            patterns: vec![pattern],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        };
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(8);
        let (mut port, recorder) = Recorder::channel();
        let mut control = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        worker.recorder = Some(recorder);
        control.send(AudioCommand::ReplaceProject(Arc::new(
            AudioProject::prepare(&project, 3),
        )));
        assert!(port.request(3, CaptureTap::Lane(0), 4));
        control.send(AudioCommand::ArmCapture);
        control.send(AudioCommand::SetPlaying {
            playing: true,
            request: 1,
        });
        for frame in 0..=96_000 {
            if frame % 257 == 0 {
                worker.drain_commands();
            }
            worker.next_stereo();
        }
        let result = port.poll().unwrap();
        assert_eq!(result.project, 3);
        assert!(matches!(
            result.outcome,
            crate::capture::Outcome::Complete(48_000)
        ));
        assert_eq!(result.frames.len(), 96_000);
        assert!(result
            .frames
            .iter()
            .any(|frame| frame[0].unsigned_abs() > 1000));
        assert!(result.frames.iter().all(|frame| frame[1] == 0));
        assert!(result.frames[72_000..]
            .iter()
            .any(|frame| frame[0].unsigned_abs() > 1000));

        let take = Take::new(48_000, result.frames).unwrap();
        project.patterns[0].length = 16;
        for lane in [1, 2, 4, 5] {
            project.patterns[0].lanes[lane][0] = project.patterns[0].lanes[0][0];
        }
        for (lane, machine) in [
            (0, MachineKind::VastFunSynth),
            (1, MachineKind::ByteBusR2r),
            (2, MachineKind::OtisTranswave),
            (3, MachineKind::VastFunSynth),
            (4, MachineKind::VarSampler),
            (5, MachineKind::OtisTwin),
        ] {
            project.lane_patches[lane] = LanePatch::for_machine(machine);
        }
        project.instruments.routing.inputs[0] = LaneInput::Capture;
        project.instruments.routing.inputs[2] = LaneInput::Capture;
        project.instruments.routing.inputs[3] = LaneInput::Lane(4);
        project.instruments.routing.inputs[4] = LaneInput::Capture;
        project.instruments.routing.inputs[5] = LaneInput::Capture;
        project.instruments.routing.frame_size = 1024;
        project.instruments.routing.frame_bytebus = true;
        project.instruments.routing.bytebus.enabled = true;
        project.instruments.routing.bytebus.lookup = ByteLookup::Invert;
        project.patterns[0].automation[4].byte_override = true;
        project.instruments.take = Some(take);

        let dir = std::env::temp_dir().join(format!("leviathan-wave7-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("capture.leviathan.json");
        let routing = project.instruments.routing;
        crate::project::save(&path, project).unwrap();
        let project = crate::project::load(&path).unwrap();
        assert_eq!(project.instruments.routing, routing);
        assert_eq!(
            project.instruments.take.as_ref().unwrap().frames.len(),
            96_000
        );
        let live = render_test_project_live(&project, 96_000, 96_000);

        let destination = dir.join("stems");
        ExportJob::start(
            project,
            false,
            ExportRequest {
                settings: ExportSettings {
                    encoding: WavEncoding::Float32,
                    stems: true,
                    tail_seconds: 0.0,
                    ..ExportSettings::default()
                },
                destination: destination.clone(),
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let master = std::fs::read(destination.join("master.wav")).unwrap();
        let start = master.len() - live.len() * 8;
        assert_eq!(&master[start - 8..start - 4], b"data");
        let read = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap());
        for (index, frame) in master[start..].chunks_exact(8).enumerate() {
            assert_eq!(
                live[index],
                (read(&frame[..4]), read(&frame[4..])),
                "frame {index}"
            );
        }
        for name in ["0-vast", "1-bbus", "2-otis", "3-vast", "4-smpl", "5-otis2"] {
            let stem = std::fs::read(destination.join(format!("{name}.wav"))).unwrap();
            assert_eq!(stem.len(), master.len());
            assert!(
                stem[start..]
                    .chunks_exact(4)
                    .any(|sample| read(sample).abs() > 0.001),
                "{name}"
            );
            if name == "4-smpl" {
                assert!(stem[start + 72_000 * 8..]
                    .chunks_exact(4)
                    .any(|sample| read(sample).abs() > 0.001));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

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
                voice_calibration: false,
                instruments: Default::default(),
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
                voice_calibration: false,
                instruments: Default::default(),
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
        render_test_project_live_at(project, 48_000.0, music_frames, frames)
    }

    fn render_test_project_live_at(
        project: &Project,
        rate: f64,
        music_frames: usize,
        frames: usize,
    ) -> Vec<(f32, f32)> {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(16);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), rate, rx, tel_tx, None);
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
    fn shader_take_replays_the_clocked_ot2_performance_in_chain_export() {
        use crate::export::{ExportJob, ExportRequest, ExportSettings, ExportSource};
        use crate::project;
        use crate::wav::WavEncoding;
        use leviathan_core::rack_routing::LaneInput;
        use leviathan_dsp::MachineKind;

        let cc = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        let app = crate::LeviathanApp::with_audio(&cc, |_, _, _, _, _| Err("Offline".into()));
        let directory =
            std::env::temp_dir().join(format!("leviathan-shader-take-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let song_path = directory.join("return.leviathan.json");

        for from_capture in [false, true] {
            let mut song = app.project_snapshot();
            song.bpm = 120.0;
            let mut phrase = Pattern {
                length: 4,
                ..Pattern::default()
            };
            phrase.lanes[0][..4].fill(NoteCell {
                active: true,
                midi: 51,
                velocity: 120,
                gate_fraction: 1.0,
                lock: SoundLock {
                    machine: MachineKind::OtisTwin as i16,
                    ..Default::default()
                },
                ..Default::default()
            });
            song.patterns = vec![phrase.clone(), phrase];
            assert!(song.chain.push(0));
            assert!(song.chain.push(1));
            if from_capture {
                let pcm = (0..512)
                    .map(|i| {
                        let sign = if i < 256 { 1 } else { -1 };
                        let value = sign * if i % 256 < 128 { 8000 } else { -8000 };
                        [value; 2]
                    })
                    .collect();
                song.instruments.take = Take::new(48_000, pcm);
                song.instruments.routing.inputs[0] = LaneInput::Capture;
            }

            let frames = 48_000;
            let dry = render_test_project_live(&song, 24_000, 24_000);
            song.instruments.shader_bus.route.lane = Some(0);
            let (tx, rx) = rtrb::RingBuffer::new(16);
            let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(32);
            let mut audio = AudioControl::new(tx);
            let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
            let recording = Arc::new(shader_bus::Recorder::new(LaunchTarget::Chain, 0));
            audio.send(AudioCommand::ReplaceProject(Arc::new(
                AudioProject::prepare(&song, 1),
            )));
            audio.send(AudioCommand::Launch(LaunchTarget::Chain));
            audio.send(AudioCommand::Reset);
            audio.send(AudioCommand::RecordShaderTake(recording.clone()));
            audio.send(AudioCommand::SetPlaying {
                playing: true,
                request: 1,
            });
            worker.drain_commands();
            let mut responded = None;
            let mut live = Vec::with_capacity(frames);
            for sample in 0..frames {
                live.push(worker.next_stereo());
                if sample % 113 == 0 {
                    if let Some(request) = worker
                        .shader
                        .status()
                        .request
                        .filter(|&r| Some(r) != responded)
                    {
                        responded = Some(request);
                        if request.pulse != 2 {
                            let word = if request.pulse % 2 == 0 { 0 } else { 255 };
                            audio.send(AudioCommand::ShaderReply(shader_bus::Reply {
                                request,
                                word,
                                rotation: 0,
                            }));
                        }
                        worker.drain_commands();
                    }
                }
            }
            audio.send(AudioCommand::FinishShaderTake);
            worker.drain_commands();
            assert!(recording.is_done());
            let take = recording.snapshot().unwrap();
            assert_eq!(take.words, [256, 0, 255, 255, 255, 0, 255, 0]);
            assert!(live[6000..24_000]
                .iter()
                .zip(&dry[6000..])
                .any(|(a, b)| (a.0 - b.0).abs() > 1e-4));
            song.instruments.shader_bus.take = Some(take.clone());
            project::save(&song_path, song).unwrap();
            let reopened = project::load(&song_path).unwrap();
            assert_eq!(reopened.instruments.shader_bus.take.as_ref(), Some(&take));

            let destination = directory.join(if from_capture { "capture" } else { "rom" });
            ExportJob::start(
                reopened,
                false,
                ExportRequest {
                    settings: ExportSettings {
                        source: ExportSource::Chain,
                        encoding: WavEncoding::Float32,
                        stems: true,
                        tail_seconds: 0.0,
                        ..Default::default()
                    },
                    destination: destination.clone(),
                },
            )
            .unwrap()
            .finish()
            .unwrap();
            let master = std::fs::read(destination.join("master.wav")).unwrap();
            let start = master.len() - frames * 8;
            assert_eq!(&master[start - 8..start - 4], b"data");
            for (index, frame) in master[start..].chunks_exact(8).enumerate() {
                let left = f32::from_le_bytes(frame[..4].try_into().unwrap());
                let right = f32::from_le_bytes(frame[4..].try_into().unwrap());
                assert_eq!(
                    (left, right),
                    live[index],
                    "capture {from_capture}, sample {index}"
                );
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn followed_midi_restart_rejects_shader_results_from_the_previous_run() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(8);
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        audio.send(AudioCommand::SetShaderRoute(shader_bus::Route {
            lane: Some(0),
            feedback: false,
        }));
        audio.send(AudioCommand::SetPlaying { playing: true, request: 1 });
        worker.drain_commands();
        worker.next_stereo();
        let old = worker.shader.status().request.unwrap();

        worker.handle_midi(MidiEvent::Clock);
        worker.handle_midi(MidiEvent::Start);
        assert!(!worker.player.evaluator.playing);
        worker.handle_midi(MidiEvent::Clock);
        worker.next_stereo();
        let fresh = worker.shader.status().request.unwrap();
        assert_eq!(fresh.pulse, 0);
        assert_ne!(fresh.epoch, old.epoch);
        for (request, word) in [(old, 255), (fresh, 0)] {
            audio.send(AudioCommand::ShaderReply(shader_bus::Reply { request, word, rotation: 0 }));
        }
        worker.drain_commands();
        for _ in 0..5_999 {
            worker.next_stereo();
        }
        assert_eq!(worker.shader.frame(), Some((0, 0)));
    }

    #[test]
    fn note_gate_settings_survive_reload_and_match_live_and_wav() {
        use crate::project;
        use leviathan_dsp::MachineKind;

        let ctx = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        let app = crate::LeviathanApp::with_audio(&ctx, |_, _, _, _, _| Err("Offline".into()));
        let mut source = app.project_snapshot();
        source.bpm = 120.0;
        source.lane_patches[0] = LanePatch::for_machine(MachineKind::SubBass);
        source.patterns[0] = Pattern {
            length: 2,
            ..Pattern::default()
        };
        let note = NoteCell {
            active: true,
            midi: 48,
            velocity: 64,
            gate_fraction: 1.0,
            ..NoteCell::default()
        };
        source.patterns[0].lanes[0][0] = note;
        source.patterns[0].lanes[0][1] = NoteCell { velocity: 115, ..note };

        let dir = std::env::temp_dir().join(format!("leviathan-note-gates-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("song.leviathan.json");
        source.instruments.note_gates = NoteGateSettings::default();
        project::save(&path, source).unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let music = 12_000;
        let frames = music + 2_400;
        let mut loaded = project::load(&path).unwrap();
        assert_eq!(loaded.instruments.note_gates, NoteGateSettings::default());
        let legacy = render_test_project_live(&loaded, music, frames);
        file["version"] = 1.into();
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let old_v1 = project::load(&path).unwrap();
        assert_eq!(old_v1.instruments.note_gates, NoteGateSettings::default());
        assert_eq!(render_test_project_live(&old_v1, music, frames), legacy);

        let mut gates = NoteGateSettings {
            enabled: true,
            ..Default::default()
        };
        gates.one_shot[2] = true;
        loaded.instruments.note_gates = gates;
        project::save(&path, loaded).unwrap();
        let loaded = project::load(&path).unwrap();
        assert_eq!(loaded.instruments.note_gates, gates);
        let live = render_test_project_live(&loaded, music, frames);
        assert_eq!(live[..6_000], legacy[..6_000]);
        assert_ne!(live[6_000], legacy[6_000]);
        let late_energy = |pcm: &[(f32, f32)]| {
            pcm[music + 1_600..]
                .iter()
                .map(|&(left, right)| left * left + right * right)
                .sum::<f32>()
        };
        assert!(late_energy(&live) < late_energy(&legacy) * 0.01);

        let output = dir.join("song.wav");
        ExportJob::start(
            loaded,
            false,
            ExportRequest {
                settings: ExportSettings {
                    tail_seconds: 0.05,
                    encoding: WavEncoding::Float32,
                    ..ExportSettings::default()
                },
                destination: output.clone(),
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let wav = std::fs::read(output).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        let data = wav.len() - frames * 8;
        assert_eq!(&wav[data - 8..data - 4], b"data");
        let read = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap());
        for (frame, &expected) in wav[data..].chunks_exact(8).zip(&live) {
            assert_eq!((read(&frame[..4]), read(&frame[4..])), expected);
        }
    }

    #[test]
    fn kit_and_echo_match_live_and_export_after_project_reload() {
        use crate::export::{ExportJob, ExportRequest, ExportSettings};
        use crate::wav::WavEncoding;
        use crate::project;
        use leviathan_dsp::{default_lane_patches, esp, MachineKind};

        let mut pattern = Pattern { length: 8, rate: PlaybackRate::EightX, ..Pattern::default() };
        for row in 0..8 {
            pattern.lanes[0][row] = NoteCell { active: true, midi: 60 + row as u8, velocity: 127, ..NoteCell::default() };
        }
        let mut lane_patches = default_lane_patches();
        lane_patches[0] = LanePatch::for_machine(MachineKind::OtisKit);
        let mut project = Project {
            bpm: 120.0, master_send: 0.0, delay_mode: Default::default(), bender_params: Default::default(),
            lane_mix: LaneMix::default(), lane_patches, patterns: vec![pattern], selected_pattern: 0,
            chain: PatternChain::default(), samples: Default::default(), sample_regions: Default::default(),
            custom_shader: None, voice_calibration: true, instruments: Default::default(),
        };
        project.lane_mix.channels[0].0.fx_send = 0.85;
        project.instruments.kit.load_factory();
        project.instruments.kit.params[1].reverse = true;
        project.instruments.echo = echo::Params {
            echo_mix: 0.75, space_mix: 0.25, time: 0.52, feedback: 0.9, ..echo::Params::default()
        };
        let dir = std::env::temp_dir().join(format!("leviathan-w8-parity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kit.leviathan.json");
        let mut energies = Vec::new();
        for (index, mode) in esp::Mode::ALL.into_iter().enumerate() {
            project.instruments.echo.esp = mode;
            let live = render_test_project_live(&project, 6000, 15_600);
            project::save(&path, project).unwrap();
            project = project::load(&path).unwrap();
            let source = project::load(&path).unwrap();
            let destination = dir.join(format!("bounce-{index}"));
            ExportJob::start(source, false, ExportRequest {
                settings: ExportSettings { tail_seconds: 0.2, stems: true, encoding: WavEncoding::Float32, ..ExportSettings::default() },
                destination: destination.clone(),
            }).unwrap().finish().unwrap();
            let decode = |bytes: &[u8]| f32::from_le_bytes(bytes.try_into().unwrap());
            let master = std::fs::read(destination.join("master.wav")).unwrap();
            let read = |frame: &[u8]| (decode(&frame[..4]), decode(&frame[4..]));
            let start = master.len() - live.len() * 8;
            assert_eq!(&master[start - 8..start - 4], b"data");
            let frames = master[start..].chunks_exact(8);
            assert_eq!(frames.len(), live.len());
            for (frame, expected) in frames.zip(live.iter()) {
                assert_eq!(read(frame), *expected, "{mode:?}");
            }
            let kit = std::fs::read(destination.join("0-okit.wav")).unwrap();
            assert!(kit[start..].chunks_exact(4).any(|frame| decode(frame).abs() > 0.01));
            let wet = std::fs::read(destination.join("bend.wav")).unwrap();
            let energy: f64 = wet[start..].chunks_exact(4).map(|frame| f64::from(decode(frame)).powi(2)).sum();
            assert!(energy > 0.1);
            energies.push(energy);
        }
        assert!(energies[1..].iter().all(|energy| (energy - energies[0]).abs() > 1e-4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn calibrated_project_defaults_and_locks_match_live_and_wav() {
        use leviathan_dsp::{default_lane_patches, MachineKind};

        let mut pattern = Pattern {
            length: 2,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        pattern.lanes[0][0] = NoteCell {
            active: true,
            midi: 60,
            velocity: 118,
            ..NoteCell::default()
        };
        pattern.lanes[0][1] = NoteCell {
            lock: SoundLock {
                mask: 1 << 7,
                values: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.72],
                ..SoundLock::default()
            },
            ..pattern.lanes[0][0]
        };
        let mut lane_patches = default_lane_patches();
        lane_patches[0] = LanePatch::for_machine_with_calibration(MachineKind::FourOpFm);
        let mut project = Project {
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_mix: LaneMix::default(),
            lane_patches,
            patterns: vec![pattern],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        };
        let legacy = render_test_project_live(&project, 1_500, 1_980);

        let dir = std::env::temp_dir().join(format!("leviathan-calibration-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.leviathan.json");
        project.voice_calibration = true;
        crate::project::save(&path, project).unwrap();
        let project = crate::project::load(&path).unwrap();
        assert!(project.voice_calibration);
        assert_eq!(project.lane_patches, lane_patches);
        assert_eq!(project.patterns[0].lanes[0][1].lock.values[7], 0.72);
        let live = render_test_project_live(&project, 1_500, 1_980);
        assert_ne!(live, legacy);

        let destination = dir.join("calibrated.wav");
        ExportJob::start(
            project,
            false,
            ExportRequest {
                destination: destination.clone(),
                settings: ExportSettings {
                    encoding: WavEncoding::Float32,
                    tail_seconds: 0.01,
                    ..ExportSettings::default()
                },
            },
        )
        .unwrap()
        .finish()
        .unwrap();
        let master = std::fs::read(destination).unwrap();
        std::fs::remove_dir_all(dir).unwrap();

        let data_start = master.len() - live.len() * 8;
        assert_eq!(&master[data_start - 8..data_start - 4], b"data");
        for (n, frame) in master[data_start..].chunks_exact(8).enumerate() {
            let left = f32::from_le_bytes(frame[..4].try_into().unwrap());
            let right = f32::from_le_bytes(frame[4..].try_into().unwrap());
            assert_eq!(live[n], (left, right), "frame {n}");
        }
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
            voice_calibration: false,
            instruments: Default::default(),
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
            voice_calibration: false,
            instruments: Default::default(),
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
            voice_calibration: false,
            instruments: Default::default(),
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
            voice_calibration: false,
            instruments: Default::default(),
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
            voice_calibration: false,
            instruments: Default::default(),
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

    #[test]
    fn play_from_row_reports_the_seek_and_cancels_an_active_capture() {
        let (tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, mut tel_rx) = rtrb::RingBuffer::new(8);
        let (mut port, recorder) = Recorder::channel();
        let mut audio = AudioControl::new(tx);
        let mut worker = RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, None);
        worker.recorder = Some(recorder);
        assert!(port.request(0, leviathan_dsp::CaptureTap::Bend, 4));
        audio.send(AudioCommand::ArmCapture);
        audio.send(AudioCommand::SetPlaying {
            playing: true,
            request: 1,
        });
        worker.drain_commands();
        worker.next_stereo();
        let capture = tel_rx.pop().unwrap();
        assert_eq!(capture.capture_stage, Stage::Recording);
        assert_eq!(capture.capture_beats_left, Some(4.0));

        audio.send(AudioCommand::PlayFromRow {
            pattern: 0,
            row: 12,
            request: 2,
        });
        worker.drain_commands();
        worker.next_stereo();
        let seek = tel_rx.pop().unwrap();
        assert_eq!(seek.play_request, 2);
        assert!(seek.playing);
        assert_eq!(seek.transport_position, (3.0, 12));
        assert_eq!(seek.capture_stage, Stage::Idle);
        assert!(matches!(
            port.poll().unwrap().outcome,
            crate::capture::Outcome::Cancelled
        ));
    }

    #[test]
    fn every_voice_option_survives_save_and_reload_sample_for_sample() {
        use crate::project;
        use leviathan_dsp::otis_kit::{FilterTaper, VelocityCurve};
        use leviathan_dsp::MachineKind;

        let ctx = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        let app = crate::LeviathanApp::with_audio(&ctx, |_, _, _, _, _| Err("Offline".into()));
        let machines = [
            MachineKind::ByteBusR2r,
            MachineKind::OtisTranswave,
            MachineKind::Fs77FormantFm,
            MachineKind::FourOpFm,
            MachineKind::OtisKit,
            MachineKind::AcidLead,
        ];
        let dir = std::env::temp_dir().join(format!("leviathan-options-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut seed = 0x2545_f491_u32;
        let mut coin = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed & 1 == 1
        };
        for round in 0..6 {
            let mut source = app.project_snapshot();
            source.bpm = 140.0;
            source.voice_calibration = coin();
            source.patterns[0] = Pattern { length: 2, ..Pattern::default() };
            for (lane, machine) in machines.into_iter().enumerate() {
                source.lane_patches[lane] = if source.voice_calibration {
                    LanePatch::for_machine_with_calibration(machine)
                } else {
                    LanePatch::for_machine(machine)
                };
                for row in [0, 3, 6, 9] {
                    source.patterns[0].lanes[lane][row] = NoteCell {
                        active: true,
                        midi: 48 + (row as u8 * 5 + lane as u8 * 3) % 24,
                        velocity: 70 + row as u8 * 5,
                        gate_fraction: if row == 3 { 1.0 } else { 0.5 },
                        ..NoteCell::default()
                    };
                }
            }
            let instruments = &mut source.instruments;
            instruments.kit.load_factory();
            instruments.bbus.cycle_lock = coin();
            instruments.bbus.grit[0] = coin();
            instruments.otis.warm_tables = coin();
            instruments.oversample.enabled = coin();
            instruments.oversample.grit[2] = coin();
            instruments.note_gates.enabled = coin();
            let kit_lane = &mut instruments.kit.lanes[4];
            kit_lane.raw_output = coin();
            kit_lane.filter_taper = if coin() { FilterTaper::Squared } else { FilterTaper::Linear };
            kit_lane.velocity = if coin() { VelocityCurve::Soft } else { VelocityCurve::Chip };
            kit_lane.clock = round as u8 % 3;

            let (music, frames) = (24_000, 30_000);
            let before = render_test_project_live(&source, music, frames);
            let path = dir.join(format!("round-{round}.leviathan.json"));
            let expected = (
                source.voice_calibration,
                source.instruments.bbus,
                source.instruments.otis,
                source.instruments.oversample,
                source.instruments.note_gates,
                source.instruments.kit.lanes,
            );
            project::save(&path, source).unwrap();
            let loaded = project::load(&path).unwrap();
            assert_eq!(
                (
                    loaded.voice_calibration,
                    loaded.instruments.bbus,
                    loaded.instruments.otis,
                    loaded.instruments.oversample,
                    loaded.instruments.note_gates,
                    loaded.instruments.kit.lanes,
                ),
                expected,
                "round {round}"
            );
            let after = render_test_project_live(&loaded, music, frames);
            assert!(before.iter().any(|&(l, r)| l != 0.0 || r != 0.0), "round {round} is silent");
            assert_eq!(before, after, "round {round}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn midi_worker() -> (RtAudioWorker, MidiControls) {
        let (midi, events) = MidiControls::new();
        let (_tx, rx) = rtrb::RingBuffer::new(8);
        let (tel_tx, _tel_rx) = rtrb::RingBuffer::new(4);
        (RtAudioWorker::new(Pattern::default(), 48_000.0, rx, tel_tx, Some(events)), midi)
    }

    #[test]
    fn stamped_midi_notes_start_on_their_sample_one_block_later() {
        use std::time::Duration;
        let key = MidiKey { channel: 0, note: 48 };
        for (delay_ms, expected) in [(0.0, 0usize), (2.0, 96), (3.5, 168), (5.0, 240)] {
            let (mut worker, mut midi) = midi_worker();
            let t0 = Instant::now();
            worker.begin_block(256, t0);
            for _ in 0..256 {
                worker.next_stereo();
            }
            midi.inject_at(
                MidiEvent::NoteOn { key, velocity: 120 },
                t0 + Duration::from_secs_f64(delay_ms / 1000.0),
            );
            worker.begin_block(256, t0 + Duration::from_secs_f64(256.0 / 48_000.0));
            let first = (0..256).position(|_| worker.next_stereo() != (0.0, 0.0));
            let first = first.expect("the note sounds within the block");
            assert!(
                (expected..=expected + 2).contains(&first),
                "{delay_ms} ms: first sound at frame {first}, expected {expected}"
            );
        }
        // Unstamped events (tests, injected UI events) play at the block start.
        let (mut worker, mut midi) = midi_worker();
        worker.begin_block(256, Instant::now());
        midi.inject(MidiEvent::NoteOn { key, velocity: 120 });
        worker.begin_block(256, Instant::now());
        assert!(worker.next_stereo() != (0.0, 0.0) || worker.next_stereo() != (0.0, 0.0));
    }

    #[test]
    fn midi_clock_sets_tempo_and_holds_phase_from_the_first_tick() {
        use std::time::Duration;
        let (mut worker, mut midi) = midi_worker();
        let block = 256usize;
        let block_seconds = block as f64 / 48_000.0;
        let bpm = 130.0;
        let tick = 60.0 / (bpm * 24.0);
        let t0 = Instant::now();
        let at = |seconds: f64| t0 + Duration::from_secs_f64(seconds);
        // Clock runs for half a second before Start, as gear does when stopped.
        let mut next_tick = 0.0;
        let start_at = 0.5 + 0.3 * tick;
        let mut started = false;
        let mut ticks_after_start = 0u64;
        let mut first_tick_frame = None;
        for k in 0..(12.0 / block_seconds) as usize {
            let now = k as f64 * block_seconds;
            while next_tick < now {
                if !started && next_tick > start_at {
                    midi.inject_at(MidiEvent::Start, at(start_at));
                    started = true;
                }
                midi.inject_at(MidiEvent::Clock, at(next_tick));
                if started {
                    ticks_after_start += 1;
                }
                next_tick += tick;
            }
            worker.begin_block(block, at(now));
            for frame in 0..block {
                let was_playing = worker.player.evaluator.playing;
                worker.next_stereo();
                if !was_playing && worker.player.evaluator.playing && first_tick_frame.is_none() {
                    first_tick_frame = Some(k * block + frame);
                }
            }
            if started && ticks_after_start == 0 {
                assert!(!worker.player.evaluator.playing, "Start waits for the next tick");
            }
        }
        // Start armed until the first tick after it, which plays one block late on its sample.
        let first_tick_time = (start_at / tick).ceil() * tick;
        let expected_frame = ((first_tick_time + block_seconds) * 48_000.0).round() as usize;
        let got = first_tick_frame.expect("transport started");
        assert!(got.abs_diff(expected_frame) <= 2, "started at {got}, expected {expected_frame}");
        let tempo = worker.clock.bpm.expect("tempo estimated");
        assert!((f64::from(tempo) - bpm).abs() < 0.05, "tempo {tempo}");
        // Phase: the transport sits on the tick count, within a 96th of a beat.
        let beat = worker.player.cursor_position(&worker.patterns[..worker.pattern_count]).0;
        let expected = (ticks_after_start - 1) as f64 / 24.0;
        let since_last_tick = (12.0 - (next_tick - tick) - block_seconds).max(0.0) * bpm / 60.0;
        assert!(
            (beat - (expected + since_last_tick)).abs() < 1.0 / 96.0,
            "beat {beat}, ticks say {}",
            expected + since_last_tick
        );
    }

    #[test]
    fn wav_export_matches_live_playback_at_every_sample_rate() {
        use leviathan_dsp::{echo, MachineKind};

        let ctx = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        let app = crate::LeviathanApp::with_audio(&ctx, |_, _, _, _, _| Err("Offline".into()));
        let make = || {
        let mut project = app.project_snapshot();
        project.bpm = 128.0;
        let machines = [
            MachineKind::ByteBusR2r,
            MachineKind::OtisTranswave,
            MachineKind::Fs77FormantFm,
            MachineKind::FourOpFm,
            MachineKind::OtisKit,
            MachineKind::AcidLead,
        ];
        project.patterns[0] = Pattern { length: 16, ..Pattern::default() };
        for (lane, machine) in machines.into_iter().enumerate() {
            project.lane_patches[lane] = LanePatch::for_machine_with_calibration(machine);
            project.lane_mix.channels[lane].0.delay_send = 0.3;
            project.lane_mix.channels[lane].0.fx_send = 0.4;
            for row in (lane % 3..16).step_by(3) {
                project.patterns[0].lanes[lane][row] = NoteCell {
                    active: true,
                    midi: 45 + ((row * 7 + lane * 5) % 24) as u8,
                    velocity: 90 + (row as u8 % 4) * 9,
                    gate_fraction: if row % 2 == 0 { 0.5 } else { 1.0 },
                    ..NoteCell::default()
                };
            }
        }
        project.instruments.kit.load_factory();
        project.instruments.echo = echo::Params { echo_mix: 0.4, feedback: 0.5, ..echo::Params::default() };
        project.bender_params.feedback = 0.4;
        project
        };

        let dir = std::env::temp_dir().join(format!("leviathan-rate-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for rate in crate::export::SAMPLE_RATES {
            let project = make();
            let destination = dir.join(format!("{rate}.wav"));
            ExportJob::start(
                make(),
                false,
                ExportRequest {
                    destination: destination.clone(),
                    settings: ExportSettings {
                        encoding: WavEncoding::Float32,
                        sample_rate: rate,
                        tail_seconds: 0.01,
                        ..ExportSettings::default()
                    },
                },
            )
            .unwrap()
            .finish()
            .unwrap();
            let wav = std::fs::read(&destination).unwrap();
            let data = wav.windows(4).position(|w| w == b"data").unwrap() + 8;
            let frames = (wav.len() - data) / 8;
            // One bar of music, then the tail; compare the music the live
            // transport renders before it is stopped.
            let music = (4.0 * 60.0 * f64::from(rate) / project.bpm).floor() as usize - 1;
            let live = render_test_project_live_at(&project, f64::from(rate), music, music);
            assert!(frames > music, "{rate}: {frames} frames");
            let mut energy = 0.0f64;
            for (n, frame) in wav[data..data + music * 8].chunks_exact(8).enumerate() {
                let left = f32::from_le_bytes(frame[..4].try_into().unwrap());
                let right = f32::from_le_bytes(frame[4..].try_into().unwrap());
                assert_eq!(live[n], (left, right), "{rate} Hz frame {n}");
                energy += f64::from(left * left + right * right);
            }
            assert!(energy > 1.0, "{rate} Hz export is too quiet to compare");
            // LEVIATHAN_KEEP_EXPORTS=<dir> keeps the WAVs for listening or analysis.
            if let Some(keep) = std::env::var_os("LEVIATHAN_KEEP_EXPORTS") {
                let keep = std::path::PathBuf::from(keep);
                std::fs::create_dir_all(&keep).unwrap();
                std::fs::copy(&destination, keep.join(format!("export-{rate}.wav"))).unwrap();
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
