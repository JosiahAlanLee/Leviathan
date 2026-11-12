mod audio;
mod export;
mod export_ui;
mod gpu_view;
mod inspector_ui;
mod lane_mix;
mod link;
mod master_ui;
mod midi;
mod project;
mod sampler_ui;
mod session_ui;
mod tracker_edit;
mod tracker_keys;
mod wav;

use std::sync::Arc;

use eframe::egui;
use export_ui::ExportUi;
use gpu_view::FeedbackViewport;
use inspector_ui::{InspectorEdits, LANE_NAMES};
use lane_mix::LaneMix;
use leviathan_core::{
    CvFrame, EngineConfig, FillCondition, LaunchTarget, NoteCell, Pattern, PatternChain,
    SongPlayer, SongStatus, SoundLock, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{MachineKind, MasterPeaks};
use link::SoundLinkBridge;
use master_ui::MasterMeters;
use midi::{MidiControls, MidiEvent, MidiReceiver, MidiTarget};

use audio::{try_start_audio, AudioCommand, AudioControl, AudioProject, AudioTelemetry};
use sampler_ui::{BankAction, SamplerBankUi};
use session_ui::{ArrangementAction, ProjectAction, ProjectUi};
use tracker_edit::EditHistory;
use tracker_keys::{PatternSync, SubColumn, TrackerCursor};

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
    project_generation: u64,
    inspector_drag_recorded: bool,
    playing: bool,
    fill_active: bool,
    bpm: f64,
    master_send: f32,
    lane_mix: LaneMix,
    master_meters: MasterMeters,
    sound_link: SoundLinkBridge,
    midi: MidiControls,
    midi_target: MidiTarget,
    play_request: u64,
    cursor: TrackerCursor,
    sampler_ui: SamplerBankUi,
    audio: AudioControl,
    tel_rx: rtrb::Consumer<AudioTelemetry>,
    offline_player: Option<SongPlayer>,
    audio_status: String,
    latest_cv: CvFrame
        latest_scope: leviathan_gpu::AudioScopeSnapshot,,
    latest_song: SongStatus,
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
        ) -> Result<(cpal::Stream, String), String>,
    ) -> Self {
        let viewport = FeedbackViewport::new(cc);

        let initial_pattern = starter_electro_pattern();
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::<AudioCommand>::new(256);
        let (tel_tx, tel_rx) = rtrb::RingBuffer::<AudioTelemetry>::new(64);
        let (midi, midi_events) = MidiControls::new();

        let (_audio_stream, offline_player, audio_status) =
            match start_audio(initial_pattern.clone(), cmd_rx, tel_tx, midi_events) {
                Ok((stream, label)) => (Some(stream), None, label),
                Err(label) => (
                    None,
                    Some(SongPlayer::new(EngineConfig {
                        sample_rate: 60.0,
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
            project_generation: 0,
            inspector_drag_recorded: false,
            playing: false,
            fill_active: false,
            bpm: 132.0,
            master_send: 0.0,
            lane_mix: LaneMix::default(),
            master_meters: MasterMeters::default(),
            sound_link: SoundLinkBridge::default(),
            midi,
            midi_target: MidiTarget::default(),
            play_request: 0,
            cursor,
            sampler_ui: SamplerBankUi::default(),
            audio: AudioControl::new(cmd_tx),
            tel_rx,
            offline_player,
            audio_status,
            latest_cv: CvFrame::default(),
            latest_scope: leviathan_gpu::AudioScopeSnapshot::default(),
            latest_song: SongStatus::default(),
            viewport,
        };
        app.sync_midi_target();
        app
    }

    fn send_audio(&mut self, command: AudioCommand) {
        if let Some(player) = &mut self.offline_player {
            command.apply_transport(player, self.patterns.len());
            self.latest_song = player.status();
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

    fn read_midi(&mut self, ctx: &egui::Context) {
        let (_, count) = self.midi.events.begin_batch();
        let record = self.midi.record_armed
            && !self.project_ui.dialog_open()
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
                    self.cursor.last_edit_msg = format!("MIDI {}", midi_label(key.note));
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

    fn select_pattern(&mut self, index: usize) {
        if self.selected_pattern == index {
            return;
        }
        self.selected_pattern = index;
        self.cursor.anchor = None;
        self.cursor.moved_this_frame = true;
        self.inspector_drag_recorded = false;
        if !self.playing {
            self.send_audio(AudioCommand::Launch(LaunchTarget::Pattern(index)));
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

    fn apply_inspector_edits(
        &mut self,
        ctx: &egui::Context,
        selected: usize,
        lane: usize,
        row: usize,
        edits: InspectorEdits,
    ) {
        if edits.cell.is_none()
            && edits.automation.is_none()
            && edits.pattern_length.is_none()
            && edits.pattern_rate.is_none()
        {
            return;
        }
        self.record_inspector_undo_once(ctx, selected);

        let mut full_sync = false;
        if let Some(len) = edits.pattern_length {
            self.patterns[selected].length = len.clamp(1, ROWS_PER_PATTERN);
            if self.cursor.row >= self.patterns[selected].length {
                self.cursor.row = self.patterns[selected].length - 1;
            }
            full_sync = true;
        }
        if let Some(rate) = edits.pattern_rate {
            self.patterns[selected].rate = rate;
            full_sync = true;
        }
        if let Some(cell) = edits.cell {
            self.patterns[selected].lanes[lane][row] = cell;
            if !full_sync {
                self.send_audio(AudioCommand::SetCell {
                    pattern: selected,
                    lane,
                    row,
                    cell,
                });
            }
        }
        if let Some(automation) = edits.automation {
            self.patterns[selected].automation[row] = automation;
            if !full_sync {
                self.send_audio(AudioCommand::SetAutomation {
                    pattern: selected,
                    row,
                    automation,
                });
            }
        }
        if full_sync {
            self.push_full_pattern(selected);
        }
    }

    fn apply_arrangement_action(&mut self, action: ArrangementAction) {
        let index = self.selected_pattern;
        match action {
            ArrangementAction::Select(index) => self.select_pattern(index),
            ArrangementAction::AddPattern { clone } => {
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
            ArrangementAction::SetRate(rate) => {
                let pattern = &mut self.patterns[index];
                self.histories[index].record_before_edit(pattern);
                pattern.rate = rate;
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
            lane_mix: self.lane_mix,
            patterns: self.patterns.clone(),
            selected_pattern: self.selected_pattern,
            chain: self.chain,
            samples: self.sampler_ui.samples.clone(),
custom_shader: self.viewport.custom_shader().cloned(),
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
        self.lane_mix = project.lane_mix;
        self.master_meters = MasterMeters::default();
        self.histories = (0..project.patterns.len())
            .map(|_| EditHistory::default())
            .collect();
        self.patterns = project.patterns;
        self.selected_pattern = project.selected_pattern;
        self.chain = project.chain;
        self.sampler_ui.restore(project.samples);
        self.viewport.restore_custom_shader(project.custom_shader);
        self.playing = false;
        self.midi.record_armed = false;
        self.midi.events.discard_pending();
        self.fill_active = false;
        self.inspector_drag_recorded = false;
        self.cursor = TrackerCursor::default();
        self.latest_cv = CvFrame::default()
        self.latest_scope = leviathan_gpu::AudioScopeSnapshot::default();;
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
        while let Ok(telemetry) = self.tel_rx.pop() {
            if telemetry.generation == self.project_generation {
                if telemetry.play_request == self.play_request {
                    self.playing = telemetry.playing;
                }
                self.latest_cv = telemetry.cv;
                self.latest_scope = telemetry.scope;
                self.latest_song = telemetry.song;
                peaks.merge(telemetry.master);
            }
        }
        let now = ctx.input(|input| input.time);
        self.master_meters.update(peaks, now);

        let (file, arrangement) = egui::TopBottomPanel::top("project_and_patterns")
            .show(ctx, |ui| {
                let file = self.project_ui.toolbar(ui, &mut self.export_ui);
                let arrangement = session_ui::draw_arrangement(
                    ui,
                    &self.patterns,
                    self.selected_pattern,
                    &self.chain,
                    self.latest_song,
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
                self.apply_arrangement_action(action);
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

        let selected = self.selected_pattern;
        let previous_audible = self.lane_mix.audible_mask();
        if !modal_open && !ctx.wants_keyboard_input() {
            tracker_keys::apply_lane_mix_shortcuts(ctx, &mut self.lane_mix);
        }

        if !ctx.input(|i| i.pointer.any_down()) {
            self.inspector_drag_recorded = false;
        }

        if !modal_open
            && !ctx.wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
        {
            self.toggle_playing();
        }

        if !modal_open {
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
            }
        }

        self.read_midi(ctx);
        self.sync_midi_target();
        let (hovering_wav, mut bank_actions) = self.sampler_ui.poll_drag_and_drop(ctx);
        let hovering_wgsl = self.viewport.poll_shader_drop(ctx);
        let now_secs = ctx.input(|i| i.time);
        self.viewport.poll_hot_reload(now_secs);
        if let Some(player) = self.offline_player.as_mut() {
            self.latest_cv = player.step_sample(&self.patterns).0;
            self.latest_song = player.status();
        }

        self.sound_link.emit_frame(&self.latest_cv, self.playing);
        let tex_id = self.viewport.render_frame(ctx, &self.latest_cv), &self.latest_scope;

        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("LEVIATHAN");
                ui.separator();
                let play_label = if self.playing {
                    "Stop [Space]"
                } else {
                    "Play [Space]"
                };
                if ui.button(play_label).clicked() {
                    self.toggle_playing();
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
                ui.label("Delay send");
                let mut send_percent = self.master_send * 100.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut send_percent)
                            .range(0.0..=100.0)
                            .speed(0.5_f64)
                            .suffix("%"),
                    )
                    .changed()
                    && send_percent.is_finite()
                {
                    self.master_send = (send_percent / 100.0).clamp(0.0, 1.0);
                    self.send_audio(AudioCommand::SetMasterSend(self.master_send));
                }
                self.master_meters.draw(ui, now);
                ui.separator();
                self.sound_link.draw_controls(ui);
                ui.separator();
                ui.monospace(format!(
                    "Oct:{} [-/=] | Step:{} [[/]] | Col:{} | Beat {:06.2} Row {:02X}",
                    self.cursor.base_octave,
                    self.cursor.edit_step,
                    self.cursor.column.label(),
                    self.latest_cv.beat,
                    self.latest_cv.row
                ));
                ui.separator();
                ui.small(&self.audio_status);
                ui.separator();
                ui.small(self.viewport.status_label());
                ui.separator();
                self.midi.draw(ui);
            });
        });

        let (sel_lane, sel_row) = (self.cursor.lane, self.cursor.row);
        let cur_cell = self.patterns[selected].lanes[sel_lane][sel_row];
        let cur_auto = self.patterns[selected].automation[sel_row];
        let cur_len = self.patterns[selected].length;
        let cur_rate = self.patterns[selected].rate;
        let mut inspector_edits = InspectorEdits::default();

        egui::SidePanel::right("visual_and_bus_panel")
            .min_width(376.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Live Feedback Surface");
                    ui.image((tex_id, egui::vec2(348.0, 196.0)));
                    ui.monospace(format!(
                        "Z: {:4.2} V | Roll: {:+5.2} V | Fbk: {:4.2} V",
                        self.latest_cv.z_v, self.latest_cv.roll_v, self.latest_cv.feedback_v
                    ));
                    ui.horizontal(|ui| {
                        for bit in 0..9 {
                            let high = self.latest_cv.bytebus_v[bit] > 5.0;
                            let tag = if bit < 8 {
                                format!("B{bit}")
                            } else {
                                "CLK".to_string()
                            };
                            let _ = ui.selectable_label(high, tag);
                        }
                    });

                    ui.separator();
                    self.viewport.draw_shader_controls(ui, hovering_wgsl);
                    bank_actions.extend(self.sampler_ui.draw(ui, hovering_wav));

                    ui.separator();
                    inspector_edits = inspector_ui::draw(
                        ui, sel_lane, sel_row, cur_cell, cur_auto, cur_len, cur_rate,
                    );
                });
            });

        self.apply_inspector_edits(ctx, selected, sel_lane, sel_row, inspector_edits);

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
            };
            self.send_audio(cmd);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let undo_n = self.histories[selected].undo_len();
                let redo_n = self.histories[selected].redo_len();
                if ui
                    .add_enabled(undo_n > 0, egui::Button::new(format!("Undo ({undo_n})")))
                    .on_hover_text("Ctrl+Z / Cmd+Z")
                    .clicked()
                    && self.histories[selected].undo(&mut self.patterns[selected])
                {
                    self.cursor.last_edit_msg = "Undo".to_string();
                    self.push_full_pattern(selected);
                }
                if ui
                    .add_enabled(redo_n > 0, egui::Button::new(format!("Redo ({redo_n})")))
                    .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
                    .clicked()
                    && self.histories[selected].redo(&mut self.patterns[selected])
                {
                    self.cursor.last_edit_msg = "Redo".to_string();
                    self.push_full_pattern(selected);
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
                ui.monospace(format!("Sel:{bw}x{bh} | {}", self.cursor.last_edit_msg));
            });
            ui.separator();

            let cur_lane = self.cursor.lane;
            let cur_row = self.cursor.row;
            let cur_col = self.cursor.column;
            let scroll_to_cursor = self.cursor.moved_this_frame;
            let sel_rect = self.cursor.active_rect();
            let pat_len = self.patterns[selected].length;

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("score_v3_grid")
                    .striped(true)
                    .min_col_width(82.0)
                    .show(ui, |ui| {
                        ui.monospace("ROW");
                        for (lane, name) in LANE_NAMES.into_iter().enumerate() {
                            self.lane_mix.header(ui, name, lane);
                        }
                        ui.end_row();

                        let active_row = self.latest_cv.row;
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
                            ui.label(row_rt);

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

                                let is_note_cur =
                                    cur_lane == lane && cur_row == row && cur_col == SubColumn::Note;
                                let is_vel_cur = cur_lane == lane
                                    && cur_row == row
                                    && cur_col == SubColumn::Velocity;
                                let in_block =
                                    sel_rect.is_multi_cell() && sel_rect.contains(lane, row);

                                let mut note_rt = egui::RichText::new(note_str).monospace().color(
                                    if in_block {
                                        egui::Color32::from_rgb(245, 250, 255)
                                    } else if row >= pat_len {
                                        egui::Color32::from_rgb(90, 95, 105)
                                    } else if cell.active {
                                        egui::Color32::from_rgb(220, 235, 255)
                                    } else {
                                        egui::Color32::from_rgb(125, 130, 140)
                                    },
                                );
                                let mut vel_rt = egui::RichText::new(vel_str).monospace().color(
                                    if in_block {
                                        egui::Color32::from_rgb(175, 240, 205)
                                    } else if row >= pat_len {
                                        egui::Color32::from_rgb(80, 85, 95)
                                    } else if cell.active {
                                        egui::Color32::from_rgb(130, 215, 175)
                                    } else {
                                        egui::Color32::from_rgb(105, 110, 120)
                                    },
                                );
                                if in_block {
                                    let block_bg =
                                        egui::Color32::from_rgba_unmultiplied(45, 105, 180, 110);
                                    note_rt = note_rt.background_color(block_bg);
                                    vel_rt = vel_rt.background_color(block_bg);
                                }

                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0_f32;
                                    let shift_held = ui.input(|i| i.modifiers.shift);
                                    let r_note = ui.selectable_label(is_note_cur, note_rt);
                                    if r_note.clicked() {
                                        self.cursor.select(lane, row, SubColumn::Note, shift_held);
                                    }
                                    if r_note.double_clicked() {
                                        self.histories[selected]
                                            .record_before_edit(&self.patterns[selected]);
                                        let target = &mut self.patterns[selected].lanes[lane][row];
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
                                        self.cursor
                                            .select(lane, row, SubColumn::Velocity, shift_held);
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
        });

        let audible = self.lane_mix.audible_mask();
        if audible != previous_audible {
            self.send_audio(AudioCommand::SetLaneAudibleMask(audible));
        }
        self.sync_midi_target();
        if self.playing || self.audio.has_pending() || self.master_meters.is_active(now) {
            ctx.request_repaint();
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
        LeviathanApp::with_audio(&cc, |_, _, _, _| Err("Offline test clock".to_string()))
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
        frame(
            &ctx,
            &mut app,
            vec![
                key(Key::Num2, Modifiers::ALT),
                key(Key::Num4, Modifiers::ALT | Modifiers::SHIFT),
            ],
        );
        assert_eq!(app.lane_mix, LaneMix { mute_mask: 0b10, solo_mask: 0b1000 });
        app.apply_arrangement_action(ArrangementAction::AppendToChain);
        app.apply_arrangement_action(ArrangementAction::AddPattern { clone: true });
        app.apply_arrangement_action(ArrangementAction::SetRate(PlaybackRate::Half));
        app.apply_arrangement_action(ArrangementAction::AppendToChain);

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
        assert_eq!(app.lane_mix, LaneMix { mute_mask: 0b10, solo_mask: 0b1000 });
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![key(Key::Num0, Modifiers::ALT)]);
        assert_eq!(app.lane_mix, LaneMix::default());
        assert_eq!(app.selected_pattern, 1);
        assert_eq!(app.patterns[1].rate, PlaybackRate::Half);
        assert_eq!(app.chain.as_slice(), &[0, 1]);
        assert!(!app.playing);
    }
}
