use eframe::egui;
use leviathan_dsp::{BenderParams, DelayMode, MasterPeaks, BENDER_MAX_SHIFT_HZ};

const LEVEL_FLOOR: f32 = 0.001;
const CLIP_HOLD_SECONDS: f64 = 1.0;

pub fn draw_delay_mode(ui: &mut egui::Ui, mode: &mut DelayMode) -> bool {
    let before = *mode;
    let label = match mode {
        DelayMode::Slapback => "Slapback",
        DelayMode::LongPingPong => "Long Ping-Pong",
    };
    egui::ComboBox::from_label("Delay mode")
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(mode, DelayMode::Slapback, "Slapback");
            ui.selectable_value(mode, DelayMode::LongPingPong, "Long Ping-Pong")
                .on_hover_text("Fixed 125 / 166.7 ms taps through 96 kHz");
        })
        .response
        .on_hover_text("Switching modes clears the delay tail.");
    *mode != before
}

pub fn draw_bender(ui: &mut egui::Ui, params: &mut BenderParams) -> bool {
    let before = *params;
    egui::CollapsingHeader::new("BEND").default_open(true).show(ui, |ui| {
        let mut shift_hz = params.shift_hz();
        if ui
            .add(
                egui::Slider::new(&mut shift_hz, -BENDER_MAX_SHIFT_HZ..=BENDER_MAX_SHIFT_HZ)
                    .clamping(egui::SliderClamping::Edits)
                    .step_by(1.0)
                    .suffix(" Hz")
                    .text("Shift"),
            )
            .changed()
        {
            params.shift_norm = 0.5 * (shift_hz / BENDER_MAX_SHIFT_HZ + 1.0);
        }
        ui.add(egui::Slider::new(&mut params.feedback, 0.0..=1.0).text("Feedback"));
        ui.add(egui::Slider::new(&mut params.decay, 0.0..=1.0).text("Tank Decay"));
        ui.checkbox(&mut params.freeze, "Freeze")
            .on_hover_text("Hold the current tail, or capture the start of the next send if the tank is empty.");
    });
    *params != before
}

#[derive(Default)]
pub struct MasterMeters {
    levels: [f32; 2],
    clip_until: [f64; 2],
    last_update: f64,
}

impl MasterMeters {
    pub fn update(&mut self, peaks: MasterPeaks, now: f64) {
        let decay = (-7.0 * (now - self.last_update).max(0.0) as f32).exp();
        self.last_update = now;
        for channel in 0..2 {
            let level = (self.levels[channel] * decay).max(peaks.output[channel]);
            self.levels[channel] = if level >= LEVEL_FLOOR { level } else { 0.0 };
            if peaks.clipped[channel] {
                self.clip_until[channel] = now + CLIP_HOLD_SECONDS;
            }
        }
    }

    pub fn is_active(&self, now: f64) -> bool {
        self.levels.iter().any(|&level| level > 0.0) || self.clip_until.iter().any(|&end| end > now)
    }

    pub fn draw(&self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            for (channel, label) in ["L", "R"].into_iter().enumerate() {
                let level = self.levels[channel];
                let db = 20.0 * level.max(LEVEL_FLOOR).log10();
                let fill = if self.clip_until[channel] > now {
                    egui::Color32::LIGHT_RED
                } else {
                    egui::Color32::from_rgb(105, 193, 153)
                };
                ui.label(label);
                ui.add(
                    egui::ProgressBar::new((db + 60.0) / 60.0)
                        .desired_width(42.0)
                        .desired_height(9.0)
                        .fill(fill),
                )
                .on_hover_text(format!("{label} output peak {db:.1} dBFS"));
            }
            let label = egui::RichText::new("CLIP").small();
            let label = if self.clip_until.iter().any(|&end| end > now) {
                label.color(egui::Color32::LIGHT_RED)
            } else {
                label.weak()
            };
            ui.label(label)
                .on_hover_text("The mix exceeded 0 dBFS before the limiter");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_peaks_across_packets_and_holds_clip_after_levels_decay() {
        let mut peaks = MasterPeaks {
            output: [0.98, 0.23],
            clipped: [true, false],
        };
        peaks.merge(MasterPeaks {
            output: [0.02, 0.6],
            clipped: [false, false],
        });
        let mut meters = MasterMeters::default();
        meters.update(peaks, 5.0);
        assert_eq!(meters.levels, [0.98, 0.6]);
        assert_eq!(meters.clip_until, [6.0, 0.0]);

        meters.update(MasterPeaks::default(), 5.5);
        assert!(meters.levels[0] < 0.1 && meters.is_active(5.5));
        meters.update(MasterPeaks::default(), 7.0);
        assert!(!meters.is_active(7.0));
    }
}
