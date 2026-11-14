use eframe::egui;
use leviathan_core::rack_routing::{ByteLookup, LaneInput, RackRouting};
use leviathan_core::NUM_LANES;
use leviathan_dsp::{CaptureTap, LanePatch, MachineKind};

use crate::capture::{Stage, Take, MAX_CAPTURE_BEATS};

pub struct RackPanel {
    source: CaptureTap,
    beats: u8,
    pub stage: Stage,
    pub beats_left: Option<f64>,
    pub status: &'static str,
    pub routing_error: bool,
}

impl Default for RackPanel {
    fn default() -> Self {
        Self {
            source: CaptureTap::Lane(0),
            beats: 4,
            stage: Stage::Idle,
            beats_left: None,
            status: "",
            routing_error: false,
        }
    }
}

pub enum CaptureAction {
    Arm(CaptureTap, u8),
    Cancel,
}

impl RackPanel {
    pub fn busy_text(&self, playing: bool) -> String {
        match (self.stage, self.beats_left, playing) {
            (Stage::Recording, Some(beats), _) => format!("Recording · {beats:.1} beats left"),
            (Stage::Recording, None, _) => "Recording".into(),
            (_, Some(beats), true) => format!("Armed · starts in {beats:.1} beats"),
            (_, None, true) => "Armed for next beat".into(),
            _ => "Armed · waiting for Play".into(),
        }
    }

    pub fn draw_capture(
        &mut self,
        ui: &mut egui::Ui,
        take: Option<&Take>,
        has_audio: bool,
        busy: bool,
        playing: bool,
    ) -> Option<CaptureAction> {
        egui::CollapsingHeader::new("Capture sample")
            .default_open(true)
            .show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_label("Capture source")
                        .selected_text(tap_name(self.source))
                        .show_ui(ui, |ui| {
                            for lane in 0..NUM_LANES as u8 {
                                let tap = CaptureTap::Lane(lane);
                                ui.selectable_value(&mut self.source, tap, tap_name(tap));
                            }
                            ui.selectable_value(&mut self.source, CaptureTap::Bend, "BEND return");
                        });
                    ui.add(
                        egui::DragValue::new(&mut self.beats)
                            .range(1..=MAX_CAPTURE_BEATS)
                            .suffix(" beats"),
                    );
                });
                if let Some(take) = take {
                    ui.label(format!(
                        "{:.2} s stereo · {} Hz",
                        take.frames.len() as f64 / f64::from(take.rate),
                        take.rate
                    ));
                }
                if busy {
                    ui.label(self.busy_text(playing));
                    if ui.button("Cancel capture").clicked() {
                        return Some(CaptureAction::Cancel);
                    }
                } else if ui
                    .add_enabled(has_audio, egui::Button::new("Arm capture"))
                    .clicked()
                {
                    return Some(CaptureAction::Arm(self.source, self.beats));
                }
                if !self.status.is_empty() {
                    ui.label(self.status);
                }
                None
            })
            .body_returned
            .flatten()
    }
}

fn tap_name(tap: CaptureTap) -> String {
    match tap {
        CaptureTap::Lane(lane) => format!("Lane {}", lane + 1),
        CaptureTap::Bend => "BEND return".to_string(),
    }
}

pub fn draw_routing(
    ui: &mut egui::Ui,
    routing: &mut RackRouting,
    lane: usize,
    patch: LanePatch,
    has_take: bool,
    live_word: Option<u8>,
    routing_error: bool,
) {
    egui::CollapsingHeader::new("Sample and ByteBus routing").default_open(true).show(ui, |ui| {
        let input = &mut routing.inputs[lane];
        egui::ComboBox::from_id_salt(("routing-input", lane))
            .selected_text(match *input {
                LaneInput::Instrument => "Instrument".to_string(),
                LaneInput::Capture => "Capture sample".to_string(),
                LaneInput::Lane(source) => format!("Lane {}", source + 1),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(input, LaneInput::Instrument, "Instrument");
                ui.selectable_value(
                    input,
                    LaneInput::Capture,
                    "Capture sample (SMPL, OTIS, OT2, VAST)",
                );
                if patch.machine == MachineKind::VastFunSynth || matches!(input, LaneInput::Lane(_))
                {
                    for source in 0..NUM_LANES {
                        if source != lane {
                            ui.selectable_value(
                                input,
                                LaneInput::Lane(source as u8),
                                format!("Lane {} into VAST", source + 1),
                            );
                        }
                    }
                }
            });
        if has_take {
            let region = &mut routing.region;
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut region.trim_start)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .prefix("Start "),
                );
                ui.add(
                    egui::DragValue::new(&mut region.trim_end)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .prefix("End "),
                );
                ui.checkbox(&mut region.loop_enabled, "Sustain loop");
            });
            ui.add(
                egui::DragValue::new(&mut routing.frame_size)
                    .range(2..=u16::MAX)
                    .suffix(" samples per frame"),
            );
        }
        ui.checkbox(&mut routing.frame_bytebus, "Clock OTIS frames from ByteBus");
        ui.separator();
        ui.checkbox(&mut routing.bytebus.enabled, "Register → Lookup → BBUS");
        if routing.bytebus.enabled || routing.frame_bytebus {
            ui.add(egui::DragValue::new(&mut routing.bytebus.reset_word).prefix("Reset word "));
            let label = |lookup| match lookup {
                ByteLookup::Identity => "Identity",
                ByteLookup::Invert => "Invert",
                ByteLookup::Square => "Square",
                ByteLookup::Fold => "Fold",
            };
            egui::ComboBox::from_label("Lookup")
                .selected_text(label(routing.bytebus.lookup))
                .show_ui(ui, |ui| {
                    for lookup in [
                        ByteLookup::Identity,
                        ByteLookup::Invert,
                        ByteLookup::Square,
                        ByteLookup::Fold,
                    ] {
                        ui.selectable_value(&mut routing.bytebus.lookup, lookup, label(lookup));
                    }
                });
            if let Some(word) = live_word {
                ui.monospace(format!("Held {word:02X}"));
            }
        }
        if routing_error {
            ui.label("Lane routing would form a cycle");
        }
    });
}
