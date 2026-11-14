mod audio;
mod capture;
mod export;
mod export_ui;
mod gen_ui;
mod gpu_view;
mod inspector_ui;
mod lane_mix;
mod link;
mod locks_ui;
mod master_ui;
mod midi;
mod mixer_ui;
mod project;
mod rack_ui;
mod sampler_ui;
mod session_ui;
mod smf;
mod tracker_edit;
mod tracker_keys;
mod wav;
mod workflow_ui;

use std::sync::Arc;

use eframe::egui;
use export_ui::ExportUi;
use gen_ui::{GenUi, MidiDestination, MidiFileAction};
use gpu_view::FeedbackViewport;
use lane_mix::LaneMix;
use leviathan_core::{
    CvFrame, EngineConfig, FillCondition, NoteCell, Pattern, PatternChain, SongPlayer, SongStatus,
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
    routed_byte: Option<u8>,
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
        let mut app = Self::with_audio(cc, try_start_audio);
        app.midi.refresh();
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
        let viewport = FeedbackViewport::new(cc);

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
            voice_calibration: false,
            inspector_drag_recorded: false,
            deck: Deck::default(),
            edit_lane_patch: false,
            follow_playback: false,
            followed_row: None,
            playing: false,
            fill_active: false,
            bpm: 132.0,
            master_send: 0.0,
            delay_mode: DelayMode::default(),
            bender_params: BenderParams::default(),
            lane_mix: LaneMix::default(),
            lane_patches: default_lane_patches(),
            master_meters: MasterMeters::default(),
            sound_link: SoundLinkBridge::default(),
            midi,
            midi_target: MidiTarget::default(),
            play_request: 0,
            applied_play_request: 0,
            cursor,
            sampler_ui: SamplerBankUi::default(),
            instruments: project::InstrumentSettings::default(),
            capture_port,
            rack_panel: rack_ui::RackPanel::default(),
            routed_byte: None,
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
        app.sync_midi_target();
        app
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
        self.playing = !self.playing;
        self.play_request = self.play_request.wrapping_add(1);
        self.send_audio(AudioCommand::SetPlaying {
            playing: self.playing,
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
        ui.separator();
        let (lane, row, pattern) = (self.cursor.lane, self.cursor.row, self.selected_pattern);
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
    }

    fn draw_routing_deck(&mut self, ui: &mut egui::Ui) {
        let lane = self.cursor.lane;
        ui.heading(format!(
            "Lane {} · {}",
            lane + 1,
            self.lane_patches[lane].machine.short_name()
        ));
        let mut routing = self.instruments.routing;
        rack_ui::draw_routing(
            ui,
            &mut routing,
            lane,
            self.lane_patches[lane],
            self.instruments.take.is_some(),
            self.routed_byte,
            self.rack_panel.routing_error,
        );
        if routing != self.instruments.routing {
            self.rack_panel.routing_error = routing.render_order().is_none();
            if !self.rack_panel.routing_error {
                self.instruments.routing = routing;
                self.send_audio(AudioCommand::SetRackRouting(routing));
            }
        }
    }

    fn draw_samples_deck(&mut self, ui: &mut egui::Ui, hovering_wav: bool) -> Vec<BankAction> {
        match self.rack_panel.draw_capture(
            ui,
            self.instruments.take.as_deref(),
            self._audio_stream.is_some(),
            self.capture_port.busy(),
            self.playing,
        ) {
            Some(rack_ui::CaptureAction::Arm(source, beats)) => {
                if self
                    .capture_port
                    .request(self.project_generation, source, beats)
                {
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
        self.sampler_ui.draw(ui, hovering_wav)
    }

    fn draw_scene_deck(
        &mut self,
        ui: &mut egui::Ui,
        preview: egui::TextureId,
        hovering_wgsl: bool,
    ) {
        ui.heading("Feedback");
        let width = ui.available_width();
        ui.image((preview, egui::vec2(width, width * 9.0 / 16.0)));
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
            instruments: self.instruments.clone(),
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
        self.rack_panel.stage = capture::Stage::Idle;
        self.rack_panel.beats_left = None;
        self.rack_panel.routing_error = false;
        self.routed_byte = None;
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
                self.latest_scope = telemetry.scope;
                self.latest_song = telemetry.song;
                self.rack_panel.stage = telemetry.capture_stage;
                self.rack_panel.beats_left = telemetry.capture_beats_left;
                self.transport_position = telemetry.transport_position;
                self.routed_byte = telemetry.routed_byte;
                peaks.merge(telemetry.master);
            }
        }
        if let Some(result) = self
            .capture_port
            .poll()
            .filter(|result| result.project == self.project_generation)
        {
            self.rack_panel.status = match result.outcome {
                Outcome::Complete(rate) => {
                    if let Some(take) = Take::new(rate, result.frames) {
                        self.instruments.take = Some(take.clone());
                        self.send_audio(AudioCommand::InstallTake(take));
                        "Capture sample ready"
                    } else {
                        "Capture had no audio frames"
                    }
                }
                Outcome::Cancelled => "Capture canceled",
                Outcome::Full => "Capture exceeded 192,000 frames",
            };
        }
        let now = ctx.input(|input| input.time);
        self.master_meters.update(peaks, now);

        let (file, arrangement) = egui::TopBottomPanel::top("project_and_patterns")
            .show(ctx, |ui| {
                let file = self.project_ui.toolbar(ui, &mut self.export_ui);
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
        let (hovering_wav, mut bank_actions) = self.sampler_ui.poll_drag_and_drop(ctx);
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
        let preview = self
            .viewport
            .render_frame(ctx, now, &visual_cv, &self.latest_scope);

        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("LEVIATHAN");
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
                if ui.selectable_label(self.fill_active, "FILL").clicked() {
                    self.fill_active = !self.fill_active;
                    self.send_audio(AudioCommand::SetFill(self.fill_active));
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
                self.midi.draw(ui);
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
                if self.capture_port.busy() {
                    ui.strong(self.rack_panel.busy_text(self.playing));
                } else if !self.rack_panel.status.is_empty() {
                    ui.small(self.rack_panel.status);
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

        let mut song_action = None;
        if self.deck != Deck::Grid {
            let width = (ctx.screen_rect().width() * 0.35).clamp(320.0, 420.0);
            egui::SidePanel::right("workflow_deck")
                .resizable(false)
                .exact_width(width)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt((self.deck, self.deck == Deck::Voice && self.edit_lane_patch))
                        .show(ui, |ui| match self.deck {
                            Deck::Grid => {}
                            Deck::Voice => self.draw_voice_deck(ui),
                            Deck::Mixer => self.draw_mixer_deck(ui),
                            Deck::Routing => self.draw_routing_deck(ui),
                            Deck::Samples => {
                                bank_actions.extend(self.draw_samples_deck(ui, hovering_wav))
                            }
                            Deck::Scene => self.draw_scene_deck(ui, preview, hovering_wgsl),
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
                });
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

            egui::ScrollArea::both().id_salt("tracker_scroll").show(ui, |ui| {
                egui::Grid::new("score_v3_grid")
                    .striped(true)
                    .min_col_width(75.0)
                    .show(ui, |ui| {
                        ui.monospace("ROW");
                        for (lane, patch) in self.lane_patches.iter().enumerate() {
                            let name = format!("{} {}", lane + 1, patch.machine.short_name());
                            self.lane_mix.header(ui, &name, lane);
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

                                    let r_vel = ui.selectable_label(is_vel_cur, vel_rt);
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
            self.viewport.request_next_frame(ctx, now);
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_drag_and_drop(true),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Leviathan -- Rust A/V Tracker",
        options,
        Box::new(|cc| Ok(Box::new(LeviathanApp::new(cc)))),
    )
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

    fn frame(ctx: &egui::Context, app: &mut LeviathanApp, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 840.0),
            )),
            events,
            ..egui::RawInput::default()
        };
        let _ = ctx.run(input, |ctx| {
            eframe::App::update(app, ctx, &mut eframe::Frame::_new_kittest());
        });
    }

    fn offline_app(ctx: &egui::Context) -> LeviathanApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        LeviathanApp::with_audio(&cc, |_, _, _, _, _| Err("Offline test clock".to_string()))
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
