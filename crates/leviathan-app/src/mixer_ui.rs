use eframe::egui;
use leviathan_core::NUM_LANES;
use leviathan_dsp::LanePatch;

use crate::lane_mix::LaneMix;

pub fn draw(ui: &mut egui::Ui, mix: &mut LaneMix, patches: &[LanePatch; NUM_LANES]) {
    egui::CollapsingHeader::new("Lane Mixer").default_open(true).show(ui, |ui| {
        egui::Grid::new("lane_mixer").striped(true).show(ui, |ui| {
            for label in ["Lane", "Level", "Pan", "Delay Send", "BEND Send"] {
                ui.strong(label);
            }
            ui.end_row();

            for (lane, (patch, channel)) in patches.iter().zip(&mut mix.channels).enumerate() {
                let channel = &mut channel.0;
                ui.monospace(format!("L{} {}", lane + 1, patch.machine.short_name()));
                ui.add(
                    egui::DragValue::new(&mut channel.level)
                        .range(0.0..=1.25)
                        .speed(0.005),
                );
                ui.add(
                    egui::DragValue::new(&mut channel.pan)
                        .range(-1.0..=1.0)
                        .speed(0.01),
                );
                ui.add(
                    egui::DragValue::new(&mut channel.delay_send)
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
                ui.add(
                    egui::DragValue::new(&mut channel.fx_send)
                        .range(0.0..=1.0)
                        .speed(0.005),
                );
                ui.end_row();
            }
        });
    });
}
