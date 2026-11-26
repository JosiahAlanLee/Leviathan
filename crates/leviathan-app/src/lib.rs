#[cfg(target_os = "android")]
mod android_files;
mod audio;
mod audition;
mod capture;
mod export;
mod export_ui;
mod gen_ui;
mod gpu_view;
mod inspector_ui;
mod kit;
mod kit_ui;
mod lane_mix;
mod link;
mod locks_ui;
mod master_ui;
mod midi;
mod mixer_ui;
mod param_ui;
mod pcm;
mod project;
mod rack_ui;
mod rework_ui;
mod sampler_ui;
mod session_ui;
mod shader_bus;
mod smf;
#[cfg(any(target_os = "android", test))]
mod tablet_ui;
#[cfg(any(target_os = "android", test))]
mod system_bars;
mod tracker_edit;
mod tracker_keys;
mod visual_sampler;
mod wav;
mod workflow_ui;

use std::sync::Arc;

use eframe::egui;
use export_ui::ExportUi;
use gen_ui::{GenUi, MidiDestination, MidiFileAction};
use gpu_view::FeedbackViewport;
use lane_mix::LaneMix;
use leviathan_core::{
    CvFrame, EngineConfig, FillCondition, LaunchTarget, NoteCell, Pattern, PatternChain, SongPlayer, SongStatus,
    SoundLock, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{default_lane_patches, BenderParams, DelayMode, LanePatch, MachineKind, MasterPeaks};
use link::SoundLinkBridge;
use locks_ui::draw_lane_patch_inspector;
use master_ui::MasterMeters;
use midi::{MidiControls, MidiEvent, MidiReceiver, MidiTarget};

use audio::{try_start_audio, AudioCommand, AudioControl, AudioProject, AudioTelemetry};
use capture::{CapturePort, Outcome, Recorder, Take};
use sampler_ui::{BankAction, SamplerBankUi};
use session_ui::{ArrangementAction, ProjectAction, ProjectUi};
use tracker_edit::EditHistory;
use tracker_keys::{PatternSync, SubColumn, TrackerCursor};
use workflow_ui::{Deck, TransportShortcut};

const OFFLINE_CLOCK_HZ: f64 = 60.0;

fn starter_electro_pattern() -> Pattern {
    let mut pat = Pattern::default();
    let bass_notes: [u8; 8] = [42, 42, 45, 42, 49, 47, 45, 43];

    for row in 0..ROWS_PER_PATTERN {
        let step16 = row % 16;
        let bar = row / 16;

        if matches!(step16, 0 | 3 | 6 | 10 | 14) {
            pat.lanes[0][row] = NoteCell {
                active: true,
                midi: 36,
                velocity: if step16 == 0 || step16 == 10 { 124 } else { 104 },
                gate_fraction: 0.45,
                ..NoteCell::default()
            };
        }

        if matches!(step16, 4 | 12) {
            pat.lanes[1][row] = NoteCell {
                active: true,
                midi: 66,
                velocity: 108,
                gate_fraction: 0.35,
                lock: SoundLock {
                    machine: MachineKind::FourOpFm as i16,
                    mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 5),
                    values: [0.35, 0.46, 0.65, 0.30, 0.45, 0.22, 0.15, 0.10],
                },
                ..NoteCell::default()
            };
        } else if matches!(step16, 9 | 15) && bar % 2 == 1 {
            pat.lanes[1][row] = NoteCell {
                active: true,
                midi: 73,
                velocity: 96,
                gate_fraction: 0.40,
                lock: SoundLock {
                    machine: MachineKind::FourOpFm as i16,
                    mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 6),
                    values: [0.45, 0.82, 0.75, 0.05, 0.55, 0.35, 0.68, 0.20],
                },
                ..NoteCell::default()
            };
        }

        if step16 % 2 == 0 {
            pat.lanes[2][row] = NoteCell {
                active: true,
                midi: bass_notes[step16 / 2],
                velocity: 112,
                gate_fraction: 0.65,
                ..NoteCell::default()
            };
        }

        if step16 % 2 == 1 {
            pat.lanes[3][row] = NoteCell {
                active: true,
                air: true,
                midi: 60,
                velocity: 88,
                gate_fraction: 0.25,
                fill: if step16 >= 13 {
                    FillCondition::WhenFill
                } else {
                    FillCondition::Always
                },
                ..NoteCell::default()
            };
        }

        if step16 == 0 && bar % 2 == 0 {
            pat.lanes[4][row] = NoteCell {
                active: true,
                midi: 54,
                velocity: 108,
                gate_fraction: 0.85,
                lock: SoundLock {
                    machine: MachineKind::VarSampler as i16,
                    mask: (1 << 0) | (1 << 1) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 7),
                    values: [0.75, 0.62, 0.72, 0.60, 0.05, 0.55, 0.45, 0.85],
                },
                ..NoteCell::default()
            };
        } else if step16 == 8 && bar % 2 == 1 {
            pat.lanes[4][row] = NoteCell {
                active: true,
                midi: 49,
                velocity: 116,
                gate_fraction: 0.50,
                lock: SoundLock {
                    machine: MachineKind::VarSampler as i16,
                    mask: (1 << 0) | (1 << 1) | (1 << 3) | (1 << 7),
                    values: [0.50, 0.12, 0.58, 0.05, 0.0, 0.90, 0.0, 0.15],
                },
                ..NoteCell::default()
            };
        }

        if matches!(step16, 2 | 7 | 11 | 15) {
            pat.lanes[5][row] = NoteCell {
                active: true,
                midi: if step16 == 11 { 69 } else { 57 },
                velocity: 100,
                gate_fraction: 0.40,
                probability: if step16 == 15 { 65 } else { 100 },
                ..NoteCell::default()
            };
        }

        let phase = row as f32 / ROWS_PER_PATTERN as f32;
        pat.automation[row].z = 0.25 + 0.35 * (phase * core::f32::consts::TAU).sin().abs();
        pat.automation[row].roll = 0.30 * (phase * core::f32::consts::TAU).cos();
        pat.automation[row].feedback =
            0.45 + 0.30 * (phase * core::f32::consts::PI * 4.0).sin().abs();
    }
    pat
}

fn midi_label(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
    ];
    let note = NAMES[(midi % 12) as usize];
    let oct = (midi as i32 / 12) - 1;
    format!("{note}{oct}")
}

struct LeviathanApp {
    // Stop the callback before dropping the UI's retained command snapshots.
    _audio_stream: Option<cpal::Stream>,
    patterns: Vec<Pattern>,
    histories: Vec<EditHistory>,
    selected_pattern: usize,
    chain: PatternChain,
    project_ui: ProjectUi,
    export_ui: ExportUi,
    gen_ui: GenUi,
    project_generation: u64,
    voice_calibration: bool,
    inspector_drag_recorded: bool,
    deck: Deck,
    edit_lane_patch: bool,
    follow_playback: bool,
    followed_row: Option<(usize, usize)>,
    playing: bool,
    fill_active: bool,
    #[cfg(target_os = "android")]
    touch_fill: tablet_ui::TouchFill,
    #[cfg(any(target_os = "android", test))]
    tablet_scrub: tablet_ui::TabletScrub,
    #[cfg(test)]
    exercise_tablet_layout: bool,
    #[cfg(test)]
    simulated_system_bars: Option<system_bars::Edges>,
    #[cfg(target_os = "android")]
    system_bars: Option<system_bars::SystemBars>,
    #[cfg(target_os = "android")]
    android_files: Option<android_files::Files>,
    bpm: f64,
    master_send: f32,
    delay_mode: DelayMode,
    bender_params: BenderParams,
    lane_mix: LaneMix,
    lane_patches: [LanePatch; NUM_LANES],
    master_meters: MasterMeters,
    sound_link: SoundLinkBridge,
    midi: MidiControls,
    midi_target: MidiTarget,
    play_request: u64,
    applied_play_request: u64,
    cursor: TrackerCursor,
    sampler_ui: SamplerBankUi,
    instruments: project::InstrumentSettings,
    capture_port: CapturePort,
    rack_panel: rack_ui::RackPanel,
    kit_ui: kit_ui::KitUi,
    rework: rework_ui::ReworkUi,
    echo_timing: leviathan_dsp::echo::Timing,
    show_kit: bool,
    show_waveform: bool,
    visual_sampler: visual_sampler::VisualSampler,
    split_undo: Option<(kit::Kit, kit::Kit)>,
    kit_sounding: [u8; NUM_LANES],
    capture_kit_slot: Option<usize>,
    routed_byte: Option<u8>,
    latest_shader: shader_bus::Status,
    shader_recording: Option<Arc<shader_bus::Recorder>>,
    audio: AudioControl,
    tel_rx: rtrb::Consumer<AudioTelemetry>,
    offline_player: Option<SongPlayer>,
    offline_clock: Option<f64>,
    audio_status: String,
    latest_cv: CvFrame,
    latest_scope: leviathan_gpu::AudioScopeSnapshot,
    latest_song: SongStatus,
    transport_position: (f64, usize),
    viewport: FeedbackViewport,
}

impl LeviathanApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(target_os = "android")]
        let mut app = {
            log::info!("Creating tracker with audio disabled");
            let app = Self::with_audio(cc, |_, _, _, _, _| Err("Audio not started".into()));
            log::info!("Tracker constructed");
            app
        };
        #[cfg(not(target_os = "android"))]
        let mut app = Self::with_audio(cc, try_start_audio);
        if cfg!(target_os = "android") {
            app.deck = Deck::Grid;
        } else {
            app.midi.refresh();
        }
        app
    }

    fn with_audio(
        cc: &eframe::CreationContext<'_>,
        start_audio: impl FnOnce(
            Pattern,
            rtrb::Consumer<AudioCommand>,
            rtrb::Producer<AudioTelemetry>,
            MidiReceiver,
            Recorder,
        ) -> Result<(cpal::Stream, String), String>,
    ) -> Self {
        let viewport = if cfg!(target_os = "android") {
            // The first device pass isolates audio and touch from shader feedback.
            FeedbackViewport::new_software()
        } else {
            FeedbackViewport::new(cc)
        };

        let initial_pattern = starter_electro_pattern();
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::<AudioCommand>::new(256);
        let (tel_tx, tel_rx) = rtrb::RingBuffer::<AudioTelemetry>::new(256);
        let (midi, midi_events) = MidiControls::new();
        let (capture_port, recorder) = Recorder::channel();

        let (_audio_stream, offline_player, audio_status) = match start_audio(
            initial_pattern.clone(),
            cmd_rx,
            tel_tx,
            midi_events,
            recorder,
        ) {
            Ok((stream, label)) => (Some(stream), None, label),
            Err(label) => (
                None,
                Some(SongPlayer::new(EngineConfig {
                    sample_rate: OFFLINE_CLOCK_HZ,
                    ..EngineConfig::default()
                })),
                label,
            ),
        };

        let mut cursor = TrackerCursor::default();
        cursor.select(4, 0, SubColumn::Note, false);

        let mut app = Self {
            _audio_stream,
            patterns: vec![initial_pattern],
            histories: vec![EditHistory::default()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            project_ui: ProjectUi::default(),
            export_ui: ExportUi::default(),
            gen_ui: GenUi::default(),
            project_generation: 0,
            // New sessions use calibrated voicing; saved projects keep theirs.
            voice_calibration: true,
            inspector_drag_recorded: false,
            deck: Deck::default(),
            edit_lane_patch: false,
            follow_playback: false,
            followed_row: None,
            playing: false,
            fill_active: false,
            #[cfg(target_os = "android")]
            touch_fill: tablet_ui::TouchFill::default(),
            #[cfg(any(target_os = "android", test))]
            tablet_scrub: tablet_ui::TabletScrub::default(),
            #[cfg(test)]
            exercise_tablet_layout: false,
            #[cfg(test)]
            simulated_system_bars: None,
            #[cfg(target_os = "android")]
            system_bars: None,
            #[cfg(target_os = "android")]
            android_files: None,
            bpm: 132.0,
            master_send: 0.0,
            delay_mode: DelayMode::default(),
            bender_params: BenderParams::default(),
            lane_mix: LaneMix::default(),
            lane_patches: calibrated_lane_patches(),
            master_meters: MasterMeters::default(),
            sound_link: SoundLinkBridge::default(),
            midi,
            midi_target: MidiTarget::default(),
            play_request: 0,
            applied_play_request: 0,
            cursor,
            sampler_ui: SamplerBankUi::default(),
            instruments: project::InstrumentSettings {
                note_gates: project::NoteGateSettings {
                    enabled: true,
                    ..Default::default()
                },
                bbus: project::BbusSettings {
                    cycle_lock: true,
                    ..Default::default()
                },
                otis: project::OtisSettings { warm_tables: true },
                oversample: project::OversampleSettings { enabled: true, ..Default::default() },
                ..Default::default()
            },
            capture_port,
            rack_panel: rack_ui::RackPanel::default(),
            kit_ui: kit_ui::KitUi::default(),
            rework: rework_ui::ReworkUi::default(),
            echo_timing: Default::default(),
            show_kit: false,
            show_waveform: true,
            visual_sampler: visual_sampler::VisualSampler::default(),
            split_undo: None,
            kit_sounding: [0; NUM_LANES],
            capture_kit_slot: None,
            routed_byte: None,
            latest_shader: shader_bus::Status::default(),
            shader_recording: None,
            audio: AudioControl::new(cmd_tx),
            tel_rx,
            offline_player,
            offline_clock: None,
            audio_status,
            latest_cv: CvFrame::default(),
            latest_scope: leviathan_gpu::AudioScopeSnapshot::default(),
            latest_song: SongStatus::default(),
            transport_position: (0.0, 0),
            viewport,
        };
        // The audio engine starts from the rack's legacy defaults: give it
        // the session's calibrated voicing and factory patches.
        app.send_audio(AudioCommand::SetVoiceCalibration(app.voice_calibration));
        for (lane, patch) in app.lane_patches.into_iter().enumerate() {
            app.send_audio(AudioCommand::SetLanePatch { lane, patch });
        }
        app.sync_midi_target();
        app
    }

    #[cfg(target_os = "android")]
    #[inline(never)]
    fn start_diagnostic_audio(&mut self) {
        log::info!("Starting AAudio on request");
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(256);
        let (tel_tx, tel_rx) = rtrb::RingBuffer::new(256);
        let (capture_port, recorder) = Recorder::channel();
        let (_, midi_events) = MidiControls::new();
        match try_start_audio(self.patterns[0].clone(), cmd_rx, tel_tx, midi_events, recorder) {
            Ok((stream, label)) => {
                self._audio_stream = Some(stream);
                self.audio = AudioControl::new(cmd_tx);
                self.tel_rx = tel_rx;
                self.capture_port = capture_port;
                self.offline_player = None;
                self.offline_clock = None;
                self.playing = false;
                self.applied_play_request = self.play_request;
                self.send_audio(AudioCommand::ReplaceProject(Arc::new(AudioProject::prepare(
                    &self.project_snapshot(),
                    self.project_generation,
                ))));
                self.send_audio(AudioCommand::SetMidiTarget(self.midi_target));
                self.send_audio(AudioCommand::SetFill(self.fill_active));
                log::info!("Started {label}");
                self.audio_status = label;
            }
            Err(error) => {
                log::error!("AAudio start failed: {error}");
                self.audio_status = error;
            }
        }
    }

    #[cfg(any(target_os = "android", test))]
    fn draw_touch_notes(&mut self, ui: &mut egui::Ui, pattern: usize, portrait: bool) {
        const KEYS: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
        let enabled = self.cursor.row < self.patterns[pattern].length;
        let octave = |app: &mut Self, ui: &mut egui::Ui| {
            if ui.add(egui::Button::new("Oct −").min_size(egui::vec2(46.0, 36.0))).clicked() {
                app.cursor.base_octave = (app.cursor.base_octave - 1).max(1);
            }
            if ui.add(egui::Button::new("Oct +").min_size(egui::vec2(46.0, 36.0))).clicked() {
                app.cursor.base_octave = (app.cursor.base_octave + 1).min(6);
            }
        };
        let key = |app: &mut Self, ui: &mut egui::Ui, semitone: usize, width: f32| {
            if ui.add_enabled(
                enabled,
                egui::Button::new(KEYS[semitone]).min_size(egui::vec2(width, 44.0)),
            ).clicked() {
                let midi = ((app.cursor.base_octave + 1) * 12 + semitone as i32) as u8;
                app.enter_touch_note(pattern, midi);
            }
        };
        if portrait {
            ui.horizontal(|ui| {
                octave(self, ui);
                ui.label(format!("Octave {}", self.cursor.base_octave));
            });
            let width = ((ui.available_width() - ui.spacing().item_spacing.x * 5.0) / 6.0)
                .max(24.0).floor();
            for offset in [0, 6] {
                ui.horizontal(|ui| {
                    for semitone in offset..offset + 6 {
                        key(self, ui, semitone, width);
                    }
                });
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                octave(self, ui);
                for semitone in 0..KEYS.len() {
                    key(self, ui, semitone, 36.0);
                }
            });
        }
    }

    #[cfg(any(target_os = "android", test))]
    fn enter_touch_note(&mut self, pattern: usize, midi: u8) {
        let length = self.patterns[pattern].length.clamp(1, ROWS_PER_PATTERN);
        if self.cursor.row >= length {
            return;
        }
        let update = self.cursor.enter_note(
            &mut self.patterns[pattern],
            &mut self.histories[pattern],
            midi,
            None,
        );
        self.cursor.row = (update.row + self.cursor.edit_step) % length;
        self.send_audio(AudioCommand::SetCell {
            pattern,
            lane: update.lane,
            row: update.row,
            cell: update.cell,
        });
        // The worker checks the actual transport state at the callback boundary.
        self.send_audio(AudioCommand::PreviewStep {
            lane: update.lane,
            cell: update.cell,
        });
    }

    #[cfg(any(target_os = "android", test))]
    fn draw_tablet_scrub(&mut self, ui: &mut egui::Ui, pattern: usize) {
        let lane = self.cursor.lane;
        let row = self.cursor.row;
        let cell = self.patterns[pattern].lanes[lane][row];
        let patch = self.lane_patches[lane];
        let enabled = row < self.patterns[pattern].length;
        if let Some((mut drag, up)) =
            self.tablet_scrub.draw(ui, (pattern, lane, row), cell, &patch, enabled)
        {
            self.apply_tablet_scrub(pattern, lane, row, &mut drag, up);
            if drag.recorded_undo {
                self.tablet_scrub.mark_edit_recorded();
            }
        }
    }

    #[cfg(any(target_os = "android", test))]
    fn apply_tablet_scrub(
        &mut self,
        pattern: usize,
        lane: usize,
        row: usize,
        drag: &mut tablet_ui::ScrubDrag,
        up: f32,
    ) {
        let cell = drag.cell_at(up, &self.lane_patches[lane]);
        if self.patterns[pattern].lanes[lane][row] == cell {
            return;
        }
        if !drag.recorded_undo {
            self.histories[pattern].record_before_edit(&self.patterns[pattern]);
            drag.recorded_undo = true;
        }
        self.patterns[pattern].lanes[lane][row] = cell;
        self.send_audio(AudioCommand::SetCell { pattern, lane, row, cell });
    }

    fn set_fill(&mut self, active: bool) {
        if self.fill_active != active {
            self.fill_active = active;
            self.send_audio(AudioCommand::SetFill(active));
        }
    }

    fn send_audio(&mut self, command: AudioCommand) {
        if let Some(player) = &mut self.offline_player {
            command.apply_transport(player, &self.patterns);
            self.latest_song = player.status();
            self.transport_position = player.cursor_position(&self.patterns);
            if matches!(
                command,
                AudioCommand::SetPlaying { .. } | AudioCommand::PlayFromRow { .. }
            ) {
                self.offline_clock = None;
                self.applied_play_request = self.play_request;
            }
        } else {
            self.audio.send(command);
        }
    }

    fn toggle_playing(&mut self) {
        self.set_playing(!self.playing);
    }

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.play_request = self.play_request.wrapping_add(1);
        self.send_audio(AudioCommand::SetPlaying {
            playing,
            request: self.play_request,
        });
    }

    fn play_from_cursor(&mut self) {
        self.playing = true;
        self.play_request = self.play_request.wrapping_add(1);
        self.send_audio(AudioCommand::PlayFromRow {
            pattern: self.selected_pattern,
            row: self.cursor.row,
            request: self.play_request,
        });
    }

    fn advance_offline_transport(&mut self, now: f64) {
        let Some(player) = self.offline_player.as_mut() else {
            return;
        };
        if player.evaluator.playing {
            let clock = self
                .offline_clock
                .get_or_insert(now - 1.0 / OFFLINE_CLOCK_HZ);
            let elapsed = ((now - *clock) * OFFLINE_CLOCK_HZ + 1e-7).floor();
            // Bound work after a suspended window. This fallback only clocks the UI.
            let steps = elapsed.clamp(0.0, 15.0) as usize;
            for _ in 0..steps {
                self.latest_cv = player.step_sample(&self.patterns).0;
            }
            *clock = if elapsed > 15.0 {
                now
            } else {
                *clock + steps as f64 / OFFLINE_CLOCK_HZ
            };
            if steps > 0 {
                self.transport_position = (self.latest_cv.beat, self.latest_cv.row);
            }
        } else {
            self.offline_clock = None;
            self.latest_cv = player.step_sample(&self.patterns).0;
            self.transport_position = player.cursor_position(&self.patterns);
        }
        self.latest_song = player.status();
    }

    fn sync_midi_target(&mut self) {
        let target = MidiTarget {
            pattern: self.selected_pattern,
            lane: self.cursor.lane,
            row: self.cursor.row,
        };
        if self.midi_target != target {
            self.midi_target = target;
            self.send_audio(AudioCommand::SetMidiTarget(target));
        }
    }

    fn read_midi(&mut self, ctx: &egui::Context, modal_open: bool) {
        let (_, count) = self.midi.events.begin_batch();
        let record = self.midi.record_armed
            && !modal_open
            && ctx.memory(|memory| memory.top_modal_layer().is_none());
        for _ in 0..count {
            match self.midi.events.pop() {
                Some(MidiEvent::NoteOn { key, velocity }) if record => {
                    let pattern = self.selected_pattern;
                    let update = self.cursor.enter_note(
                        &mut self.patterns[pattern],
                        &mut self.histories[pattern],
                        key.note,
                        Some(velocity),
                    );
                    self.cursor.last_edit_msg = format!("MIDI {}", midi_label(update.cell.midi));
                    self.send_audio(AudioCommand::SetCell {
                        pattern,
                        lane: update.lane,
                        row: update.row,
                        cell: update.cell,
                    });
                }
                Some(event) => {
                    if let Some(player) = &mut self.offline_player {
                        if event.apply_transport(player, self.patterns.len()) {
                            self.playing = player.evaluator.playing;
                        }
                    }
                }
                None => {}
            }
        }
    }

    fn push_full_pattern(&mut self, index: usize) {
        self.send_audio(AudioCommand::SetPattern {
            index,
            pattern: Arc::new(self.patterns[index].clone()),
        });
    }

    fn step_history(&mut self, index: usize, redo: bool) {
        let previous_bpm = self.bpm;
        let history = &mut self.histories[index];
        let pattern = &mut self.patterns[index];
        let changed = if redo {
            history.redo(pattern, &mut self.bpm)
        } else {
            history.undo(pattern, &mut self.bpm)
        };
        if changed {
            self.cursor.last_edit_msg = if redo { "Redo" } else { "Undo" }.to_string();
            self.push_full_pattern(index);
            if self.bpm != previous_bpm {
                self.send_audio(AudioCommand::SetBpm(self.bpm));
            }
        }
    }

    fn import_midi(
        &mut self,
        mut import: smf::Import,
        destination: MidiDestination,
        use_tempo: bool,
    ) -> Result<String, String> {
        let bpm = import.bpm.clamp(40.0, 240.0);
        let tempo_changes = use_tempo && self.bpm != bpm;
        let mut message = match destination {
            MidiDestination::Pattern => {
                let selected = self.selected_pattern;
                let pattern = import.patterns.pop().ok_or("MIDI file has no pattern")?;
                self.histories[selected].record_before_import(
                    &self.patterns[selected],
                    tempo_changes.then_some(self.bpm),
                );
                self.patterns[selected] = pattern;
                self.cursor.anchor = None;
                self.cursor.row = self.cursor.row.min(self.patterns[selected].length - 1);
                self.push_full_pattern(selected);
                format!("Imported pattern {selected:02X}")
            }
            MidiDestination::AppendChain => {
                let start = self.patterns.len();
                let count = import.patterns.len();
                let entries = import.chain.as_slice();
                if start + count > project::PATTERN_BANK_CAPACITY {
                    return Err(format!("MIDI chain needs {count} free pattern slots"));
                }
                if self.chain.as_slice().len() + entries.len() > leviathan_core::MAX_CHAIN_ENTRIES {
                    return Err(format!(
                        "MIDI chain needs {} free chain entries",
                        entries.len()
                    ));
                }
                for pattern in import.patterns {
                    let index = self.patterns.len();
                    self.patterns.push(pattern);
                    self.histories.push(EditHistory::default());
                    self.push_full_pattern(index);
                }
                for &index in entries {
                    self.chain.push((start + usize::from(index)) as u8);
                }
                self.send_audio(AudioCommand::SetChain(self.chain));
                format!(
                    "Appended {} chain entries in {count} patterns",
                    entries.len()
                )
            }
            MidiDestination::OpenChain => {
                let count = import.patterns.len();
                let mut project = self.project_snapshot();
                if use_tempo {
                    project.bpm = bpm;
                }
                project.patterns = import.patterns;
                project.selected_pattern = usize::from(import.chain.as_slice()[0]);
                project.chain = import.chain;
                self.install_project(project);
                self.project_ui = ProjectUi::default();
                format!("Opened MIDI project with {count} patterns")
            }
        };
        self.inspector_drag_recorded = false;
        if use_tempo && self.bpm != bpm {
            self.bpm = bpm;
            self.send_audio(AudioCommand::SetBpm(bpm));
        }
        if use_tempo {
            if bpm != import.bpm {
                message.push_str(&format!(" · file tempo limited to {bpm} BPM"));
            }
            if import.tempo_changes {
                message.push_str(" · used the initial tempo");
            }
        }
        if import.dropped_notes > 0 {
            message.push_str(&format!(
                " · {} notes exceeded six lanes",
                import.dropped_notes
            ));
        }
        Ok(message)
    }

    fn apply_midi_file(&mut self, action: MidiFileAction) {
        let result = match action {
            MidiFileAction::Import {
                path,
                destination,
                rate,
                use_tempo,
            } => smf::load(&path, rate, destination.scope())
                .and_then(|import| self.import_midi(import, destination, use_tempo)),
            MidiFileAction::Export {
                path,
                scope,
                format,
            } => {
                let patterns = match scope {
                    smf::Scope::Pattern => vec![&self.patterns[self.selected_pattern]],
                    smf::Scope::Chain => self
                        .chain
                        .as_slice()
                        .iter()
                        .map(|&index| &self.patterns[usize::from(index)])
                        .collect(),
                };
                smf::save(&path, &patterns, self.bpm, format)
                    .map(|()| format!("Exported {}", path.display()))
            }
        };
        self.gen_ui.finish(result);
    }

    fn select_pattern(&mut self, index: usize) {
        if self.selected_pattern == index {
            return;
        }
        self.selected_pattern = index;
        self.cursor.anchor = None;
        self.cursor.moved_this_frame = true;
        self.inspector_drag_recorded = false;
    }

    fn follow_row(&mut self, ctx: &egui::Context, modal_open: bool) -> Option<usize> {
        if !self.follow_playback || !self.playing {
            self.followed_row = None;
            return None;
        }
        if modal_open
            || self.applied_play_request != self.play_request
            || self.cursor.moved_this_frame
        {
            return None;
        }
        if ctx.wants_keyboard_input()
            || ctx.memory(|m| m.any_popup_open())
            || ctx.input(|i| i.pointer.any_down() || i.pointer.any_released())
        {
            return None;
        }
        let (pattern, row) = (self.latest_song.pattern, self.transport_position.1);
        if pattern >= self.patterns.len() || self.followed_row == Some((pattern, row)) {
            return None;
        }
        self.select_pattern(pattern);
        self.cursor.moved_this_frame = false;
        self.followed_row = Some((pattern, row));
        Some(row)
    }

    fn draw_voice_deck(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.edit_lane_patch, false, "Step Lock");
            ui.selectable_value(&mut self.edit_lane_patch, true, "Lane Patch");
        });
        let (lane, row, pattern) = (self.cursor.lane, self.cursor.row, self.selected_pattern);
        let gates = &mut self.instruments.note_gates;
        let before = *gates;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut gates.enabled, "Use note gates")
                .on_hover_text("Release the original voices at gate-off and give each adjacent step its own attack.");
            if gates.enabled {
                ui.checkbox(&mut gates.one_shot[lane], format!("Lane {} one-shot", lane + 1))
                    .on_hover_text("Let the original voices on this lane finish their full decay.");
            }
        });
        let after = *gates;
        if after != before {
            self.send_audio(AudioCommand::SetNoteGates(after));
        }
        let patch = &self.lane_patches[lane];
        let machine = if self.edit_lane_patch {
            patch.machine
        } else {
            let lock = &self.patterns[pattern].lanes[lane][row].lock;
            MachineKind::from_patch_and_lock(patch, lock)
        };
        if machine == MachineKind::ByteBusR2r {
            let bbus = &mut self.instruments.bbus;
            let before = *bbus;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut bbus.cycle_lock, "BBUS cycle lock · Project")
                    .on_hover_text("Restart the BBUS register every note cycle so it repeats at the note's pitch, like Parallel Sum clocked from an oscillator. Off keeps every lane free-running, as in projects made before this option.");
                if bbus.cycle_lock {
                    ui.checkbox(&mut bbus.grit[lane], format!("BBUS grit · Lane {}", lane + 1))
                        .on_hover_text("Let this lane's BBUS register run free across note cycles: the original, noisier and more digital sound.");
                }
            });
            let after = *bbus;
            if after != before {
                self.send_audio(AudioCommand::SetBbus(after));
            }
        }
        if matches!(
            machine,
            MachineKind::Fs77FormantFm | MachineKind::FourOpFm | MachineKind::AcidLead
        ) {
            let oversample = &mut self.instruments.oversample;
            let before = *oversample;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut oversample.enabled, "Oversample FM and Acid · Project")
                    .on_hover_text("Run FS77 and the 4-Op FM operators at four times the output rate and band-limit Acid's saw, so high notes and deep modulation stop folding back as low, inharmonic tones. Off keeps every lane at the original single rate, as in projects made before this option.");
                if oversample.enabled {
                    ui.checkbox(&mut oversample.grit[lane], format!("FM and Acid grit · Lane {}", lane + 1))
                        .on_hover_text("Keep this lane at the original single rate, with its aliasing.");
                }
            });
            let after = *oversample;
            if after != before {
                self.send_audio(AudioCommand::SetOversample(after));
            }
        }
        if machine == MachineKind::OtisTranswave {
            let otis = &mut self.instruments.otis;
            if ui
                .checkbox(&mut otis.warm_tables, "OTIS warm tables · Project")
                .on_hover_text("TableSel alternates bright and warm: the lower half of each quarter plays the original table, the upper half a darker twin. Off keeps the four bright tables only, as in projects made before this option.")
                .changed()
            {
                let otis = *otis;
                self.send_audio(AudioCommand::SetOtis(otis));
            }
        }
        ui.separator();
        if self.edit_lane_patch {
            if ui
                .checkbox(&mut self.voice_calibration, "Voice calibration")
                .on_hover_text("Use calibrated voice levels and tuning. Switching restarts active voices. Reset a lane patch to load its calibrated defaults.")
                .changed()
            {
                self.send_audio(AudioCommand::SetVoiceCalibration(self.voice_calibration));
            }
            if draw_lane_patch_inspector(
                ui,
                lane,
                &mut self.lane_patches[lane],
                self.voice_calibration,
            ) {
                self.send_audio(AudioCommand::SetLanePatch {
                    lane,
                    patch: self.lane_patches[lane],
                });
            }
        } else if let Some(cell) = inspector_ui::draw_step(
            ui,
            lane,
            row,
            self.patterns[pattern].lanes[lane][row],
            &self.lane_patches[lane],
            self.instruments.kit.lanes[lane].velocity,
        ) {
            self.record_inspector_undo_once(ui.ctx(), pattern);
            self.patterns[pattern].lanes[lane][row] = cell;
            self.send_audio(AudioCommand::SetCell {
                pattern,
                lane,
                row,
                cell,
            });
        }
    }

    fn draw_mixer_deck(&mut self, ui: &mut egui::Ui) {
        mixer_ui::draw(ui, &mut self.lane_mix, &self.lane_patches);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Master delay send");
            let mut percent = self.master_send * 100.0;
            if ui
                .add(
                    egui::DragValue::new(&mut percent)
                        .range(0.0..=100.0)
                        .speed(0.5)
                        .suffix("%"),
                )
                .changed()
                && percent.is_finite()
            {
                self.master_send = (percent / 100.0).clamp(0.0, 1.0);
                self.send_audio(AudioCommand::SetMasterSend(self.master_send));
            }
        });
        if master_ui::draw_delay_mode(ui, &mut self.delay_mode) {
            self.send_audio(AudioCommand::SetDelayMode(self.delay_mode));
        }
        if master_ui::draw_bender(ui, &mut self.bender_params) {
            self.send_audio(AudioCommand::SetBenderParams(self.bender_params));
        }
        if master_ui::draw_echo(ui, &mut self.instruments.echo, self.echo_timing, self.bpm) {
            self.send_audio(AudioCommand::SetEchoParams(self.instruments.echo));
        }
    }

    fn draw_routing_deck(&mut self, ui: &mut egui::Ui) {
        let (lane, row, pattern) = (self.cursor.lane, self.cursor.row, self.selected_pattern);
        let patch = &self.lane_patches[lane];
        let lock = &self.patterns[pattern].lanes[lane][row].lock;
        let step_machine = MachineKind::from_patch_and_lock(patch, lock);
        ui.heading(format!("Lane {} routing", lane + 1));
        ui.label(format!(
            "Patch {} · Step {pattern:02X}/{row:02X} {}",
            patch.machine.short_name(),
            step_machine.short_name(),
        ));
        let allow_lane_source = [patch.machine, step_machine].contains(&MachineKind::VastFunSynth);
        let mut routing = self.instruments.routing;
        rack_ui::draw_routing(
            ui,
            &mut routing,
            lane,
            allow_lane_source,
            self.instruments.take.is_some(),
            self.routed_byte,
            self.instruments.shader_bus.route.lane.is_some(),
            self.rack_panel.routing_error,
        );
        if routing != self.instruments.routing {
            self.rack_panel.routing_error = routing.render_order().is_none();
            if !self.rack_panel.routing_error {
                self.instruments.routing = routing;
                self.send_audio(AudioCommand::SetRackRouting(routing));
            }
        }
        if self.instruments.shader_bus.route.lane == Some(lane as u8) {
            let input = self
                .latest_shader
                .request
                .map_or("—".into(), |req| format!("{:02X}", req.word));
            let byte = self
                .latest_shader
                .latched
                .map_or("—".into(), |reply| format!("{:02X}", reply.word));
            ui.monospace(format!("Shader {input} → {byte} · OT2 FramePos"))
                .on_hover_text("Set the return and its probes in Scene (F6).");
        }
    }

    fn draw_samples_deck(&mut self, ui: &mut egui::Ui, hovering_wav: bool) -> Vec<BankAction> {
        let selected = self.show_kit.then_some(self.kit_ui.slot);
        match self.rack_panel.draw_capture(
            ui,
            self.instruments.take.as_deref(),
            self._audio_stream.is_some(),
            self.capture_port.busy(),
            self.playing,
            if self.capture_port.busy() { self.capture_kit_slot } else { selected },
        ) {
            Some(rack_ui::CaptureAction::Arm(source, beats)) => {
                if self.capture_port.request(self.project_generation, source, beats) {
                    self.capture_kit_slot = selected;
                    self.rack_panel.status = "";
                    self.rack_panel.stage = capture::Stage::Armed;
                    self.rack_panel.beats_left = None;
                    self.send_audio(AudioCommand::ArmCapture);
                }
            }
            Some(rack_ui::CaptureAction::Cancel) => self.send_audio(AudioCommand::CancelCapture),
            None => {}
        }
        ui.separator();
        if self.show_kit {
            let sounding = self.kit_sounding[self.cursor.lane];
            self.kit_ui.sounding = std::array::from_fn(|slot| sounding & (1 << slot) != 0);
            let edits = self.kit_ui.draw(ui, &mut self.instruments.kit, self.cursor.lane,
                self.instruments.take.is_some(), self.capture_port.busy(), self._audio_stream.is_some());
            if edits.samples {
                self.publish_kit();
            } else if edits.controls {
                self.send_audio(AudioCommand::SetKitControls {
                    params: self.instruments.kit.params, lanes: self.instruments.kit.lanes,
                });
            }
            if edits.audition {
                let slot = self.kit_ui.slot;
                if let Some(sample) = self.instruments.kit.samples[slot].clone() {
                    let request = rework_ui::kit_audition(sample, self.instruments.kit.params[slot]);
                    self.send_audio(AudioCommand::Audition(Arc::new(request)));
                }
            }
            if edits.use_capture {
                if let Some(take) = &self.instruments.take {
                    match self.instruments.kit.capture_into(self.kit_ui.slot, take) {
                        Ok(()) => {
                            self.kit_ui.status = "Capture copied into slot".into();
                            self.publish_kit();
                        }
                        Err(error) => self.kit_ui.status = error,
                    }
                }
            }
        }
        let rework = self.rework.draw(
            ui, self.project_generation, &self.instruments, self.kit_ui.slot, self._audio_stream.is_some(),
        );
        if let Some(request) = rework.audition {
            self.send_audio(AudioCommand::Audition(Arc::new(request)));
        }
        if rework.commit && self.rework.commit(self.project_generation, &mut self.instruments) {
            match self.rework.target() {
                rework_ui::Target::Take => {
                    if let Some(take) = &self.instruments.take {
                        self.send_audio(AudioCommand::InstallTake(take.clone()));
                    }
                }
                rework_ui::Target::Kit(_) => self.publish_kit(),
            }
        }
        if self.show_kit {
            Vec::new()
        } else {
            self.sampler_ui.draw(ui, hovering_wav)
        }
    }

    #[cfg(target_os = "android")]
    fn request_android_file(&mut self, kind: android_files::Kind, ctx: &egui::Context) {
        if !self.android_files.as_ref().is_some_and(|files| !files.pending()) { return; }
        let data = if matches!(kind, android_files::Kind::Save) {
            project::encode(self.project_snapshot())
        } else {
            Ok(Vec::new())
        };
        let files = self.android_files.as_mut().unwrap();
        match data {
            Ok(bytes) => files.launch(kind, bytes, ctx),
            Err(error) => files.status = error,
        }
    }

    #[cfg(target_os = "android")]
    fn poll_android_files(&mut self, ctx: &egui::Context) {
        use android_files::{Kind, Reply};
        let Some((kind, reply)) = self.android_files.as_mut().and_then(|files| files.poll(ctx)) else { return };
        let status = match (kind, reply) {
            (Kind::Wav, Reply::File(path, name)) => {
                self.visual_sampler.load(path, Some(name), true, ctx);
                "WAV selected.".into()
            }
            (Kind::Open, Reply::File(path, _)) => {
                let result = project::load(&path);
                let _ = std::fs::remove_file(path);
                match result {
                    Ok(project) => {
                        self.install_project(project);
                        "Opened project.".into()
                    }
                    Err(error) => error,
                }
            }
            (_, Reply::Saved) => "Saved a project copy.".into(),
            (_, Reply::Cancelled) => "Files closed without changes.".into(),
            (_, Reply::Failed(error)) => error,
            (_, Reply::File(path, _)) => {
                let _ = std::fs::remove_file(path);
                "Unexpected file result.".into()
            }
        };
        self.android_files.as_mut().unwrap().status = status;
    }

    fn draw_visual_sampler(&mut self, ui: &mut egui::Ui) {
        let actions = self.visual_sampler.draw(
            ui, &self.instruments.kit, self._audio_stream.is_some(), self.split_undo.is_some(),
        );
        if let Some(request) = actions.audio {
            match request {
                Some(request) => self.send_audio(AudioCommand::Audition(Arc::new(request))),
                None => self.send_audio(AudioCommand::StopAudition),
            }
        }
        #[cfg(target_os = "android")]
        if actions.open {
            self.request_android_file(android_files::Kind::Wav, ui.ctx());
        }
        if let Some(first) = actions.commit {
            self.visual_sampler.status = match self.commit_visual_split(first) {
                Ok(()) => "Slices copied to OtisKit. Select a slot and Enter hit on an OtisKit lane.".into(),
                Err(error) => error,
            };
        }
        if actions.undo {
            self.visual_sampler.status = match self.undo_visual_split() {
                Ok(()) => "Restored kit before the split.".into(),
                Err(error) => error,
            };
        }
        let slot = self.visual_sampler.lane_slot;
        if actions.hear_slot {
            if let Some(sample) = self.instruments.kit.samples[slot].clone() {
                let request = rework_ui::kit_audition(sample, self.instruments.kit.params[slot]);
                self.send_audio(AudioCommand::Audition(Arc::new(request)));
            }
        }
        if actions.enter {
            self.visual_sampler.status = match self.enter_visual_hit(slot) {
                Ok(()) => format!("Entered slot {}.", slot + 1),
                Err(error) => error,
            };
        }
    }

    fn commit_visual_split(&mut self, first: usize) -> Result<(), String> {
        let next = self.visual_sampler.make_kit(&self.instruments.kit, first)?;
        let mut check = self.project_snapshot();
        check.instruments.kit = next.clone();
        // Validate the real serialized budget before modifying UI/audio state.
        project::encode(check)?;
        let previous = std::mem::replace(&mut self.instruments.kit, next.clone());
        self.split_undo = Some((previous, next));
        self.publish_kit();
        Ok(())
    }

    fn undo_visual_split(&mut self) -> Result<(), String> {
        let (before, after) = self.split_undo.as_ref().ok_or("No sampler split to undo.")?;
        let kit = &self.instruments.kit;
        if kit.params != after.params || kit.lanes != after.lanes ||
            !kit.samples.iter().zip(&after.samples).all(|(a, b)|
                a.as_ref().map(|s| s.generation()) == b.as_ref().map(|s| s.generation()))
        {
            self.split_undo = None;
            return Err("The kit changed after the split; undo would discard later edits.".into());
        }
        self.instruments.kit = before.clone();
        self.split_undo = None;
        self.publish_kit();
        Ok(())
    }

    fn enter_visual_hit(&mut self, slot: usize) -> Result<(), String> {
        let (pattern, lane, row) = (self.selected_pattern, self.cursor.lane, self.cursor.row);
        let cells = &mut self.patterns[pattern];
        if row >= cells.length || cells.lanes[lane][row].active {
            return Err("Select an empty step inside the active pattern.".into());
        }
        if self.instruments.kit.samples.get(slot).and_then(Option::as_ref).is_none() {
            return Err("Choose a filled kit slot.".into());
        }
        let patch = self.lane_patches[lane];
        if patch.machine != MachineKind::OtisKit || self.instruments.kit.lanes[lane].bytebus {
            return Err("Select a note-addressed OtisKit lane in Voice / Trig or Grid.".into());
        }
        let config = EngineConfig::default();
        let midi = leviathan_dsp::otis_kit::note_for_slot(
            slot, patch.params[0], config.pitch_zero, config.key_offset,
        ).ok_or("No native-pitch note for the selected slot.")?;
        let cell = NoteCell { active: true, midi, velocity: 100, ..NoteCell::default() };
        self.histories[pattern].record_before_edit(cells);
        cells.lanes[lane][row] = cell;
        let next_row = (row + self.cursor.edit_step) % cells.length;
        self.cursor.select(lane, next_row, SubColumn::Note, false);
        self.cursor.moved_this_frame = true;
        self.send_audio(AudioCommand::SetCell { pattern, lane, row, cell });
        #[cfg(target_os = "android")]
        self.send_audio(AudioCommand::PreviewStep { lane, cell });
        Ok(())
    }

    fn publish_kit(&mut self) {
        self.send_audio(AudioCommand::SetKit(Arc::new(self.instruments.kit.clone())));
    }

    fn draw_scene_deck(
        &mut self,
        ui: &mut egui::Ui,
        preview: egui::TextureId,
        hovering_wgsl: bool,
    ) {
        ui.heading("Feedback");
        let width = ui.available_width();
        let field = ui.image((preview, egui::vec2(width, width * 9.0 / 16.0)));
        if self.instruments.shader_bus.route.lane.is_some() {
            let accepted = self.latest_shader.latched;
            let rotation =
                accepted.map_or(self.instruments.shader_bus.rotation, |reply| reply.rotation);
            let painter = ui.painter_at(field.rect);
            let points = shader_bus::Probe::points(rotation);
            for (bit, ([x, y], label)) in points
                .into_iter()
                .zip(["0", "1", "2", "3", "4", "5", "6", "7"])
                .enumerate()
            {
                let position =
                    field.rect.min + egui::vec2(x * field.rect.width(), y * field.rect.height());
                let lit = accepted.is_some_and(|reply| reply.word & (1 << bit) != 0);
                let fill = if lit {
                    egui::Color32::from_rgb(212, 233, 157)
                } else {
                    egui::Color32::BLACK
                };
                let text = if lit {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::WHITE
                };
                painter.circle(
                    position,
                    8.0,
                    fill,
                    egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
                );
                painter.text(
                    position,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::monospace(10.0),
                    text,
                );
            }
            field.on_hover_text("The numbered probes show the bits of the last latched return.");
        }
        self.draw_shader_bus(ui);
        ui.small(self.viewport.status_label());
        ui.monospace(format!(
            "Z {:.2} V · Roll {:+.2} V · Feedback {:.2} V",
            self.latest_cv.z_v, self.latest_cv.roll_v, self.latest_cv.feedback_v
        ));
        self.viewport.draw_shader_controls(ui, hovering_wgsl);
        ui.separator();
        let (pattern, row) = (self.selected_pattern, self.cursor.row);
        if let Some(automation) =
            inspector_ui::draw_automation(ui, row, self.patterns[pattern].automation[row])
        {
            self.record_inspector_undo_once(ui.ctx(), pattern);
            self.patterns[pattern].automation[row] = automation;
            self.send_audio(AudioCommand::SetAutomation {
                pattern,
                row,
                automation,
            });
        }
    }

    fn draw_shader_bus(&mut self, ui: &mut egui::Ui) {
        let patch = &mut self.instruments.shader_bus;
        let previous = patch.route;
        let route_label = previous
            .lane
            .map_or("Off".into(), |lane| format!("Lane {}", lane + 1));
        egui::ComboBox::from_label("Shader return to OT2")
            .selected_text(route_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut patch.route.lane, None, "Off");
                for (lane, voice) in self.lane_patches.iter().enumerate() {
                    ui.selectable_value(
                        &mut patch.route.lane,
                        Some(lane as u8),
                        format!("Lane {} · {}", lane + 1, voice.machine.short_name()),
                    );
                }
            });
        if patch.route.lane.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::DragValue::new(&mut patch.rotation).range(0..=359).prefix("Probes ").suffix("°"));
                ui.add(egui::DragValue::new(&mut patch.threshold).prefix("Threshold "))
                    .on_hover_text("A probe sets its bit when the sampled field reaches this brightness.");
                ui.checkbox(&mut patch.route.feedback, "Return → Lookup")
                    .on_hover_text("Feed the held return through Lookup into the next shader step. The reset word seeds the loop.");
            });
        }
        let route = patch.route;
        if route != previous {
            if route.lane != previous.lane {
                self.latest_shader = shader_bus::Status::default();
            }
            self.send_audio(AudioCommand::SetShaderRoute(route));
        }
        let Some(lane) = route.lane else { return };
        let input = self
            .latest_shader
            .request
            .map_or("—".into(), |req| format!("{:02X}", req.word));
        let held = self
            .latest_shader
            .latched
            .map_or("—".into(), |reply| format!("{:02X}", reply.word));
        ui.monospace(format!("In {input} · Held {held}"));
        if self.latest_shader.missed > 0 {
            ui.small(format!("{} missed pulses", self.latest_shader.missed));
        }
        if self.offline_player.is_some() {
            ui.small("Shader return uses the audio device clock.");
        }
        if ui.button("Clear field").clicked() {
            self.latest_shader.request = None;
            self.viewport.clear_feedback();
            self.send_audio(AudioCommand::ClearShaderField);
        }
        if let Some(pulses) = self.shader_recording.as_ref().map(|recording| recording.len()) {
            ui.horizontal(|ui| {
                ui.label(format!("Recording · {pulses} pulses"));
                if ui.button("Keep take").clicked() {
                    self.send_audio(AudioCommand::FinishShaderTake);
                }
            });
        } else if ui.add_enabled(self.offline_player.is_none(), egui::Button::new("Record return from start"))
            .on_hover_text("Restart the cued pattern or chain and capture the held byte on each pulse. Pause or keep the take to finish.")
            .clicked()
        {
            self.record_shader_take(lane);
        }
        if let Some(take) = &self.instruments.shader_bus.take {
            ui.small(format!(
                "WAV take · {} · Lane {} · {} pulses",
                take.target.label(),
                take.lane + 1,
                take.words.len()
            ));
        }
    }

    fn record_shader_take(&mut self, lane: u8) {
        let target = if self.latest_song.chain_entry.is_some() && !self.chain.as_slice().is_empty()
        {
            LaunchTarget::Chain
        } else {
            LaunchTarget::Pattern(self.latest_song.pattern.min(self.patterns.len() - 1))
        };
        self.set_playing(false);
        self.send_audio(AudioCommand::Launch(target));
        self.send_audio(AudioCommand::Reset);
        self.latest_shader = shader_bus::Status::default();
        let recording = Arc::new(shader_bus::Recorder::new(target, lane));
        self.shader_recording = Some(Arc::clone(&recording));
        self.send_audio(AudioCommand::RecordShaderTake(recording));
        self.set_playing(true);
    }

    fn record_inspector_undo_once(&mut self, ctx: &egui::Context, selected: usize) {
        if !self.inspector_drag_recorded {
            self.histories[selected].record_before_edit(&self.patterns[selected]);
            if ctx.input(|i| i.pointer.any_down()) {
                self.inspector_drag_recorded = true;
            }
        }
    }

    fn apply_arrangement_action(&mut self, ctx: &egui::Context, action: ArrangementAction) {
        let index = self.selected_pattern;
        match action {
            ArrangementAction::Select(index) => {
                self.follow_playback = false;
                self.select_pattern(index);
            }
            ArrangementAction::AddPattern { clone } => {
                self.follow_playback = false;
                let pattern = if clone {
                    self.patterns[index].clone()
                } else {
                    Pattern::default()
                };
                let next = self.patterns.len();
                self.patterns.push(pattern);
                self.histories.push(EditHistory::default());
                self.push_full_pattern(next);
                self.select_pattern(next);
            }
            ArrangementAction::SetLength(length) => {
                self.record_inspector_undo_once(ctx, index);
                self.patterns[index].length = length;
                self.cursor.row = self.cursor.row.min(length - 1);
                self.push_full_pattern(index);
            }
            ArrangementAction::SetRate(rate) => {
                let pattern = &mut self.patterns[index];
                self.histories[index].record_before_edit(pattern);
                pattern.rate = rate;
                self.push_full_pattern(index);
            }
            ArrangementAction::SetSwing(swing) => {
                self.record_inspector_undo_once(ctx, index);
                self.patterns[index].swing = swing;
                self.push_full_pattern(index);
            }
            ArrangementAction::Launch(target) => self.send_audio(AudioCommand::Launch(target)),
            ArrangementAction::AppendToChain => {
                self.chain.push(index as u8);
                self.send_audio(AudioCommand::SetChain(self.chain));
            }
            ArrangementAction::PopChain => {
                self.chain.pop();
                self.send_audio(AudioCommand::SetChain(self.chain));
            }
            ArrangementAction::ClearChain => {
                self.chain = PatternChain::default();
                self.send_audio(AudioCommand::SetChain(self.chain));
            }
        }
    }

    fn project_snapshot(&self) -> project::Project {
        let mut instruments = self.instruments.clone();
        if let Some(take) = self
            .shader_recording
            .as_ref()
            .and_then(|recording| recording.snapshot())
        {
            instruments.shader_bus.take = Some(take);
        }
        project::Project {
            bpm: self.bpm,
            master_send: self.master_send,
            delay_mode: self.delay_mode,
            bender_params: self.bender_params,
            lane_mix: self.lane_mix,
            lane_patches: self.lane_patches,
            patterns: self.patterns.clone(),
            selected_pattern: self.selected_pattern,
            chain: self.chain,
            samples: self.sampler_ui.samples.clone(),
            sample_regions: self.sampler_ui.regions,
            custom_shader: self.viewport.custom_shader().cloned(),
            voice_calibration: self.voice_calibration,
            instruments,
        }
    }

    fn apply_project_action(&mut self, action: ProjectAction) {
        match action {
            ProjectAction::Save(path) => {
                let result = project::save(&path, self.project_snapshot());
                self.project_ui.finish(path, "Saved", result);
            }
            ProjectAction::Open(path) => {
                let result = project::load(&path).map(|project| self.install_project(project));
                self.project_ui.finish(path, "Opened", result);
            }
        }
    }

    fn install_project(&mut self, project: project::Project) {
        self.project_generation = self.project_generation.wrapping_add(1);
        self.visual_sampler = visual_sampler::VisualSampler::default();
        self.split_undo = None;
        let prepared = Arc::new(AudioProject::prepare(&project, self.project_generation));
        self.bpm = project.bpm;
        self.master_send = project.master_send;
        self.delay_mode = project.delay_mode;
        self.bender_params = project.bender_params;
        self.lane_mix = project.lane_mix;
        self.lane_patches = project.lane_patches;
        self.voice_calibration = project.voice_calibration;
        self.instruments = project.instruments;
        self.rack_panel.status = "";
        self.kit_ui.status.clear();
        self.kit_sounding = [0; NUM_LANES];
        self.rework.reset();
        self.echo_timing = Default::default();
        self.capture_kit_slot = None;
        self.rack_panel.stage = capture::Stage::Idle;
        self.rack_panel.beats_left = None;
        self.rack_panel.routing_error = false;
        self.routed_byte = None;
        self.latest_shader = shader_bus::Status::default();
        self.shader_recording = None;
        self.master_meters = MasterMeters::default();
        self.histories = (0..project.patterns.len())
            .map(|_| EditHistory::default())
            .collect();
        self.patterns = project.patterns;
        self.selected_pattern = project.selected_pattern;
        self.chain = project.chain;
        self.sampler_ui
            .restore(project.samples, project.sample_regions);
        self.viewport.restore_custom_shader(project.custom_shader);
        self.playing = false;
        self.midi.record_armed = false;
        self.midi.events.discard_pending();
        self.fill_active = false;
        #[cfg(target_os = "android")]
        {
            self.touch_fill = tablet_ui::TouchFill::default();
            self.tablet_scrub.end_drag();
        }
        self.inspector_drag_recorded = false;
        self.cursor = TrackerCursor::default();
        self.latest_cv = CvFrame::default();
        self.latest_scope = leviathan_gpu::AudioScopeSnapshot::default();
        self.offline_clock = None;
        self.transport_position = (0.0, 0);
        self.followed_row = None;
        self.latest_song = SongStatus {
            pattern: self.selected_pattern,
            ..SongStatus::default()
        };
        self.send_audio(AudioCommand::ReplaceProject(prepared));
    }
}

impl eframe::App for LeviathanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "android")]
        if let Some(system_bars) = &mut self.system_bars {
            system_bars.update(ctx).reserve(ctx);
        }
        #[cfg(test)]
        if let Some(system_bars) = self.simulated_system_bars {
            system_bars.reserve(ctx);
        }

        #[cfg(target_os = "android")]
        self.poll_android_files(ctx);
        self.audio.flush();
        self.export_ui.poll(ctx);
        let mut peaks = MasterPeaks::default();
        let mut visual_gates = [0.0_f32; NUM_LANES];
        let telemetry_generation = self.project_generation;
        while let Ok(telemetry) = self.tel_rx.pop() {
            if telemetry.generation == self.project_generation {
                if telemetry.play_request == self.play_request {
                    self.playing = telemetry.playing;
                    self.applied_play_request = telemetry.play_request;
                }
                if telemetry.cv.gate_v != self.latest_cv.gate_v || telemetry.cv.retrigger_mask != 0
                {
                    self.sound_link.emit_frame(&telemetry.cv, telemetry.playing);
                }
                for (peak, gate) in visual_gates.iter_mut().zip(telemetry.cv.gate_v) {
                    *peak = peak.max(gate);
                }
                self.latest_cv = telemetry.cv;
                self.midi.clock_bpm = telemetry.midi_clock_bpm;
                self.latest_scope = telemetry.scope;
                self.latest_song = telemetry.song;
                self.rack_panel.stage = telemetry.capture_stage;
                self.rack_panel.beats_left = telemetry.capture_beats_left;
                self.transport_position = telemetry.transport_position;
                self.routed_byte = telemetry.routed_byte;
                self.latest_shader = telemetry.shader;
                self.echo_timing = telemetry.echo_timing;
                self.kit_sounding = telemetry.kit_sounding;
                peaks.merge(telemetry.master);
            }
        }
        if self
            .shader_recording
            .as_ref()
            .is_some_and(|recording| recording.is_done())
        {
            if let Some(take) = self
                .shader_recording
                .take()
                .and_then(|recording| recording.snapshot())
            {
                self.instruments.shader_bus.take = Some(take);
            }
        }
        if let Some(result) = self
            .capture_port
            .poll()
            .filter(|result| result.project == self.project_generation)
        {
            let destination = self.capture_kit_slot.take();
            self.rack_panel.status = match result.outcome {
                Outcome::Complete(rate) => {
                    if let Some(take) = Take::new(rate, result.frames) {
                        if let Some(slot) = destination {
                            match self.instruments.kit.capture_into(slot, &take) {
                                Ok(()) => {
                                    self.publish_kit();
                                    self.kit_ui.status = format!("Capture saved to Slot {}", slot + 1);
                                    "Kit capture ready"
                                }
                                Err(error) => {
                                    self.kit_ui.status = error;
                                    "Kit capture could not fit"
                                }
                            }
                        } else {
                            self.instruments.take = Some(take.clone());
                            self.send_audio(AudioCommand::InstallTake(take));
                            "Capture sample ready"
                        }
                    } else {
                        "Capture had no audio frames"
                    }
                }
                Outcome::Cancelled => "Capture canceled",
                Outcome::Full => "Capture exceeded 192,000 frames",
            };
        }
        self.rework.poll(ctx, self.project_generation, &self.instruments);
        let now = ctx.input(|input| input.time);
        self.master_meters.update(peaks, now);

        let (file, arrangement) = egui::TopBottomPanel::top("project_and_patterns")
            .show(ctx, |ui| {
                #[cfg(not(target_os = "android"))]
                let file = self.project_ui.toolbar(ui, &mut self.export_ui);
                #[cfg(target_os = "android")]
                let file = {
                    ui.horizontal_wrapped(|ui| {
                        let ready = self.android_files.as_ref().is_some_and(|f| !f.pending());
                        if ui.add_enabled(ready, egui::Button::new("Open project")).clicked() {
                            self.request_android_file(android_files::Kind::Open, ui.ctx());
                        }
                        if ui.add_enabled(ready, egui::Button::new("Save a copy")).clicked() {
                            self.request_android_file(android_files::Kind::Save, ui.ctx());
                        }
                        if let Some(files) = &self.android_files {
                            if !files.status.is_empty() { ui.small(&files.status); }
                        }
                        self.export_ui.toolbar(ui, ready);
                    });
                    None
                };
                let arrangement = session_ui::draw_pattern_bar(
                    ui,
                    &self.patterns,
                    self.selected_pattern,
                    self.playing,
                );
                (file, arrangement)
            })
            .inner;
        let modal_open = self.project_ui.dialog_open() || self.export_ui.dialog_open();
        if let Some(action) = file.or_else(|| self.project_ui.show_dialog(ctx)) {
            self.apply_project_action(action);
        } else if !modal_open {
            if let Some(action) = arrangement {
                self.apply_arrangement_action(ctx, action);
            }
        }
        if let Some(request) = self.export_ui.show_dialog(
            ctx,
            self.selected_pattern,
            !self.chain.as_slice().is_empty(),
            &self.instruments.shader_bus,
        ) {
            let snapshot = self.project_snapshot();
            self.export_ui.start(ctx, snapshot, self.fill_active, request);
        }

        let pattern_changed = self.cursor.moved_this_frame;
        self.cursor.moved_this_frame = false;
        let selected = self.selected_pattern;
        let previous_audible = self.lane_mix.audible_mask();
        let previous_channels = self.lane_mix.channels;
        let editor_has_keys =
            !modal_open && !ctx.wants_keyboard_input() && !ctx.memory(|m| m.any_popup_open());
        if editor_has_keys {
            self.deck.read_shortcuts(ctx);
            tracker_keys::apply_lane_mix_shortcuts(ctx, &mut self.lane_mix);
            match TransportShortcut::read(ctx) {
                Some(TransportShortcut::Toggle) => self.toggle_playing(),
                Some(TransportShortcut::FromCursor) => self.play_from_cursor(),
                None => {}
            }
        }

        if editor_has_keys {
            match self.cursor.handle_input(
                ctx,
                &mut self.patterns[selected],
                &mut self.histories[selected],
            ) {
                PatternSync::None => {}
                PatternSync::Single(upd) => {
                    self.send_audio(AudioCommand::SetCell {
                        pattern: selected,
                        lane: upd.lane,
                        row: upd.row,
                        cell: upd.cell,
                    });
                }
                PatternSync::FullPattern => self.push_full_pattern(selected),
                PatternSync::Undo => self.step_history(selected, false),
                PatternSync::Redo => self.step_history(selected, true),
            }
        }

        self.cursor.moved_this_frame |= pattern_changed;
        self.read_midi(ctx, modal_open);
        self.sync_midi_target();
        if self.visual_sampler.poll() {
            self.send_audio(AudioCommand::StopAudition);
        }
        let visual_drop = self.deck == Deck::Samples && self.show_waveform
            && !modal_open && self.visual_sampler.accepts_drop(ctx);
        let (hovering_wav, mut bank_actions) = if visual_drop {
            self.visual_sampler.use_drop(ctx);
            (false, Vec::new())
        } else {
            self.sampler_ui.poll_drag_and_drop(ctx)
        };
        let hovering_wgsl = self.viewport.poll_shader_drop(ctx);
        self.viewport.poll_hot_reload(now);
        self.advance_offline_transport(now);

        self.sound_link.emit_frame(&self.latest_cv, self.playing);
        let mut visual_cv = self.latest_cv;
        if telemetry_generation == self.project_generation {
            for (gate, peak) in visual_cv.gate_v.iter_mut().zip(visual_gates) {
                *gate = gate.max(peak);
            }
        }
        let patch = &self.instruments.shader_bus;
        let probe = patch
            .route
            .lane
            .and(self.latest_shader.request)
            .map(|request| shader_bus::Probe {
                request,
                rotation: patch.rotation,
                threshold: patch.threshold,
            });
        let preview = if cfg!(target_os = "android") {
            None
        } else {
            Some(self.viewport.render_frame(ctx, now, &visual_cv, &self.latest_scope, probe))
        };
        if let Some(reply) = self.viewport.take_probe_result() {
            self.send_audio(AudioCommand::ShaderReply(reply));
        }

        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("LEVIATHAN");
                #[cfg(target_os = "android")]
                if self._audio_stream.is_none() && ui.button("Enable audio").clicked() {
                    self.start_diagnostic_audio();
                }
                ui.separator();
                let (label, tip) = if self.playing {
                    ("Pause", "Pause transport at its current position (Space)")
                } else {
                    ("Play", "Continue from the transport position (Space)")
                };
                if ui.button(label).on_hover_text(tip).clicked() {
                    self.toggle_playing();
                }
                if ui.button("Play from cursor")
                    .on_hover_text("Audition the edited pattern from the selected row (Shift+Space). Leaves chain playback and clears queued launches.")
                    .clicked()
                {
                    self.play_from_cursor();
                }
                if ui.button("Reset").clicked() {
                    self.send_audio(AudioCommand::Reset);
                }
                #[cfg(target_os = "android")]
                {
                    let active = self.touch_fill.draw(ui, !modal_open);
                    self.set_fill(active);
                }
                #[cfg(not(target_os = "android"))]
                if ui.selectable_label(self.fill_active, "FILL").clicked() {
                    self.set_fill(!self.fill_active);
                }
                ui.separator();
                ui.label("BPM");
                if ui
                    .add(egui::DragValue::new(&mut self.bpm).range(40.0..=240.0).speed(0.5))
                    .changed()
                {
                    self.send_audio(AudioCommand::SetBpm(self.bpm));
                }
                ui.separator();
                let follow = self.midi.follow_clock;
                self.midi.draw(ui);
                if self.midi.follow_clock != follow {
                    let follow = self.midi.follow_clock;
                    self.send_audio(AudioCommand::SetMidiClockFollow(follow));
                }
                ui.separator();
                ui.small(&self.audio_status);
            });
        });

        let follow_row = self.follow_row(ctx, modal_open);
        let selected = self.selected_pattern;
        egui::TopBottomPanel::top("telemetry").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                workflow_ui::transport_status(
                    ui,
                    selected,
                    self.latest_song,
                    self.playing,
                    self.transport_position,
                );
                ui.separator();
                self.master_meters.draw(ui, now);
                ui.separator();
                ui.horizontal(|ui| {
                    workflow_ui::bytebus_status(ui, &self.latest_cv, self.routed_byte)
                });
                ui.separator();
                self.sound_link.draw_controls(ui);
                ui.separator();
                if let Some(preview) = preview {
                    if ui
                        .add(
                            egui::Image::new((preview, egui::vec2(48.0, 27.0)))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Open Scene (F6)")
                        .clicked()
                    {
                        self.deck = Deck::Scene;
                    }
                }
                if self.capture_port.busy() {
                    ui.strong(self.rack_panel.busy_text(self.playing));
                    if let Some(slot) = self.capture_kit_slot {
                        ui.small(format!("Kit {}", slot + 1));
                    }
                } else if !self.rack_panel.status.is_empty() {
                    ui.small(self.rack_panel.status);
                }
                if self.rework.busy() {
                    ui.small("REWORK processing");
                }
            });
        });

        egui::TopBottomPanel::top("deck_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                self.deck.draw_tabs(ui);
                ui.separator();
                ui.checkbox(&mut self.follow_playback, "Follow playback")
                    .on_hover_text("Show the playing pattern and keep its row visible. The edit cursor stays on its own lane and row.");
                ui.separator();
                ui.monospace(format!("Oct {} · Step {}", self.cursor.base_octave, self.cursor.edit_step))
                    .on_hover_text("Change octave with - or =, and edit step with [ or ]");
            });
        });

        if self.deck == Deck::Samples {
            egui::TopBottomPanel::top("sampler_views").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.show_waveform, "Waveform").clicked() {
                        self.show_waveform = true;
                    }
                    if ui.selectable_label(!self.show_waveform && !self.show_kit, "Capture and Banks").clicked() {
                        self.show_waveform = false;
                        self.show_kit = false;
                    }
                    if ui.selectable_label(!self.show_waveform && self.show_kit, "OtisKit").clicked() {
                        self.show_waveform = false;
                        self.show_kit = true;
                    }
                });
            });
        }
        let waveform_deck = self.deck == Deck::Samples && self.show_waveform;

        #[allow(unused_mut)]
        let mut touch_layout = cfg!(target_os = "android");
        #[cfg(test)]
        { touch_layout |= self.exercise_tablet_layout; }
        let screen = ctx.screen_rect();
        let portrait = touch_layout && screen.height() > screen.width();

        #[cfg(any(target_os = "android", test))]
        if portrait && !waveform_deck {
            egui::TopBottomPanel::bottom("portrait_note_keys").show(ctx, |ui| {
                ui.add_enabled_ui(!modal_open, |ui| self.draw_touch_notes(ui, selected, true));
            });
        }

        let mut song_action = None;
        if self.deck != Deck::Grid && !waveform_deck {
            let draw_deck = |ui: &mut egui::Ui| {
                let scroll = if portrait {
                    egui::ScrollArea::both()
                } else {
                    egui::ScrollArea::vertical()
                };
                scroll
                    .id_salt((self.deck, self.deck == Deck::Voice && self.edit_lane_patch))
                    .show(ui, |ui| match self.deck {
                        Deck::Grid => {}
                        Deck::Voice => self.draw_voice_deck(ui),
                        Deck::Mixer => self.draw_mixer_deck(ui),
                        Deck::Routing => self.draw_routing_deck(ui),
                        Deck::Samples => {
                            bank_actions.extend(self.draw_samples_deck(ui, hovering_wav))
                        }
                        Deck::Scene => {
                            if let Some(preview) = preview {
                                self.draw_scene_deck(ui, preview, hovering_wgsl);
                            } else {
                                ui.label("Shader feedback is disabled in the Android preview.");
                            }
                        }
                        Deck::Song => {
                            song_action = session_ui::draw_song(
                                ui,
                                &self.patterns,
                                selected,
                                &self.chain,
                                self.latest_song,
                                self.playing,
                            );
                        }
                    });
            };
            if portrait {
                let height = (screen.height() * 0.24).clamp(100.0, 270.0);
                egui::TopBottomPanel::bottom("portrait_workflow_deck")
                    .exact_height(height)
                    .show(ctx, draw_deck);
            } else {
                let width = (screen.width() * 0.35).clamp(320.0, 420.0);
                egui::SidePanel::right("workflow_deck")
                    .resizable(false)
                    .exact_width(width)
                    .show(ctx, draw_deck);
            }
        }
        if !ctx.input(|i| i.pointer.any_down()) {
            self.inspector_drag_recorded = false;
        }

        for action in bank_actions {
            let cmd = match action {
                BankAction::Load {
                    bank,
                    samples,
                    c4_rate_hz,
                } => AudioCommand::LoadSampleBank {
                    bank,
                    samples,
                    c4_rate_hz,
                },
                BankAction::ResetRom { bank } => AudioCommand::ResetSampleBank { bank },
                BankAction::SetRegion { bank, region } => {
                    AudioCommand::SetSampleRegion { bank, region }
                }
            };
            self.send_audio(cmd);
        }

        let tracker = egui::CentralPanel::default().show(ctx, |ui| {
            if waveform_deck {
                egui::ScrollArea::vertical().id_salt("waveform_deck").show(ui, |ui| {
                    ui.add_enabled_ui(!modal_open, |ui| self.draw_visual_sampler(ui));
                });
                return None;
            }
            #[cfg(any(target_os = "android", test))]
            if touch_layout {
                if !modal_open {
                    self.draw_tablet_scrub(ui, selected);
                    if !portrait {
                        self.draw_touch_notes(ui, selected, false);
                    }
                } else {
                    self.tablet_scrub.end_drag();
                }
            }
            let pattern_length = self.patterns[selected].length;
            let has_chain = !self.chain.as_slice().is_empty();
            let (generate, midi_file) = ui
                .add_enabled_ui(!modal_open, |ui| {
                    (
                        self.gen_ui
                            .pattern_tools(ui, &mut self.cursor, pattern_length),
                        self.gen_ui.midi_files(ui, has_chain),
                    )
                })
                .inner;
            if let Some(action) = generate {
                if self.cursor.generate(
                    &mut self.patterns[selected],
                    &mut self.histories[selected],
                    action,
                ) {
                    self.push_full_pattern(selected);
                }
            }
            ui.horizontal_wrapped(|ui| {
                let undo_n = self.histories[selected].undo_len();
                let redo_n = self.histories[selected].redo_len();
                if ui
                    .add_enabled(undo_n > 0, egui::Button::new(format!("Undo ({undo_n})")))
                    .on_hover_text("Ctrl+Z / Cmd+Z")
                    .clicked()
                {
                    self.step_history(selected, false);
                }
                if ui
                    .add_enabled(redo_n > 0, egui::Button::new(format!("Redo ({redo_n})")))
                    .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
                    .clicked()
                {
                    self.step_history(selected, true);
                }
                ui.separator();
                if ui.button("Copy").on_hover_text("Ctrl+C").clicked() {
                    self.cursor.copy_selection(&self.patterns[selected]);
                }
                if ui.button("Cut").on_hover_text("Ctrl+X").clicked() {
                    self.cursor.cut_selection(
                        &mut self.patterns[selected],
                        &mut self.histories[selected],
                    );
                    self.push_full_pattern(selected);
                }
                if ui
                    .add_enabled(self.cursor.clipboard.is_some(), egui::Button::new("Paste"))
                    .on_hover_text("Ctrl+V")
                    .clicked()
                    && self.cursor.paste_selection(
                        &mut self.patterns[selected],
                        &mut self.histories[selected],
                    )
                {
                    self.push_full_pattern(selected);
                }
                ui.separator();
                for (label, semi, tip) in [
                    ("-12", -12, "Ctrl+Shift+Down (-1 octave)"),
                    ("-1", -1, "Ctrl+Down (-1 semitone)"),
                    ("+1", 1, "Ctrl+Up (+1 semitone)"),
                    ("+12", 12, "Ctrl+Shift+Up (+1 octave)"),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        self.cursor.transpose_selection(
                            &mut self.patterns[selected],
                            &mut self.histories[selected],
                            semi,
                        );
                        self.push_full_pattern(selected);
                    }
                }
                ui.separator();
                let (bw, bh) = self.cursor.active_rect().dimensions();
                ui.monospace(format!("{bw}×{bh} · {}", self.cursor.last_edit_msg))
                    .on_hover_text(self.cursor.column.label());
            });
            ui.separator();

            let cur_lane = self.cursor.lane;
            let cur_row = self.cursor.row;
            let cur_col = self.cursor.column;
            let scroll_to_cursor = std::mem::take(&mut self.cursor.moved_this_frame);
            let sel_rect = self.cursor.active_rect();
            let pat_len = self.patterns[selected].length;

            let lane_width = ((ui.available_width() - 36.0
                - ui.spacing().item_spacing.x * NUM_LANES as f32) / NUM_LANES as f32)
                .max(36.0).floor();
            egui::ScrollArea::both().id_salt("tracker_scroll").show(ui, |ui| {
                egui::Grid::new("score_v3_grid")
                    .striped(true)
                    .min_col_width(if portrait { 0.0 } else { 75.0 })
                    .show(ui, |ui| {
                        ui.monospace("ROW");
                        for (lane, patch) in self.lane_patches.iter().enumerate() {
                            let name = format!("{} {}", lane + 1, patch.machine.short_name());
                            if portrait {
                                let button = egui::Button::new(egui::RichText::new(&name).monospace())
                                    .truncate().selected(lane == cur_lane);
                                if ui.add_sized([lane_width, 28.0], button)
                                    .on_hover_text(&name).clicked()
                                {
                                    self.cursor.select(lane, cur_row, SubColumn::Note, false);
                                }
                            } else {
                                self.lane_mix.header(ui, &name, lane);
                            }
                        }
                        ui.end_row();

                        let active_row = self.transport_position.1;
                        for row in 0..ROWS_PER_PATTERN {
                            let row_prefix = if self.playing
                                && self.latest_song.pattern == selected
                                && row == active_row
                            {
                                format!("> {:02X}", row)
                            } else {
                                format!("  {:02X}", row)
                            };
                            let mut row_rt = egui::RichText::new(row_prefix).monospace();
                            if row >= pat_len {
                                row_rt = row_rt.color(egui::Color32::from_rgb(90, 95, 105));
                            }
                            let row_label = ui.label(row_rt);
                            if !scroll_to_cursor && follow_row == Some(row) {
                                let x = ui.clip_rect().center().x;
                                ui.scroll_to_rect(
                                    egui::Rect::from_min_max(
                                        egui::pos2(x, row_label.rect.top()),
                                        egui::pos2(x, row_label.rect.bottom()),
                                    ),
                                    None,
                                );
                            }

                            for lane in 0..NUM_LANES {
                                let cell = self.patterns[selected].lanes[lane][row];
                                let mark = inspector_ui::step_marker(&cell);
                                let note_str = if cell.active {
                                    if cell.air {
                                        format!("AIR{mark}")
                                    } else {
                                        format!("{}{mark}", midi_label(cell.midi))
                                    }
                                } else {
                                    format!("---{mark}")
                                };
                                if portrait {
                                    let here = cur_lane == lane && cur_row == row;
                                    let tone = if row >= pat_len || !cell.active {
                                        egui::Color32::from_rgb(125, 130, 140)
                                    } else {
                                        egui::Color32::from_rgb(220, 235, 255)
                                    };
                                    let button = egui::Button::new(
                                        egui::RichText::new(note_str).monospace().color(tone),
                                    ).truncate().selected(here || (sel_rect.is_multi_cell()
                                        && sel_rect.contains(lane, row)));
                                    let response = ui.add_sized([lane_width, 30.0], button);
                                    if response.clicked() {
                                        self.cursor.select(lane, row, SubColumn::Note,
                                            ui.input(|i| i.modifiers.shift));
                                    }
                                    if response.double_clicked() && row < pat_len {
                                        self.histories[selected]
                                            .record_before_edit(&self.patterns[selected]);
                                        let note = &mut self.patterns[selected].lanes[lane][row];
                                        note.active = !note.active;
                                        let cell = *note;
                                        self.send_audio(AudioCommand::SetCell {
                                            pattern: selected, lane, row, cell,
                                        });
                                    }
                                    if here && scroll_to_cursor {
                                        response.scroll_to_me(None);
                                    }
                                    continue;
                                }
                                let vel_str = if cell.active {
                                    format!("{:02X}", cell.velocity)
                                } else {
                                    "--".to_string()
                                };

                                let is_note_cur = cur_lane == lane
                                    && cur_row == row
                                    && cur_col == SubColumn::Note;
                                let is_vel_cur = cur_lane == lane
                                    && cur_row == row
                                    && cur_col == SubColumn::Velocity;
                                let in_block =
                                    sel_rect.is_multi_cell() && sel_rect.contains(lane, row);

                                let mut note_rt = egui::RichText::new(note_str)
                                    .monospace()
                                    .color(if in_block {
                                        egui::Color32::from_rgb(245, 250, 255)
                                    } else if row >= pat_len {
                                        egui::Color32::from_rgb(90, 95, 105)
                                    } else if cell.active {
                                        egui::Color32::from_rgb(220, 235, 255)
                                    } else {
                                        egui::Color32::from_rgb(125, 130, 140)
                                    });
                                let mut vel_rt = egui::RichText::new(vel_str)
                                    .monospace()
                                    .color(if in_block {
                                        egui::Color32::from_rgb(175, 240, 205)
                                    } else if row >= pat_len {
                                        egui::Color32::from_rgb(80, 85, 95)
                                    } else if cell.active {
                                        egui::Color32::from_rgb(130, 215, 175)
                                    } else {
                                        egui::Color32::from_rgb(105, 110, 120)
                                    });
                                if in_block {
                                    let block_bg = egui::Color32::from_rgba_unmultiplied(
                                        45, 105, 180, 110,
                                    );
                                    note_rt = note_rt.background_color(block_bg);
                                    vel_rt = vel_rt.background_color(block_bg);
                                }

                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0_f32;
                                    let shift_held = ui.input(|i| i.modifiers.shift);
                                    let r_note = ui.selectable_label(is_note_cur, note_rt);
                                    if r_note.clicked() {
                                        self.cursor.select(
                                            lane,
                                            row,
                                            SubColumn::Note,
                                            shift_held,
                                        );
                                    }
                                    if r_note.double_clicked() {
                                        self.histories[selected]
                                            .record_before_edit(&self.patterns[selected]);
                                        let target =
                                            &mut self.patterns[selected].lanes[lane][row];
                                        target.active = !target.active;
                                        let updated = *target;
                                        self.send_audio(AudioCommand::SetCell {
                                            pattern: selected,
                                            lane,
                                            row,
                                            cell: updated,
                                        });
                                    }
                                    if is_note_cur && scroll_to_cursor {
                                        r_note.scroll_to_me(None);
                                    }

                                    let mut r_vel = ui.selectable_label(is_vel_cur, vel_rt);
                                    if cell.active {
                                        r_vel = r_vel.on_hover_ui(|ui| {
                                            ui.label(format!("MIDI velocity {}", cell.velocity));
                                            let machine = MachineKind::from_patch_and_lock(
                                                &self.lane_patches[lane],
                                                &cell.lock,
                                            );
                                            if machine == MachineKind::OtisKit {
                                                ui.label(format!(
                                                    "Kit hit gain · {}",
                                                    param_ui::format_kit_velocity(
                                                        cell.velocity,
                                                        self.instruments.kit.lanes[lane].velocity,
                                                    )
                                                ));
                                            }
                                        });
                                    }
                                    if r_vel.clicked() {
                                        self.cursor.select(
                                            lane,
                                            row,
                                            SubColumn::Velocity,
                                            shift_held,
                                        );
                                    }
                                    if is_vel_cur && scroll_to_cursor {
                                        r_vel.scroll_to_me(None);
                                    }
                                });
                            }
                            ui.end_row();
                        }
                    });
            });
            midi_file
        });
        if let Some(action) = tracker.inner {
            self.apply_midi_file(action);
            ctx.request_repaint();
        }

        if !modal_open {
            if let Some(action) = song_action {
                self.apply_arrangement_action(ctx, action);
                ctx.request_repaint();
            }
        }

        let audible = self.lane_mix.audible_mask();
        if audible != previous_audible {
            self.send_audio(AudioCommand::SetLaneAudibleMask(audible));
        }
        if self.lane_mix.channels != previous_channels {
            self.send_audio(AudioCommand::SetLaneChannels(self.lane_mix.dsp_channels()));
        }
        self.sync_midi_target();
        if self.audio.has_pending() {
            ctx.request_repaint();
        }
        if self.playing
            || self.applied_play_request != self.play_request
            || self.capture_port.busy()
            || self.master_meters.is_active(now)
        {
            if cfg!(target_os = "android") {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            } else {
                self.viewport.request_next_frame(ctx, now);
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_drag_and_drop(true),
        ..eframe::NativeOptions::default()
    };
    run_tracker(options)
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_tag("Leviathan").with_max_level(log::LevelFilter::Info),
    );
    std::panic::set_hook(Box::new(|info| log::error!("{info}")));
    log::info!("NativeActivity entered; starting GLES tracker");
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        android_app: Some(app),
        ..eframe::NativeOptions::default()
    };
    if let Err(error) = run_tracker(options) {
        log::error!("Could not launch Leviathan: {error}");
    }
}

fn run_tracker(options: eframe::NativeOptions) -> eframe::Result<()> {
    #[cfg(target_os = "android")]
    let android_app = options.android_app.clone();
    eframe::run_native(
        "Leviathan -- Rust A/V Tracker",
        options,
        Box::new(move |cc| {
            let app = LeviathanApp::new(cc);
            #[cfg(target_os = "android")]
            let app = {
                let mut app = app;
                app.system_bars = android_app.clone().map(system_bars::SystemBars::new);
                app.android_files = android_app.map(android_files::Files::new);
                app
            };
            Ok(Box::new(app))
        }),
    )
}

/// The default lane machines with their calibrated factory patches.
fn calibrated_lane_patches() -> [LanePatch; NUM_LANES] {
    default_lane_patches().map(|patch| LanePatch::for_machine_with_calibration(patch.machine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Key, Modifiers};
    use leviathan_core::PlaybackRate;

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn tap(key_code: Key, modifiers: Modifiers) -> Vec<Event> {
        let press = key(key_code, modifiers);
        let mut release = press.clone();
        if let Event::Key { pressed, .. } = &mut release {
            *pressed = false;
        }
        vec![press, release]
    }

    fn frame(ctx: &egui::Context, app: &mut LeviathanApp, events: Vec<Event>) -> egui::FullOutput {
        frame_at(ctx, app, egui::vec2(1280.0, 840.0), events)
    }

    fn frame_at(
        ctx: &egui::Context,
        app: &mut LeviathanApp,
        size: egui::Vec2,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                size,
            )),
            events,
            ..egui::RawInput::default()
        };
        ctx.run(input, |ctx| {
            eframe::App::update(app, ctx, &mut eframe::Frame::_new_kittest());
        })
    }

    fn offline_app(ctx: &egui::Context) -> LeviathanApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        LeviathanApp::with_audio(&cc, |_, _, _, _, _| Err("Offline test clock".to_string()))
    }

    fn click_text(ctx: &egui::Context, app: &mut LeviathanApp, label: &str) {
        let output = frame(ctx, app, Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Visible control {label:?}"));
        let pointer = |pressed| Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(ctx, app, vec![Event::PointerMoved(position), pointer(true)]);
        frame(ctx, app, vec![pointer(false)]);
    }

    #[test]
    fn tablet_scrub_uses_one_undo_and_sends_real_cell_commands() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let (tx, mut rx) = rtrb::RingBuffer::new(16);
        app.audio = AudioControl::new(tx);
        app.offline_player = None;
        let original = app.patterns[0].lanes[0][0];
        assert!(original.active);
        let mut gesture = tablet_ui::ScrubDrag::new(tablet_ui::ScrubMode::Note, original);
        app.apply_tablet_scrub(0, 0, 0, &mut gesture, 0.0);
        assert_eq!(app.histories[0].undo_len(), 0);
        assert!(rx.pop().is_err());
        app.apply_tablet_scrub(0, 0, 0, &mut gesture, 8.0);
        app.apply_tablet_scrub(0, 0, 0, &mut gesture, 24.0);
        assert_eq!(app.patterns[0].lanes[0][0].midi, original.midi + 3);
        for semitones in [1, 3] {
            assert!(matches!(rx.pop(), Ok(AudioCommand::SetCell {
                pattern: 0, lane: 0, row: 0, cell,
            }) if cell.midi == original.midi + semitones));
        }
        assert_eq!(app.histories[0].undo_len(), 1);
        app.step_history(0, false);
        assert_eq!(app.patterns[0].lanes[0][0], original);
    }

    #[test]
    fn portrait_keeps_six_lanes_and_the_note_keyboard_in_view() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.deck = Deck::Grid;
        app.exercise_tablet_layout = true;
        app.simulated_system_bars = Some(system_bars::Edges {
            top: 36,
            bottom: 56,
            left: 8,
            right: 8,
        });
        let size = egui::vec2(600.0, 900.0);

        fn bounds(output: &egui::FullOutput, label: &str) -> egui::Rect {
            output.shapes.iter().find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    if text.galley.text() == label {
                        let rect = text.visual_bounding_rect();
                        assert!(shape.clip_rect.contains_rect(rect), "{label} is clipped");
                        return Some(rect);
                    }
                }
                None
            }).unwrap_or_else(|| panic!("Missing {label}"))
        }

        let output = frame_at(&ctx, &mut app, size, Vec::new());
        let last_lane = format!("6 {}", app.lane_patches[5].machine.short_name());
        let first_lane = format!("1 {}", app.lane_patches[0].machine.short_name());
        let first = bounds(&output, &first_lane);
        let last = bounds(&output, &last_lane);
        assert!(first.max.x < last.min.x && last.max.x < size.x);
        let c = bounds(&output, "C");
        let b = bounds(&output, "B");
        assert!(last.max.y < c.min.y && c.max.y < b.min.y && b.max.y < size.y);
        let play = bounds(&output, "Play");
        assert!(play.min.y >= 36.0 && play.max.y < first.min.y);
        assert!(first.min.x >= 8.0 && last.max.x <= size.x - 8.0);
        assert!(b.max.y <= size.y - 56.0);

        // If Android hides navigation, reclaim the bottom rather than using a fixed pad.
        app.simulated_system_bars.as_mut().unwrap().bottom = 0;
        let no_nav = frame_at(&ctx, &mut app, size, Vec::new());
        assert!(bounds(&no_nav, "B").min.y >= b.min.y + 55.0);
        app.simulated_system_bars.as_mut().unwrap().bottom = 56;

        // With an inspector open, the grid and keyboard still share the screen.
        app.deck = Deck::Mixer;
        let output = frame_at(&ctx, &mut app, size, Vec::new());
        assert!(bounds(&output, &last_lane).max.y < bounds(&output, "C").min.y);
    }

    #[test]
    fn visual_sampler_actions_fit_in_the_portrait_safe_area() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.exercise_tablet_layout = true;
        app.deck = Deck::Samples;
        app.visual_sampler = visual_sampler::VisualSampler::four_hit_fixture();
        app.simulated_system_bars = Some(system_bars::Edges {
            top: 36, bottom: 56, ..Default::default()
        });
        let output = frame_at(&ctx, &mut app, egui::vec2(600.0, 900.0), Vec::new());
        for label in ["Waveform editor", "Play trim", "Play 4", "Split to Kit", "Enter hit"] {
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == label
                    && shape.clip_rect.contains_rect(text.visual_bounding_rect())
            )), "Control not visible: {label}");
        }
    }

    #[test]
    fn visual_sampler_four_hits_round_trip_and_undo_without_the_source() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.visual_sampler = visual_sampler::VisualSampler::four_hit_fixture();
        app.patterns[0] = Pattern::default();
        let lane = 2;
        app.lane_patches[lane] = LanePatch::for_machine(MachineKind::OtisKit);
        app.lane_patches[lane].params[0] = 0.875;
        app.cursor.select(lane, 0, SubColumn::Note, false);
        app.commit_visual_split(2).unwrap();
        assert!(app.commit_visual_split(2).is_err());
        for slot in 2..6 {
            app.enter_visual_hit(slot).unwrap();
        }
        let bytes = project::encode(app.project_snapshot()).unwrap();
        drop(app.visual_sampler);
        let from_disk = project::decode(&bytes).unwrap();
        for (row, slot) in (2..6).enumerate() {
            let sample = from_disk.instruments.kit.samples[slot].as_ref().unwrap();
            assert_eq!(sample.pcm.len(), 4);
            assert_eq!(sample.rate, 48_000);
            assert_eq!(from_disk.patterns[0].lanes[lane][row].midi, [67, 60, 61, 62][row]);
            assert!(from_disk.patterns[0].lanes[lane][row].active);
        }
        app.visual_sampler = visual_sampler::VisualSampler::default();
        app.undo_visual_split().unwrap();
        assert!(app.instruments.kit.samples.iter().all(Option::is_none));
    }

    #[test]
    fn touch_note_writes_before_preview_and_wraps_at_pattern_end() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let (tx, mut rx) = rtrb::RingBuffer::new(8);
        app.audio = AudioControl::new(tx);
        app.offline_player = None;
        app.patterns[0].length = 12;
        app.cursor.select(2, 12, SubColumn::Note, false);
        app.enter_touch_note(0, 72);
        assert!(rx.pop().is_err(), "inactive rows don't accept touch notes");

        app.cursor.select(2, 11, SubColumn::Note, false);
        app.enter_touch_note(0, 72);
        let edited = app.patterns[0].lanes[2][11];
        assert_eq!(app.cursor.row, 0);
        assert!(edited.active);
        assert!(!edited.air);
        assert_eq!(edited.midi, 72);
        assert!(matches!(rx.pop(), Ok(AudioCommand::SetCell {
            pattern: 0, lane: 2, row: 11, cell,
        }) if cell == edited));
        assert!(matches!(rx.pop(), Ok(AudioCommand::PreviewStep {lane: 2, cell}) if cell == edited));
    }

    #[test]
    fn momentary_fill_reaches_the_existing_evaluator_and_releases() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let mut fill = tablet_ui::TouchFill::default();
        app.set_fill(fill.update(false, true));
        assert!(app.offline_player.as_ref().unwrap().evaluator.fill_active);
        app.set_fill(fill.update(false, false));
        assert!(!app.offline_player.as_ref().unwrap().evaluator.fill_active);
    }

    #[test]
    fn step_lock_exposes_timbre_controls_for_the_effective_machine() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.cursor.select(0, 0, SubColumn::Note, false);
        assert_eq!(app.lane_patches[0].machine, MachineKind::KickDrum);
        frame(&ctx, &mut app, tap(Key::F2, Modifiers::NONE));
        let visible = |output: &egui::FullOutput, label| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        for (machine, project_label, lane_label) in [
            (
                MachineKind::ByteBusR2r,
                "BBUS cycle lock · Project",
                Some("BBUS grit · Lane 1"),
            ),
            (
                MachineKind::OtisTranswave,
                "OTIS warm tables · Project",
                None,
            ),
            (
                MachineKind::Fs77FormantFm,
                "Oversample FM and Acid · Project",
                Some("FM and Acid grit · Lane 1"),
            ),
        ] {
            app.patterns[0].lanes[0][0].lock.machine = machine as i16;
            let output = frame(&ctx, &mut app, Vec::new());
            assert!(visible(&output, project_label), "{machine:?}");
            if let Some(label) = lane_label {
                assert!(visible(&output, label), "{machine:?}");
            }
        }
        click_text(&ctx, &mut app, "FM and Acid grit · Lane 1");
        assert_eq!(app.instruments.oversample.grit_lanes(), 1);
        click_text(&ctx, &mut app, "Lane Patch");
        let output = frame(&ctx, &mut app, Vec::new());
        assert!(!visible(&output, "Oversample FM and Acid · Project"));
        assert_eq!(app.lane_patches[0].machine, MachineKind::KickDrum);
    }

    #[test]
    fn routing_offers_a_lane_source_for_a_vast_step_lock() {
        use leviathan_core::rack_routing::LaneInput;

        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let patch = app.lane_patches[0];
        assert_eq!(patch.machine, MachineKind::KickDrum);
        app.cursor.select(0, 0, SubColumn::Note, false);
        app.patterns[0].lanes[0][0].lock.machine = MachineKind::VastFunSynth as i16;
        frame(&ctx, &mut app, tap(Key::Space, Modifiers::NONE));
        let output = frame(&ctx, &mut app, tap(Key::F4, Modifiers::NONE));
        let visible = |output: &egui::FullOutput, label| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        assert!(app.playing);
        assert!(visible(&output, "Patch KICK · Step 00/00 VAST"));
        assert!(visible(&output, "Lane 1 input"));
        click_text(&ctx, &mut app, "Instrument");
        click_text(&ctx, &mut app, "Lane 2 into VAST");
        assert_eq!(app.instruments.routing.inputs[0], LaneInput::Lane(1));
        assert_eq!((app.cursor.lane, app.cursor.row), (0, 0));

        app.cursor.select(0, 3, SubColumn::Note, false);
        let output = frame(&ctx, &mut app, Vec::new());
        assert!(visible(&output, "Patch KICK · Step 00/03 KICK"));
        assert!(visible(&output, "Lane 2 into VAST"));

        let path = std::env::temp_dir().join(format!(
            "leviathan-step-routing-{}.leviathan.json",
            std::process::id()
        ));
        project::save(&path, app.project_snapshot()).unwrap();
        let restored = project::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(restored.instruments.routing.inputs[0], LaneInput::Lane(1));
        assert_eq!(restored.lane_patches[0], patch);
        assert_eq!(
            restored.patterns[0].lanes[0][0].lock.machine,
            MachineKind::VastFunSynth as i16
        );
    }

    #[test]
    fn scene_shader_return_controls_are_available_from_the_tracker() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.cursor.select(0, 0, SubColumn::Note, false);
        app.lane_patches[0] = LanePatch::for_machine(MachineKind::OtisTwin);
        frame(&ctx, &mut app, tap(Key::F6, Modifiers::NONE));
        click_text(&ctx, &mut app, "Off");
        click_text(&ctx, &mut app, "Lane 1 · OT2");
        click_text(&ctx, &mut app, "Return → Lookup");
        let route = app.project_snapshot().instruments.shader_bus.route;
        assert_eq!(route.lane, Some(0));
        assert!(route.feedback);

        let output = frame(&ctx, &mut app, tap(Key::F4, Modifiers::NONE));
        let visible = |label| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        assert!(visible("Lookup"));
        assert!(visible("Shader — → — · OT2 FramePos"));
        assert_eq!((app.cursor.lane, app.cursor.row), (0, 0));
    }

    #[test]
    fn new_sessions_start_with_bbus_cycle_lock() {
        let ctx = egui::Context::default();
        let app = offline_app(&ctx);
        assert!(app.instruments.bbus.cycle_lock);
        assert!(app.instruments.otis.warm_tables);
        assert!(app.instruments.oversample.enabled);
        assert_eq!(app.instruments.bbus.grit_lanes(), 0);
        assert!(app.voice_calibration);
        for (patch, default) in app.lane_patches.iter().zip(default_lane_patches()) {
            assert_eq!(*patch, LanePatch::for_machine_with_calibration(default.machine));
        }
        // Saved with calibration on, so the session reopens the same way.
        assert!(app.project_snapshot().voice_calibration);
    }

    #[test]
    fn voice_deck_enables_note_gates_and_keeps_lane_one_shot_selection() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let mut old = app.project_snapshot();
        assert!(old.instruments.note_gates.enabled);
        old.instruments.note_gates = Default::default();
        app.install_project(old);
        frame(&ctx, &mut app, tap(Key::F2, Modifiers::NONE));

        click_text(&ctx, &mut app, "Use note gates");
        assert!(app.instruments.note_gates.enabled);
        click_text(&ctx, &mut app, "Lane 1 one-shot");
        assert!(app.instruments.note_gates.one_shot[0]);
        click_text(&ctx, &mut app, "Use note gates");
        assert!(!app.instruments.note_gates.enabled);
        click_text(&ctx, &mut app, "Use note gates");
        assert!(app.project_snapshot().instruments.note_gates.one_shot[0]);
    }

    #[test]
    fn editing_kit_gain_commits_the_velocity_and_undoes_in_one_step() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let (lane, row) = (app.cursor.lane, app.cursor.row);
        let step = &mut app.patterns[0].lanes[lane][row];
        step.active = true;
        step.lock.machine = MachineKind::OtisKit as i16;
        step.velocity = 110;

        frame(&ctx, &mut app, tap(Key::F2, Modifiers::NONE));
        click_text(&ctx, &mut app, "-9.75 dB");
        frame(&ctx, &mut app, vec![Event::Text("-18".into())]);
        assert_eq!(app.patterns[0].lanes[lane][row].velocity, 110);
        frame(&ctx, &mut app, vec![Event::Text(".75 dB".into())]);
        frame(&ctx, &mut app, tap(Key::Enter, Modifiers::NONE));
        assert_eq!(app.patterns[0].lanes[lane][row].velocity, 94);
        frame(&ctx, &mut app, tap(Key::Z, Modifiers::COMMAND));
        assert_eq!(app.patterns[0].lanes[lane][row].velocity, 110);
    }

    #[test]
    fn twin_step_controls_share_a_lock_and_inherit_the_original_after_clearing() {
        use leviathan_dsp::TwinLoopMode;

        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let (lane, row) = (app.cursor.lane, app.cursor.row);
        let slot = param_ui::MODE_GRIT_SLOT;
        app.lane_patches[lane] = LanePatch::for_machine(MachineKind::OtisTwin);
        app.lane_patches[lane].params[slot] = 0.28;
        app.patterns[0].lanes[lane][row].lock = SoundLock::default();
        frame(&ctx, &mut app, tap(Key::F2, Modifiers::NONE));

        click_text(&ctx, &mut app, "82.35%");
        frame(&ctx, &mut app, tap(Key::Enter, Modifiers::NONE));
        assert!(!app.patterns[0].lanes[lane][row].lock.has_param(slot));

        click_text(&ctx, &mut app, "One shot");
        click_text(&ctx, &mut app, "Bidirectional");
        let lock = app.patterns[0].lanes[lane][row].lock;
        assert_eq!(lock.mask, 1 << slot);
        let (mode, grit) = TwinLoopMode::decode(lock.values[slot]);
        assert_eq!(mode, TwinLoopMode::Bidir);
        assert!((grit - TwinLoopMode::decode(0.28).1).abs() < 1e-6);

        click_text(&ctx, &mut app, "82.35%");
        frame(&ctx, &mut app, vec![Event::Text("100".into())]);
        frame(&ctx, &mut app, tap(Key::Enter, Modifiers::NONE));
        let raw = app.patterns[0].lanes[lane][row].lock.values[slot];
        let (mode, grit) = TwinLoopMode::decode(raw);
        assert_eq!(mode, TwinLoopMode::Bidir);
        assert!((grit - 1.0).abs() < 1e-6);

        click_text(&ctx, &mut app, "Clear Locks");
        let lock = app.patterns[0].lanes[lane][row].lock;
        assert_eq!(lock.mask, 0);
        assert_eq!(app.lane_patches[lane].resolve_lock(&lock).values[slot], 0.28);
    }

    #[test]
    fn selecting_and_editing_patterns_keeps_the_deck_and_pause_position() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.apply_arrangement_action(&ctx, ArrangementAction::AddPattern { clone: true });
        frame(&ctx, &mut app, vec![key(Key::F7, Modifiers::NONE)]);
        assert_eq!(app.deck, Deck::Song);
        frame(&ctx, &mut app, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(app.deck, Deck::Song);
        assert_eq!(app.cursor.row, 1);
        assert!(!app.follow_playback);
        assert_eq!(app.latest_song.pattern, 0);
        assert_eq!(app.selected_pattern, 1);

        frame(&ctx, &mut app, tap(Key::Space, Modifiers::SHIFT));
        assert!(app.playing);
        assert_eq!(app.latest_song.pattern, 1);
        assert_eq!(app.transport_position, (0.25, 1));
        frame(&ctx, &mut app, tap(Key::Space, Modifiers::NONE));
        assert!(!app.playing);
        let paused = app.transport_position;
        app.apply_arrangement_action(&ctx, ArrangementAction::Select(0));
        frame(&ctx, &mut app, tap(Key::Space, Modifiers::NONE));
        assert!(app.playing);
        assert_eq!(app.transport_position, paused);
        assert_eq!(app.latest_song.pattern, 1);
        assert_eq!(app.selected_pattern, 0);
    }

    #[test]
    fn playback_follow_preserves_the_lane_row_and_deck() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.apply_arrangement_action(&ctx, ArrangementAction::AddPattern { clone: true });
        frame(&ctx, &mut app, vec![key(Key::F4, Modifiers::NONE)]);
        frame(&ctx, &mut app, vec![key(Key::Space, Modifiers::NONE)]);
        assert_eq!(app.selected_pattern, 1);
        app.follow_playback = true;
        frame(&ctx, &mut app, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(app.selected_pattern, 1);
        let edit = (app.cursor.lane, app.cursor.row);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.selected_pattern, 0);
        assert_eq!((app.cursor.lane, app.cursor.row), edit);
        assert_eq!(app.deck, Deck::Routing);

        app.apply_arrangement_action(&ctx, ArrangementAction::Select(1));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.selected_pattern, 1);
        assert!(!app.follow_playback);
    }

    #[test]
    fn offline_clock_advances_with_time_instead_of_egui_passes() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.toggle_playing();
        app.advance_offline_transport(1.0);
        let first = app.transport_position;
        app.advance_offline_transport(1.0 + 1.0 / 120.0);
        assert_eq!(app.transport_position, first);
        app.advance_offline_transport(1.1);
        assert!((app.transport_position.0 - 0.22).abs() < 1e-9);
    }

    #[test]
    fn opening_decks_preserves_unsnapped_project_values() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.patterns[0].swing = 0.552;
        app.bender_params.shift_norm = 0.513;
        app.patterns[0].lanes[0][0].lock.mask = 1;
        app.patterns[0].lanes[0][0].lock.values[0] = 1.25;
        app.cursor.select(0, 0, SubColumn::Note, false);
        let before = app.patterns[0].lanes[0][0];
        frame(&ctx, &mut app, tap(Key::F2, Modifiers::NONE));
        frame(&ctx, &mut app, tap(Key::F3, Modifiers::NONE));
        assert_eq!(app.patterns[0].swing, 0.552);
        assert_eq!(app.bender_params.shift_norm, 0.513);
        assert_eq!(app.patterns[0].lanes[0][0], before);
        assert_eq!(app.histories[0].undo_len(), 0);
    }

    #[test]
    fn midi_step_record_preserves_locks_and_uses_the_tracker_edit_step() {
        use midi::MidiKey;

        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.cursor.select(4, 0, SubColumn::Note, false);
        app.cursor.edit_step = 2;
        let before = app.patterns[0].lanes[4][0];
        let first = MidiEvent::NoteOn {
            key: MidiKey { channel: 1, note: 1 },
            velocity: 19,
        };
        app.midi.inject(first);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.patterns[0].lanes[4][0], before);

        app.midi.record_armed = true;
        app.midi.inject(first);
        app.midi.inject(MidiEvent::NoteOn {
            key: MidiKey {
                channel: 1,
                note: 127,
            },
            velocity: 126,
        });
        frame(&ctx, &mut app, Vec::new());
        let notes = &app.patterns[0].lanes[4];
        assert_eq!(
            (notes[0].midi, notes[0].velocity, notes[0].lock),
            (1, 19, before.lock)
        );
        assert!(notes[0].active && !notes[0].air);
        assert_eq!(
            (notes[2].midi, notes[2].velocity, app.cursor.row),
            (127, 126, 4)
        );
        let command = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        frame(&ctx, &mut app, vec![key(Key::Z, command)]);
        assert!(!app.patterns[0].lanes[4][2].active);

        app.midi.inject(MidiEvent::Start);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.playing);
        app.midi.inject(MidiEvent::Stop);
        frame(&ctx, &mut app, Vec::new());
        assert!(!app.playing);
        app.midi.inject(MidiEvent::Continue);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.playing);
    }

    #[test]
    fn ramp_shortcut_is_one_bulk_update_and_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let (tx, mut rx) = rtrb::RingBuffer::new(16);
        app.audio = AudioControl::new(tx);
        app.offline_player = None;
        app.cursor.select(1, 0, SubColumn::Velocity, false);
        app.cursor.select(1, 4, SubColumn::Velocity, true);
        app.patterns[0].lanes[1][0].velocity = 20;
        app.patterns[0].lanes[1][4].velocity = 100;
        let before = app.patterns[0].lanes[1];
        frame(&ctx, &mut app, vec![key(Key::I, Modifiers::CTRL)]);
        assert_eq!(app.histories[0].undo_len(), 1);
        assert_eq!(app.patterns[0].lanes[1][2].velocity, 60);
        let mut updates = 0;
        while let Ok(command) = rx.pop() {
            if let AudioCommand::SetPattern { index, pattern } = command {
                updates += 1;
                assert_eq!(index, 0);
                assert_eq!(pattern.lanes[1][2].velocity, 60);
            }
        }
        assert_eq!(updates, 1);
        frame(&ctx, &mut app, vec![key(Key::Z, Modifiers::COMMAND)]);
        assert_eq!(app.patterns[0].lanes[1], before);
        frame(&ctx, &mut app, vec![key(Key::Y, Modifiers::COMMAND)]);
        assert_eq!(app.patterns[0].lanes[1][2].velocity, 60);
    }

    #[test]
    fn scale_lock_applies_to_keyboard_entry_and_midi_step_recording() {
        use midi::MidiKey;
        use tracker_edit::Scale;

        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.cursor.select(4, 0, SubColumn::Note, false);
        app.cursor.scale_lock.scale = Scale::Major;
        frame(&ctx, &mut app, vec![key(Key::S, Modifiers::NONE)]);
        assert_eq!(app.patterns[0].lanes[4][0].midi, 60);
        app.midi.record_armed = true;
        app.midi.inject(MidiEvent::NoteOn {
            key: MidiKey {
                channel: 0,
                note: 63,
            },
            velocity: 95,
        });
        frame(&ctx, &mut app, Vec::new());
        let recorded = app.patterns[0].lanes[4][1];
        assert_eq!((recorded.midi, recorded.velocity), (62, 95));
    }

    #[test]
    fn midi_file_pattern_import_undo_redo_includes_tempo_and_grid() {
        let path = std::env::temp_dir().join(format!("leviathan-t14-{}.mid", std::process::id()));
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let original = app.patterns[0].clone();
        let original_bpm = app.bpm;
        let mut exported = Pattern {
            length: 8,
            rate: PlaybackRate::Half,
            ..Pattern::default()
        };
        exported.lanes[4][6] = NoteCell {
            active: true,
            midi: 73,
            velocity: 95,
            ..NoteCell::default()
        };
        let file_bpm = 123.456_789;
        smf::save(&path, &[&exported], file_bpm, smf::Format::MultiTrack).unwrap();
        app.apply_midi_file(MidiFileAction::Import {
            path: path.clone(),
            destination: MidiDestination::Pattern,
            rate: PlaybackRate::OneX,
            use_tempo: true,
        });
        std::fs::remove_file(&path).unwrap();
        assert_eq!(app.histories[0].undo_len(), 1);
        assert_eq!(app.patterns[0].lanes[4][6], exported.lanes[4][6]);
        assert_eq!(app.patterns[0].rate, PlaybackRate::Half);
        assert_eq!(app.bpm, file_bpm);
        assert_eq!(
            app.offline_player.as_ref().unwrap().evaluator.config.bpm,
            file_bpm
        );

        frame(&ctx, &mut app, vec![key(Key::Z, Modifiers::COMMAND)]);
        assert_eq!(app.patterns[0].lanes, original.lanes);
        assert_eq!(app.patterns[0].length, original.length);
        assert_eq!(app.bpm, original_bpm);
        assert_eq!(
            app.offline_player.as_ref().unwrap().evaluator.config.bpm,
            original_bpm
        );

        frame(&ctx, &mut app, vec![key(Key::Y, Modifiers::COMMAND)]);
        assert_eq!(app.patterns[0].lanes, exported.lanes);
        assert_eq!(app.patterns[0].length, exported.length);
        assert_eq!(app.bpm, file_bpm);
    }

    #[test]
    fn midi_chain_append_preserves_existing_patterns_and_checks_capacity() {
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        let original = app.patterns[0].lanes;
        let bpm = app.bpm;
        app.apply_arrangement_action(&ctx, ArrangementAction::AppendToChain);
        let make_import = || {
            let mut chain = PatternChain::default();
            for index in [0, 1, 0] {
                chain.push(index);
            }
            smf::Import {
                patterns: vec![Pattern::default(), Pattern::default()],
                chain,
                bpm: 104.0,
                dropped_notes: 0,
                tempo_changes: false,
            }
        };
        app.import_midi(make_import(), MidiDestination::AppendChain, false)
            .unwrap();
        assert_eq!(app.chain.as_slice(), &[0, 1, 2, 1]);
        assert_eq!(app.patterns[0].lanes, original);
        assert_eq!(app.bpm, bpm);
        app.patterns
            .resize_with(project::PATTERN_BANK_CAPACITY - 1, Pattern::default);
        let count = app.patterns.len();
        app.histories.resize_with(count, EditHistory::default);
        assert!(app
            .import_midi(make_import(), MidiDestination::AppendChain, true)
            .is_err());
        assert_eq!(app.chain.as_slice(), &[0, 1, 2, 1]);
        assert_eq!(app.patterns.len(), count);
        assert_eq!(app.bpm, bpm);
    }

    #[test]
    fn midi_chain_can_open_with_a_full_bank_of_patterns() {
        let path =
            std::env::temp_dir().join(format!("leviathan-t14-chain-{}.mid", std::process::id()));
        let ctx = egui::Context::default();
        let mut source = offline_app(&ctx);
        source.patterns = (0..project::PATTERN_BANK_CAPACITY)
            .map(|index| {
                let mut pattern = Pattern {
                    length: 4,
                    ..Pattern::default()
                };
                pattern.lanes[0][0] = NoteCell {
                    active: true,
                    midi: 60 + index as u8,
                    ..NoteCell::default()
                };
                pattern
            })
            .collect();
        for index in 0..source.patterns.len() {
            source.chain.push(index as u8);
        }
        source.apply_midi_file(MidiFileAction::Export {
            path: path.clone(),
            scope: smf::Scope::Chain,
            format: smf::Format::SingleTrack,
        });

        let mut app = offline_app(&ctx);
        app.master_send = 0.35;
        app.apply_midi_file(MidiFileAction::Import {
            path: path.clone(),
            destination: MidiDestination::OpenChain,
            rate: PlaybackRate::EightX,
            use_tempo: true,
        });
        std::fs::remove_file(path).unwrap();
        assert_eq!(app.patterns.len(), project::PATTERN_BANK_CAPACITY);
        assert_eq!(app.histories.len(), app.patterns.len());
        assert_eq!(app.chain, source.chain);
        assert_eq!(app.bpm, source.bpm);
        assert_eq!(app.master_send, 0.35);
        for (actual, expected) in app.patterns.iter().zip(source.patterns) {
            assert_eq!(
                (actual.length, actual.rate),
                (expected.length, expected.rate)
            );
            assert_eq!(actual.lanes, expected.lanes);
        }
    }

    #[test]
    fn save_and_open_shortcuts_restore_the_tracker_session() {
        let path = std::env::temp_dir().join(format!(
            "leviathan-ui-{}.leviathan.json",
            std::process::id()
        ));
        let cmd = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        app.master_send = 0.35;
        let bender_params = BenderParams {
            shift_norm: 0.76,
            freeze: true,
            ..BenderParams::default()
        };
        app.bender_params = bender_params;
        app.lane_mix.channels[3].0.fx_send = 0.48;
        frame(
            &ctx,
            &mut app,
            vec![
                key(Key::Num2, Modifiers::ALT),
                key(Key::Num4, Modifiers::ALT | Modifiers::SHIFT),
            ],
        );
        assert_eq!(app.lane_mix.mute_mask, 0b10);
        assert_eq!(app.lane_mix.solo_mask, 0b1000);
        app.apply_arrangement_action(&ctx, ArrangementAction::AppendToChain);
        app.apply_arrangement_action(&ctx, ArrangementAction::AddPattern { clone: true });
        app.apply_arrangement_action(&ctx, ArrangementAction::SetRate(PlaybackRate::Half));
        app.apply_arrangement_action(&ctx, ArrangementAction::AppendToChain);

        frame(&ctx, &mut app, vec![key(Key::S, cmd)]);
        assert!(app.project_ui.dialog_open());
        frame(
            &ctx,
            &mut app,
            vec![
                key(Key::A, cmd),
                Event::Text(path.to_string_lossy().into_owned()),
            ],
        );
        frame(&ctx, &mut app, vec![key(Key::Enter, Modifiers::NONE)]);
        assert!(!app.project_ui.dialog_open());
        assert!(path.is_file());
        drop(app);

        let ctx = egui::Context::default();
        let mut app = offline_app(&ctx);
        frame(&ctx, &mut app, vec![key(Key::O, cmd)]);
        assert!(app.project_ui.dialog_open());
        frame(
            &ctx,
            &mut app,
            vec![Event::Text(path.to_string_lossy().into_owned())],
        );
        frame(&ctx, &mut app, vec![key(Key::Enter, Modifiers::NONE)]);
        std::fs::remove_file(path).unwrap();
        assert_eq!(app.master_send, 0.35);
        assert_eq!(app.bender_params, bender_params);
        assert_eq!(app.lane_mix.channels[3].0.fx_send, 0.48);
        assert_eq!(app.lane_mix.mute_mask, 0b10);
        assert_eq!(app.lane_mix.solo_mask, 0b1000);
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![key(Key::Num0, Modifiers::ALT)]);
        assert_eq!((app.lane_mix.mute_mask, app.lane_mix.solo_mask), (0, 0));
        assert_eq!(app.lane_mix.channels[3].0.fx_send, 0.48);
        assert_eq!(app.selected_pattern, 1);
        assert_eq!(app.patterns[1].rate, PlaybackRate::Half);
        assert_eq!(app.chain.as_slice(), &[0, 1]);
        assert!(!app.playing);
    }
}
