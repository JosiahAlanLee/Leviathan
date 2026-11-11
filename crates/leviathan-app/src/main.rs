mod gpu_view;
mod sampler_ui;
mod tracker_edit;
mod tracker_keys;
mod wav;

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use gpu_view::FeedbackViewport;
use leviathan_core::{
    CvFrame, EngineConfig, FillCondition, NoteCell, Pattern, SoundLock, TrackerEvaluator,
    NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{MachineKind, SynthRack};
use sampler_ui::{BankAction, SamplerBankUi};
use tracker_edit::EditHistory;
use tracker_keys::{PatternSync, SubColumn, TrackerCursor};

const LANE_NAMES: [&str; NUM_LANES] = ["0 KICK", "1 FM4", "2 SUB", "3 AIR", "4 SMPL", "5 ACID"];

const ALL_MACHINES: [MachineKind; 7] = [
    MachineKind::KickDrum,
    MachineKind::FourOpFm,
    MachineKind::SubBass,
    MachineKind::NoiseWash,
    MachineKind::VarSampler,
    MachineKind::AcidLead,
    MachineKind::ChordStab,
];

fn param_labels(machine: MachineKind) -> [&'static str; 8] {
    match machine {
        MachineKind::VarSampler => [
            "P0 Decay",
            "P1 ChipClk",
            "P2 Cutoff",
            "P3 WaveBank",
            "P4 LpStart",
            "P5 LpWindow",
            "P6 LpScan",
            "P7 Mode/Bit",
        ],
        MachineKind::FourOpFm => [
            "P0 Decay",
            "P1 FmIndex",
            "P2 Cutoff",
            "P3 Alg(0-3)",
            "P4 C:M Ratio",
            "P5 ModDecay",
            "P6 Op4 Fbk",
            "P7 Detune",
        ],
        _ => [
            "P0 Decay",
            "P1 Drive",
            "P2 Cutoff",
            "P3 Aux3",
            "P4 Aux4",
            "P5 Aux5",
            "P6 Aux6",
            "P7 Aux7",
        ],
    }
}

#[derive(Clone, Debug)]
enum AudioCommand {
    SetPlaying(bool),
    SetFill(bool),
    SetBpm(f64),
    Reset,
    SetCell {
        lane: usize,
        row: usize,
        cell: NoteCell,
    },
    SetPattern(Arc<Pattern>),
    LoadSampleBank {
        bank: usize,
        samples: Arc<[i16]>,
        c4_rate_hz: f32,
    },
    ResetSampleBank {
        bank: usize,
    },
}

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

struct RtAudioWorker {
    pattern: Box<Pattern>,
    evaluator: TrackerEvaluator,
    synth: SynthRack,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<CvFrame>,
    tel_divider: u32,
}

impl RtAudioWorker {
    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            match cmd {
                AudioCommand::SetPlaying(p) => self.evaluator.playing = p,
                AudioCommand::SetFill(f) => self.evaluator.fill_active = f,
                AudioCommand::SetBpm(bpm) => self.evaluator.config.bpm = bpm.clamp(40.0, 240.0),
                AudioCommand::Reset => self.evaluator.reset_transport(),
                AudioCommand::SetCell { lane, row, cell } => {
                    if lane < NUM_LANES && row < ROWS_PER_PATTERN {
                        self.pattern.lanes[lane][row] = cell;
                    }
                }
                AudioCommand::SetPattern(snap) => {
                    self.pattern.clone_from(&snap);
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
                    self.synth.sample_bank.reset_rom_bank(bank);
                }
            }
        }
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        let cv = self.evaluator.step_sample(&self.pattern);
        let pair = self.synth.render_stereo(&cv);
        self.tel_divider = self.tel_divider.wrapping_add(1);
        if (self.tel_divider & 255) == 0 {
            let _ = self.tel_tx.push(cv);
        }
        pair
    }
}

fn try_start_audio(
    initial_pattern: Pattern,
    cmd_rx: rtrb::Consumer<AudioCommand>,
    tel_tx: rtrb::Producer<CvFrame>,
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

    let mut worker = RtAudioWorker {
        pattern: Box::new(initial_pattern),
        evaluator: TrackerEvaluator::new(EngineConfig {
            sample_rate,
            ..EngineConfig::default()
        }),
        synth: SynthRack::new(sample_rate as f32),
        cmd_rx,
        tel_tx,
        tel_divider: 0,
    };

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

    stream.play().map_err(|e| format!("Audio play failed: {e}"))?;
    Ok((stream, format!("cpal {:.0} Hz ({} ch)", sample_rate, channels)))
}

struct LeviathanApp {
    pattern: Box<Pattern>,
    history: EditHistory,
    inspector_drag_recorded: bool,
    playing: bool,
    fill_active: bool,
    bpm: f64,
    cursor: TrackerCursor,
    sampler_ui: SamplerBankUi,
    cmd_tx: rtrb::Producer<AudioCommand>,
    tel_rx: rtrb::Consumer<CvFrame>,
    _audio_stream: Option<cpal::Stream>,
    offline_eval: Option<TrackerEvaluator>,
    audio_status: String,
    latest_cv: CvFrame,
    viewport: FeedbackViewport,
}

impl LeviathanApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let viewport = FeedbackViewport::new(cc);

        let initial_pattern = starter_electro_pattern();
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::<AudioCommand>::new(256);
        let (tel_tx, tel_rx) = rtrb::RingBuffer::<CvFrame>::new(64);

        let (_audio_stream, offline_eval, audio_status) =
            match try_start_audio(initial_pattern.clone(), cmd_rx, tel_tx) {
                Ok((stream, label)) => (Some(stream), None, label),
                Err(label) => (
                    None,
                    Some(TrackerEvaluator::new(EngineConfig {
                        sample_rate: 60.0,
                        ..EngineConfig::default()
                    })),
                    label,
                ),
            };

        let mut cursor = TrackerCursor::default();
        cursor.select(4, 0, SubColumn::Note, false);

        Self {
            pattern: Box::new(initial_pattern),
            history: EditHistory::default(),
            inspector_drag_recorded: false,
            playing: false,
            fill_active: false,
            bpm: 132.0,
            cursor,
            sampler_ui: SamplerBankUi::default(),
            cmd_tx,
            tel_rx,
            _audio_stream,
            offline_eval,
            audio_status,
            latest_cv: CvFrame::default(),
            viewport,
        }
    }

    fn push_full_pattern(&mut self) {
        let snap = self.history.make_rt_snapshot(&self.pattern);
        let _ = self.cmd_tx.push(AudioCommand::SetPattern(snap));
    }
}

impl eframe::App for LeviathanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !ctx.input(|i| i.pointer.any_down()) {
            self.inspector_drag_recorded = false;
        }

        if !ctx.wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
        {
            self.playing = !self.playing;
            let _ = self.cmd_tx.push(AudioCommand::SetPlaying(self.playing));
            if let Some(ev) = self.offline_eval.as_mut() {
                ev.playing = self.playing;
            }
        }

        match self
            .cursor
            .handle_input(ctx, &mut self.pattern, &mut self.history)
        {
            PatternSync::None => {}
            PatternSync::Single(upd) => {
                let _ = self.cmd_tx.push(AudioCommand::SetCell {
                    lane: upd.lane,
                    row: upd.row,
                    cell: upd.cell,
                });
            }
            PatternSync::FullPattern => {
                self.push_full_pattern();
            }
        }

        let (hovering_wav, mut bank_actions) = self.sampler_ui.poll_drag_and_drop(ctx);

        while let Ok(cv) = self.tel_rx.pop() {
            self.latest_cv = cv;
        }
        if let Some(ev) = self.offline_eval.as_mut() {
            ev.config.bpm = self.bpm;
            ev.fill_active = self.fill_active;
            self.latest_cv = ev.step_sample(&self.pattern);
        }

        let tex_id = self.viewport.render_frame(ctx, &self.latest_cv);

        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("LEVIATHAN");
                ui.separator();
                let play_label = if self.playing {
                    "Stop [Space]"
                } else {
                    "Play [Space]"
                };
                if ui.button(play_label).clicked() {
                    self.playing = !self.playing;
                    let _ = self.cmd_tx.push(AudioCommand::SetPlaying(self.playing));
                    if let Some(ev) = self.offline_eval.as_mut() {
                        ev.playing = self.playing;
                    }
                }
                if ui.button("Reset").clicked() {
                    let _ = self.cmd_tx.push(AudioCommand::Reset);
                    if let Some(ev) = self.offline_eval.as_mut() {
                        ev.reset_transport();
                    }
                }
                if ui.selectable_label(self.fill_active, "FILL").clicked() {
                    self.fill_active = !self.fill_active;
                    let _ = self.cmd_tx.push(AudioCommand::SetFill(self.fill_active));
                }
                ui.separator();
                ui.label("BPM");
                if ui
                    .add(egui::DragValue::new(&mut self.bpm).range(40.0..=240.0).speed(0.5))
                    .changed()
                {
                    let _ = self.cmd_tx.push(AudioCommand::SetBpm(self.bpm));
                }
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
            });
        });

        let (sel_lane, sel_row) = (self.cursor.lane, self.cursor.row);
        let mut edit_cell = self.pattern.lanes[sel_lane][sel_row];
        let mut cell_dirty = false;

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

                    bank_actions.extend(self.sampler_ui.draw(ui, hovering_wav));

                    ui.separator();
                    let eff_machine = MachineKind::from_lane_and_lock(sel_lane, &edit_cell.lock);
                    ui.heading(format!(
                        "Step Lock — {} Row {:02X} ({})",
                        LANE_NAMES[sel_lane],
                        sel_row,
                        eff_machine.short_name()
                    ));
                    ui.small(
                        "Shift+Arrows/Click select; Ctrl+Z/Y undo/redo; Ctrl+C/X/V; Ctrl+Up/Dn transpose.",
                    );

                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut edit_cell.active, "On").changed() {
                            cell_dirty = true;
                        }
                        if ui.checkbox(&mut edit_cell.air, "Air").changed() {
                            cell_dirty = true;
                        }
                        ui.label("MIDI");
                        if ui
                            .add(egui::DragValue::new(&mut edit_cell.midi).range(12..=108))
                            .changed()
                        {
                            cell_dirty = true;
                        }
                        ui.label("Vel");
                        if ui
                            .add(egui::DragValue::new(&mut edit_cell.velocity).range(1..=127))
                            .changed()
                        {
                            cell_dirty = true;
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(edit_cell.lock.machine < 0, "Lane")
                            .clicked()
                        {
                            edit_cell.lock.machine = -1;
                            cell_dirty = true;
                        }
                        for m in ALL_MACHINES {
                            let code = m as i16;
                            if ui
                                .selectable_label(edit_cell.lock.machine == code, m.short_name())
                                .clicked()
                            {
                                edit_cell.lock.machine = code;
                                cell_dirty = true;
                            }
                        }
                    });

                    let labels = param_labels(eff_machine);
                    for (slot, &name) in labels.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut locked = ((edit_cell.lock.mask >> slot) & 1) != 0;
                            if ui.checkbox(&mut locked, "").changed() {
                                if locked {
                                    edit_cell.lock.mask |= 1 << slot;
                                } else {
                                    edit_cell.lock.mask &= !(1 << slot);
                                }
                                cell_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut edit_cell.lock.values[slot], 0.0..=1.0)
                                        .text(name),
                                )
                                .changed()
                            {
                                edit_cell.lock.mask |= 1 << slot;
                                cell_dirty = true;
                            }
                        });
                    }
                });
            });

        if cell_dirty {
            if !self.inspector_drag_recorded {
                self.history.record_before_edit(&self.pattern);
                if ctx.input(|i| i.pointer.any_down()) {
                    self.inspector_drag_recorded = true;
                }
            }
            self.pattern.lanes[sel_lane][sel_row] = edit_cell;
            let _ = self.cmd_tx.push(AudioCommand::SetCell {
                lane: sel_lane,
                row: sel_row,
                cell: edit_cell,
            });
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
            };
            let _ = self.cmd_tx.push(cmd);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(
                "Pattern 00 — Arrows/Shift+Arrows select | Z-M/Q-U piano | 00-7F vel | Ctrl+Z/Y | Ctrl+C/X/V | Ctrl+Up/Dn",
            );
            ui.horizontal_wrapped(|ui| {
                let undo_n = self.history.undo_len();
                let redo_n = self.history.redo_len();
                if ui
                    .add_enabled(undo_n > 0, egui::Button::new(format!("Undo ({undo_n})")))
                    .on_hover_text("Ctrl+Z / Cmd+Z")
                    .clicked()
                    && self.history.undo(&mut self.pattern)
                {
                    self.cursor.last_edit_msg = "Undo".to_string();
                    self.push_full_pattern();
                }
                if ui
                    .add_enabled(redo_n > 0, egui::Button::new(format!("Redo ({redo_n})")))
                    .on_hover_text("Ctrl+Y / Ctrl+Shift+Z")
                    .clicked()
                    && self.history.redo(&mut self.pattern)
                {
                    self.cursor.last_edit_msg = "Redo".to_string();
                    self.push_full_pattern();
                }
                ui.separator();
                if ui.button("Copy").on_hover_text("Ctrl+C").clicked() {
                    self.cursor.copy_selection(&self.pattern);
                }
                if ui.button("Cut").on_hover_text("Ctrl+X").clicked() {
                    self.cursor.cut_selection(&mut self.pattern, &mut self.history);
                    self.push_full_pattern();
                }
                if ui
                    .add_enabled(self.cursor.clipboard.is_some(), egui::Button::new("Paste"))
                    .on_hover_text("Ctrl+V")
                    .clicked()
                    && self.cursor.paste_selection(&mut self.pattern, &mut self.history)
                {
                    self.push_full_pattern();
                }
                ui.separator();
                for (label, semi, tip) in [
                    ("-12", -12, "Ctrl+Shift+Down (-1 octave)"),
                    ("-1", -1, "Ctrl+Down (-1 semitone)"),
                    ("+1", 1, "Ctrl+Up (+1 semitone)"),
                    ("+12", 12, "Ctrl+Shift+Up (+1 octave)"),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        self.cursor
                            .transpose_selection(&mut self.pattern, &mut self.history, semi);
                        self.push_full_pattern();
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

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("score_v3_grid")
                    .striped(true)
                    .min_col_width(82.0)
                    .show(ui, |ui| {
                        ui.monospace("ROW");
                        for name in LANE_NAMES {
                            ui.monospace(name);
                        }
                        ui.end_row();

                        let active_row = self.latest_cv.row;
                        for row in 0..ROWS_PER_PATTERN {
                            let row_prefix = if self.playing && row == active_row {
                                format!("> {:02X}", row)
                            } else {
                                format!("  {:02X}", row)
                            };
                            ui.monospace(row_prefix);

                            for lane in 0..NUM_LANES {
                                let cell = self.pattern.lanes[lane][row];
                                let has_lock = cell.lock.mask != 0 || cell.lock.machine >= 0;
                                let mark = if has_lock { '*' } else { ' ' };
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
                                    } else if cell.active {
                                        egui::Color32::from_rgb(220, 235, 255)
                                    } else {
                                        egui::Color32::from_rgb(125, 130, 140)
                                    },
                                );
                                let mut vel_rt = egui::RichText::new(vel_str).monospace().color(
                                    if in_block {
                                        egui::Color32::from_rgb(175, 240, 205)
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
                                        self.history.record_before_edit(&self.pattern);
                                        let target = &mut self.pattern.lanes[lane][row];
                                        target.active = !target.active;
                                        let updated = *target;
                                        let _ = self.cmd_tx.push(AudioCommand::SetCell {
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

        if self.playing {
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
        "Leviathan — Rust A/V Tracker",
        options,
        Box::new(|cc| Ok(Box::new(LeviathanApp::new(cc)))),
    )
}
