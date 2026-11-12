use eframe::egui;

use crate::inspector_ui::LANE_NAMES;
use crate::lane_mix::LaneMix;

pub fn draw(ui: &mut egui::Ui, mix: &mut LaneMix) {
    egui::CollapsingHeader::new("Lane Mixer").show(ui, |ui| {
        egui::Grid::new("lane_mixer").striped(true).show(ui, |ui| {
            for label in ["Lane", "Level", "Pan", "Delay Send"] {
                ui.strong(label);
            }
            ui.end_row();
            for (name, channel) in LANE_NAMES.iter().zip(&mut mix.channels) {
                let channel = &mut channel.0;
                ui.monospace(*name);
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
                ui.end_row();
            }
        });
    });
}
