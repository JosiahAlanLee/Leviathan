use std::path::Path;
use std::sync::Arc;

use eframe::egui;
use leviathan_dsp::sampler::{NUM_BANKS, ROM_WAVE_LEN};

use crate::wav::{decode_wav_bytes, load_wav_file, DecodedWav};

const ROM_LABELS: [&str; NUM_BANKS] = [
    "ROM 0: Anvil Clang",
    "ROM 1: 9-Bit Formant",
    "ROM 2: Minor-7th Chord",
    "ROM 3: Crunch Snare",
];

#[derive(Clone, Debug)]
pub enum BankAction {
    Load {
        bank: usize,
        samples: Arc<[i16]>,
        c4_rate_hz: f32,
    },
    ResetRom {
        bank: usize,
    },
}

#[derive(Clone, Debug)]
pub struct BankSlotMeta {
    pub label: String,
    pub frames: usize,
    pub sample_rate: u32,
    pub peaks: [f32; 64],
}

impl BankSlotMeta {
    fn rom_default(bank: usize) -> Self {
        let mut peaks = [0.0_f32; 64];
        for (i, slot) in peaks.iter_mut().enumerate() {
            let t = i as f32 / 64.0;
            *slot = match bank {
                0 => (1.0 - 0.75 * t) * 0.85,
                1 => if (i % 8) < 3 { 0.80 } else { 0.45 },
                2 => 0.55 + 0.30 * (t * core::f32::consts::TAU * 2.0).sin().abs(),
                _ => (1.0 - 0.80 * t) * 0.90,
            };
        }
        Self {
            label: ROM_LABELS[bank.min(NUM_BANKS - 1)].to_string(),
            frames: ROM_WAVE_LEN,
            sample_rate: 48_000,
            peaks,
        }
    }
}

pub struct SamplerBankUi {
    pub active_bank: usize,
    pub path_input: String,
    pub status_msg: String,
    pub slots: [BankSlotMeta; NUM_BANKS],
    /// Retains Arc buffers on the UI thread so the RT audio callback never deallocates.
    retained_arcs: Vec<Arc<[i16]>>,
}

impl Default for SamplerBankUi {
    fn default() -> Self {
        Self {
            active_bank: 0,
            path_input: String::new(),
            status_msg: "Drop .wav file(s) anywhere on window to load into B0-B3.".to_string(),
            slots: core::array::from_fn(BankSlotMeta::rom_default),
            retained_arcs: Vec::new(),
        }
    }
}

impl SamplerBankUi {
    fn apply_decoded(&mut self, bank: usize, wav: DecodedWav) -> BankAction {
        let b = bank % NUM_BANKS;
        self.slots[b] = BankSlotMeta {
            label: wav.name.clone(),
            frames: wav.samples.len(),
            sample_rate: wav.original_sr,
            peaks: wav.peaks,
        };
        self.status_msg = format!(
            "Loaded B{b}: {} ({} -> {} fr @ {} Hz)",
            wav.name,
            wav.original_frames,
            wav.samples.len(),
            wav.original_sr
        );
        self.retained_arcs.push(Arc::clone(&wav.samples));
        if self.retained_arcs.len() > 32 {
            self.retained_arcs.retain(|a| Arc::strong_count(a) > 1);
        }
        BankAction::Load {
            bank: b,
            samples: wav.samples,
            c4_rate_hz: wav.c4_rate_hz,
        }
    }

    pub fn poll_drag_and_drop(&mut self, ctx: &egui::Context) -> (bool, Vec<BankAction>) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        let mut actions = Vec::new();

        for (offset, df) in dropped.iter().enumerate() {
            let target_bank = (self.active_bank + offset) % NUM_BANKS;
            let res = if let Some(bytes) = df.bytes.as_deref() {
                let label = if df.name.is_empty() {
                    "dropped.wav"
                } else {
                    &df.name
                };
                decode_wav_bytes(label, bytes)
            } else if let Some(path) = df.path.as_deref() {
                load_wav_file(path)
            } else {
                Err("Dropped file had no path or bytes".to_string())
            };

            match res {
                Ok(wav) => actions.push(self.apply_decoded(target_bank, wav)),
                Err(err) => {
                    self.status_msg = format!("WAV drop error (B{target_bank}): {err}");
                }
            }
        }

        (hovering, actions)
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, hovering_file: bool) -> Vec<BankAction> {
        let mut actions = Vec::new();
        ui.separator();
        ui.heading("VarSampler Banks (WAV Drag & Drop)");

        if hovering_file {
            ui.colored_label(
                egui::Color32::from_rgb(110, 220, 255),
                format!(
                    ">> Release to load .wav into Bank B{} (multi-file fills B0-B3) <<",
                    self.active_bank
                ),
            );
        }

        ui.horizontal(|ui| {
            for b in 0..NUM_BANKS {
                if ui
                    .selectable_label(self.active_bank == b, format!("B{b}"))
                    .clicked()
                {
                    self.active_bank = b;
                }
            }
            if ui.button("Reset ROM").clicked() {
                let b = self.active_bank;
                self.slots[b] = BankSlotMeta::rom_default(b);
                self.status_msg = format!("Restored B{b} to procedural ROM wave.");
                actions.push(BankAction::ResetRom { bank: b });
            }
        });

        let meta = &self.slots[self.active_bank];
        ui.monospace(format!(
            "B{}: {} ({} fr @ {} Hz)",
            self.active_bank, meta.label, meta.frames, meta.sample_rate
        ));

        egui::Frame::canvas(ui.style()).show(ui, |ui| {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(340.0, 38.0), egui::Sense::hover());
            let rect = resp.rect;
            let mid_y = rect.center().y;
            let half_h = rect.height() * 0.44;
            let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(95, 195, 250));

            for (i, &pk) in meta.peaks.iter().enumerate() {
                let x = rect.left() + (i as f32 + 0.5) * (rect.width() / 64.0);
                let dy = (pk * half_h).max(1.0);
                painter.line_segment([egui::pos2(x, mid_y - dy), egui::pos2(x, mid_y + dy)], stroke);
            }
        });

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .desired_width(248.0)
                    .hint_text("/path/to/sample.wav"),
            );
            if ui.button("Load WAV").clicked() {
                let trimmed = self.path_input.trim().to_string();
                if !trimmed.is_empty() {
                    match load_wav_file(Path::new(&trimmed)) {
                        Ok(wav) => {
                            let b = self.active_bank;
                            actions.push(self.apply_decoded(b, wav));
                        }
                        Err(err) => {
                            self.status_msg = format!("Load failed: {err}");
                        }
                    }
                }
            }
        });

        ui.small(&self.status_msg);
        actions
    }
}
