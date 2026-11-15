use std::path::Path;

use eframe::egui;
use leviathan_dsp::otis_kit::{FilterTaper, VelocityCurve, CHIP_RATES, SLOTS};

use crate::kit::Kit;
use crate::param_ui::format_kit_velocity;

#[derive(Default)]
pub struct KitUi {
    pub slot: usize,
    path: String,
    pub status: String,
    pub sounding: [bool; SLOTS],
}

#[derive(Default)]
pub struct Edits {
    pub samples: bool,
    pub controls: bool,
    pub use_capture: bool,
    pub audition: bool,
}

impl KitUi {
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        kit: &mut Kit,
        lane: usize,
        has_take: bool,
        busy: bool,
        has_audio: bool,
    ) -> Edits {
        let mut edits = Edits::default();
        ui.heading("OtisKit");
        ui.small(format!(
            "Velocity gain · 110 gives {} · 94 gives {}",
            format_kit_velocity(110, kit.lanes[lane].velocity),
            format_kit_velocity(94, kit.lanes[lane].velocity),
        ))
        .on_hover_text("Step Lock can edit this gain directly in dB. Slot level and mixer gain are separate.");
        let settings = &mut kit.lanes[lane];
        let old = *settings;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Lane {}", lane + 1));
            ui.selectable_value(&mut settings.bytebus, false, "Tracker");
            ui.selectable_value(&mut settings.bytebus, true, "ByteBus");
        }).response.on_hover_text("Tracker: C-4 to G-4 selects slots 1–8 at unity, each group of 8 notes shifts an octave. ByteBus: bits 0–2 select a slot, notes control pitch normally.");
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("okit_clock")
                .selected_text(format!(
                    "{:.1} kHz",
                    CHIP_RATES[usize::from(settings.clock)] / 1000.0
                ))
                .show_ui(ui, |ui| {
                    for (index, rate) in CHIP_RATES.into_iter().enumerate() {
                        ui.selectable_value(
                            &mut settings.clock,
                            index as u8,
                            format!("{:.1} kHz", rate / 1000.0),
                        );
                    }
                });
            ui.checkbox(&mut settings.drop_samples, "Drop-sample");
            egui::ComboBox::from_id_salt("okit_velocity")
                .selected_text(settings.velocity.label())
                .show_ui(ui, |ui| {
                    for curve in VelocityCurve::ALL {
                        ui.selectable_value(&mut settings.velocity, curve, curve.label());
                    }
                })
                .response
                .on_hover_text("Velocity curve for this lane. Chip: 0.375 dB per step, as the hardware. Soft: 0.125 dB per step, so ordinary tracker velocities stay audible.");
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut settings.raw_output, "Raw output")
                .on_hover_text("Use stair-step output from the chip. With Raw output off, the converter filters both upsampling and downsampling between the chip and output rates.");
            let mut squared = settings.filter_taper == FilterTaper::Squared;
            if ui
                .checkbox(&mut squared, "Squared filter taper")
                .on_hover_text("The original Leviathan filter knob: about half the cutoff at the same setting. Off is linear, like the ES5505 K1 register.")
                .changed()
            {
                settings.filter_taper = if squared { FilterTaper::Squared } else { FilterTaper::Linear };
            }
        });
        edits.controls = *settings != old;
        ui.separator();

        let changes = ui
            .scope(|ui| {
                ui.spacing_mut().interact_size.x = 30.0;
                egui::Grid::new("kit_slots")
                    .striped(true)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        for header in ["Slot", "Tune", "Decay", "Level", "Filter", "Start", "Rev"] {
                            ui.small(header);
                        }
                        ui.end_row();
                        let mut changed = false;
                        for i in 0..SLOTS {
                            let name = kit.samples[i].as_ref().map_or("Empty", |s| s.name.as_str());
                            let label = egui::RichText::new(format!("{:02}", i + 1)).monospace();
                            let label = if self.sounding[i] {
                                label.color(egui::Color32::LIGHT_GREEN)
                            } else {
                                label
                            };
                            if ui
                                .selectable_label(i == self.slot, label)
                                .on_hover_text(name)
                                .clicked()
                            {
                                self.slot = i;
                            }
                            let params = &mut kit.params[i];
                            let before = *params;
                            ui.add(
                                egui::DragValue::new(&mut params.tune)
                                    .range(-24.0..=24.0)
                                    .speed(0.15),
                            );
                            for value in [
                                &mut params.decay,
                                &mut params.level,
                                &mut params.filter,
                                &mut params.start,
                            ] {
                                ui.add(
                                    egui::DragValue::new(value)
                                        .range(0.0..=1.0)
                                        .max_decimals(2)
                                        .speed(0.008),
                                );
                            }
                            ui.checkbox(&mut params.reverse, "");
                            changed |= *params != before;
                            ui.end_row();
                        }
                        changed
                    })
                    .inner
            })
            .inner;
        edits.controls |= changes;

        ui.separator();
        if let Some(sample) = &kit.samples[self.slot] {
            ui.label(format!("Slot {} · {}", self.slot + 1, sample.name));
            ui.small(format!(
                "{:.2} s mono · {} Hz",
                sample.pcm.len() as f64 / f64::from(sample.rate),
                sample.rate
            ));
        } else {
            ui.label(format!("Slot {} · Empty", self.slot + 1));
        }
        edits.audition = ui
            .add_enabled(
                has_audio && kit.samples[self.slot].is_some(),
                egui::Button::new("Audition slot"),
            )
            .clicked();
        ui.add_enabled_ui(!busy, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .hint_text("Path to WAV or folder")
                    .desired_width(ui.available_width()),
            );
            ui.horizontal_wrapped(|ui| {
                let eligible = !self.path.trim().is_empty();
                if ui
                    .add_enabled(eligible, egui::Button::new("Load WAV"))
                    .clicked()
                {
                    self.apply(
                        kit.load_wav(self.slot, Path::new(self.path.trim())),
                        &mut edits,
                        "WAV loaded",
                    );
                }
                if ui
                    .add_enabled(eligible, egui::Button::new("Load folder"))
                    .clicked()
                {
                    self.apply(
                        kit.load_folder(Path::new(self.path.trim())),
                        &mut edits,
                        "Kit folder loaded",
                    );
                }
                if ui
                    .button("Factory kit")
                    .on_hover_text("Replace all eight slots with the factory kit")
                    .clicked()
                {
                    kit.load_factory();
                    self.apply(Ok(()), &mut edits, "Factory kit loaded");
                }
                if ui
                    .add_enabled(
                        kit.samples[self.slot].is_some(),
                        egui::Button::new("Clear slot"),
                    )
                    .clicked()
                {
                    kit.samples[self.slot] = None;
                    self.apply(Ok(()), &mut edits, "Slot cleared");
                }
                edits.use_capture = ui
                    .add_enabled(has_take, egui::Button::new("Copy capture"))
                    .clicked();
            });
        });
        if !self.status.is_empty() {
            ui.small(&self.status);
        }
        edits
    }

    fn apply(&mut self, result: Result<(), String>, edits: &mut Edits, label: &str) {
        self.status = match result {
            Ok(()) => {
                edits.samples = true;
                label.into()
            }
            Err(error) => error,
        };
    }
}
