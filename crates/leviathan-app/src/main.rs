use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use leviathan_core::{
    CvFrame, EngineConfig, FillCondition, NoteCell, Pattern, SoundLock, TrackerEvaluator,
    NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::{MachineKind, SynthRack};
use leviathan_gpu::{FeedbackSurface, VisualUniforms, FEEDBACK_SHADER_WGSL};

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
            "P3Aux3",
            "P4 Aux4",
            "P5 Aux5",
            "P6 Aux6",
            "P7 Aux7",
        ],
    }
}

#[derive(Clone, Copy, Debug)]
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

        // Lane 1 (FourOpFm): DualPairs glass plucks on 4/12 + sound-locked Cascade bell clangs on 9/15
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

        // Lane 4 (VarSampler): Ping-pong scanned m7 chord on even bars + down-clocked 9kHz anvil on odd bars
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
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
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
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
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
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
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
    playing: bool,
    fill_active: bool,
    bpm: f64,
    selected_step: (usize, usize),
    cmd_tx: rtrb::Producer<AudioCommand>,
    tel_rx: rtrb::Consumer<CvFrame>,
    _audio_stream: Option<cpal::Stream>,
    offline_eval: Option<TrackerEvaluator>,
    audio_status: String,
    gpu_status: String,
    latest_cv: CvFrame,
    surface: FeedbackSurface,
    preview_tex: Option<egui::TextureHandle>,
    frame_index: u32,
}

impl LeviathanApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gpu_status = if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let _shader = rs
                .device
                .create_shader_module(eframe::wgpu::ShaderModuleDescriptor {
                    label: Some("leviathan_feedback_wgsl"),
                    source: eframe::wgpu::ShaderSource::Wgsl(FEEDBACK_SHADER_WGSL.into()),
                });
            "wgpu + WGSL compiled".to_string()
        } else {
            "Software rasterizer".to_string()
        };

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

        Self {
            pattern: Box::new(initial_pattern),
            playing: false,
            fill_active: false,
            bpm: 132.0,
            selected_step: (4, 0),
            cmd_tx,
            tel_rx,
            _audio_stream,
            offline_eval,
            audio_status,
            gpu_status,
            latest_cv: CvFrame::default(),
            surface: FeedbackSurface::new(160, 90),
            preview_tex: None,
            frame_index: 0,
        }
    }
}

impl eframe::App for LeviathanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.playing = !self.playing;
            let _ = self.cmd_tx.push(AudioCommand::SetPlaying(self.playing));
            if let Some(ev) = self.offline_eval.as_mut() {
                ev.playing = self.playing;
            }
        }

        while let Ok(cv) = self.tel_rx.pop() {
            self.latest_cv = cv;
        }
        if let Some(ev) = self.offline_eval.as_mut() {
            ev.config.bpm = self.bpm;
            ev.fill_active = self.fill_active;
            self.latest_cv = ev.step_sample(&self.pattern);
        }

        let surf_w = self.surface.width;
        let surf_h = self.surface.height;
        let uniforms = VisualUniforms::from_cv(
            &self.latest_cv,
            self.frame_index,
            surf_w as u32,
            surf_h as u32,
        );
        self.frame_index = self.frame_index.wrapping_add(1);
        let rgba = self.surface.step(&uniforms);
        let color_img = egui::ColorImage::from_rgba_unmultiplied([surf_w, surf_h], rgba);
        let tex = self.preview_tex.get_or_insert_with(|| {
            ctx.load_texture(
                "feedback_surface",
                color_img.clone(),
                egui::TextureOptions::LINEAR,
            )
        });
        tex.set(color_img, egui::TextureOptions::LINEAR);
        let tex_id = tex.id();

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
                    "Beat {:06.2} | Row {:02}",
                    self.latest_cv.beat, self.latest_cv.row
                ));
                ui.separator();
                ui.small(&self.audio_status);
                ui.separator();
                ui.small(&self.gpu_status);
            });
        });

        let (sel_lane, sel_row) = self.selected_step;
        let mut edit_cell = self.pattern.lanes[sel_lane][sel_row];
        let mut cell_dirty = false;

        egui::SidePanel::right("visual_and_bus_panel")
            .min_width(372.0)
            .show(ctx, |ui| {
                ui.heading("Live Feedback Surface");
                ui.image((tex_id, egui::vec2(352.0, 198.0)));
                ui.separator();
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
                let eff_machine = MachineKind::from_lane_and_lock(sel_lane, &edit_cell.lock);
                ui.heading(format!(
                    "Step Lock — {} Row {:02X} ({})",
                    LANE_NAMES[sel_lane],
                    sel_row,
                    eff_machine.short_name()
                ));
                ui.small("Left-click grid toggles step; right-click inspects without toggling.");

                ui.horizontal(|ui| {
                    if ui.checkbox(&mut edit_cell.active, "On").changed() {
                        cell_dirty = true;
                    }
                    if ui.checkbox(&mut edit_cell.air, "Air").changed() {
                        cell_dirty = true;
                    }
                    ui.label("MIDI");
                    if ui
                        .add(egui::DragValue::new(&mut edit_cell.midi).range(24..=96))
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

        if cell_dirty {
            self.pattern.lanes[sel_lane][sel_row] = edit_cell;
            let _ = self.cmd_tx.push(AudioCommand::SetCell {
                lane: sel_lane,
                row: sel_row,
                cell: edit_cell,
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Score v3 Pattern 00 (64 Rows x 6 Lanes — * marks sound-locked steps)");
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("score_v3_grid")
                    .striped(true)
                    .min_col_width(74.0)
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
                                let cell = &mut self.pattern.lanes[lane][row];
                                let has_lock = cell.lock.mask != 0 || cell.lock.machine >= 0;
                                let sep = if has_lock { '*' } else { ' ' };
                                let label = if cell.active {
                                    if cell.air {
                                        format!("AIR{sep}--")
                                    } else {
                                        format!("{}{sep}{:02X}", midi_label(cell.midi), cell.velocity)
                                    }
                                } else if has_lock {
                                    "---*--".to_string()
                                } else {
                                    "--- --".to_string()
                                };
                                let resp = ui.selectable_label(
                                    cell.active || self.selected_step == (lane, row),
                                    label,
                                );
                                if resp.clicked() {
                                    self.selected_step = (lane, row);
                                    cell.active = !cell.active;
                                    let _ = self.cmd_tx.push(AudioCommand::SetCell {
                                        lane,
                                        row,
                                        cell: *cell,
                                    });
                                } else if resp.secondary_clicked() {
                                    self.selected_step = (lane, row);
                                }
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
        viewport: egui::ViewportBuilder::default().with_inner_size([1240.0, 780.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Leviathan — Rust A/V Tracker",
        options,
        Box::new(|cc| Ok(Box::new(LeviathanApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_pattern_populates_fm_and_sampler_sound_locks() {
        let pat = starter_electro_pattern();
        assert!(pat.lanes[0][0].active);
        assert_eq!(pat.lanes[1][4].lock.machine, MachineKind::FourOpFm as i16);
        assert_ne!(pat.lanes[1][4].lock.mask, 0);
        assert_eq!(pat.lanes[4][0].lock.machine, MachineKind::VarSampler as i16);
        assert_eq!(param_labels(MachineKind::VarSampler)[1], "P1 ChipClk");
        assert_eq!(midi_label(60), "C-4");
    }
}
