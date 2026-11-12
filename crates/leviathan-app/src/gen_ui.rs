use std::path::PathBuf;

use eframe::egui;
use leviathan_core::PlaybackRate;

use crate::smf::{Format, Scope, GRID_RATES};
use crate::tracker_edit::{EuclideanFill, RampTarget, Scale};
use crate::tracker_keys::{GeneratorAction, TrackerCursor};

#[derive(Clone, Copy)]
pub enum MidiDestination {
    Pattern,
    AppendChain,
    OpenChain,
}

impl MidiDestination {
    pub fn scope(self) -> Scope {
        match self {
            Self::Pattern => Scope::Pattern,
            Self::AppendChain | Self::OpenChain => Scope::Chain,
        }
    }
}

pub enum MidiFileAction {
    Import {
        path: PathBuf,
        destination: MidiDestination,
        rate: PlaybackRate,
        use_tempo: bool,
    },
    Export {
        path: PathBuf,
        scope: Scope,
        format: Format,
    },
}

pub struct GenUi {
    rhythm: EuclideanFill,
    path: String,
    scope: Scope,
    rate: PlaybackRate,
    format: Format,
    use_tempo: bool,
    status: Option<Result<String, String>>,
}

impl Default for GenUi {
    fn default() -> Self {
        Self {
            rhythm: EuclideanFill::default(),
            path: String::new(),
            scope: Scope::default(),
            rate: PlaybackRate::OneX,
            format: Format::default(),
            use_tempo: true,
            status: None,
        }
    }
}

impl GenUi {
    pub fn pattern_tools(
        &mut self,
        ui: &mut egui::Ui,
        cursor: &mut TrackerCursor,
        pattern_length: usize,
    ) -> Option<GeneratorAction> {
        let mut action = None;
        egui::CollapsingHeader::new("Pattern Tools").show(ui, |ui| {
            let rect = cursor.active_rect();
            let available = rect.available_steps(pattern_length);
            self.rhythm.steps = self.rhythm.steps.clamp(1, available.max(1));
            self.rhythm.pulses = self.rhythm.pulses.min(self.rhythm.steps);
            ui.horizontal_wrapped(|ui| {
                ui.label("Euclidean");
                ui.add(
                    egui::DragValue::new(&mut self.rhythm.steps)
                        .range(1..=available.max(1))
                        .suffix(" steps"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.rhythm.pulses)
                        .range(0..=self.rhythm.steps)
                        .suffix(" pulses"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.rhythm.rotation)
                        .range(-63..=63)
                        .prefix("Rotate "),
                )
                .on_hover_text("Positive rotation moves pulses to later rows");
                if ui
                    .add_enabled(available > 0, egui::Button::new("Fill"))
                    .on_hover_text("Fill selected lanes from the top row. New hits use that row's pitch and velocity.")
                    .clicked()
                {
                    action = Some(GeneratorAction::Euclidean(self.rhythm));
                }
            });
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("ramp_target")
                    .selected_text(cursor.ramp_target.label())
                    .show_ui(ui, |ui| {
                        for target in RampTarget::ALL {
                            ui.selectable_value(&mut cursor.ramp_target, target, target.label());
                        }
                    });
                if ui
                    .add_enabled(rect.max_row - rect.min_row >= 2, egui::Button::new("Ramp"))
                    .on_hover_text("Ctrl/Cmd+I interpolates between the first and last selected rows")
                    .clicked()
                {
                    action = Some(GeneratorAction::Ramp);
                }
            });
            ui.horizontal_wrapped(|ui| {
                const ROOTS: [&str; 12] = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
                ui.label("Scale Lock")
                    .on_hover_text("Applies to keyboard note entry and MIDI step recording");
                egui::ComboBox::from_id_salt("scale_root")
                    .selected_text(ROOTS[usize::from(cursor.scale_lock.root)])
                    .show_ui(ui, |ui| {
                        for (root, label) in ROOTS.into_iter().enumerate() {
                            ui.selectable_value(&mut cursor.scale_lock.root, root as u8, label);
                        }
                    });
                egui::ComboBox::from_id_salt("scale_mode")
                    .selected_text(cursor.scale_lock.scale.label())
                    .show_ui(ui, |ui| {
                        for scale in Scale::ALL {
                            ui.selectable_value(&mut cursor.scale_lock.scale, scale, scale.label());
                        }
                    });
                if ui
                    .add_enabled(
                        cursor.scale_lock.scale != Scale::Chromatic,
                        egui::Button::new("Quantize selection"),
                    )
                    .clicked()
                {
                    action = Some(GeneratorAction::Quantize);
                }
            });
        });
        action
    }

    pub fn midi_files(&mut self, ui: &mut egui::Ui, has_chain: bool) -> Option<MidiFileAction> {
        let mut action = None;
        egui::CollapsingHeader::new("MIDI Files").show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.path).hint_text("Path to a .mid file"));
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.scope, Scope::Pattern, "Selected pattern");
                ui.selectable_value(&mut self.scope, Scope::Chain, "Song chain");
            });

            let path = self.path.trim();
            let can_export = !path.is_empty() && (self.scope == Scope::Pattern || has_chain);
            let mut import_to = None;
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("midi_import_grid")
                    .selected_text(format!("{} rows/beat", self.rate.rows_per_beat()))
                    .show_ui(ui, |ui| {
                        for rate in GRID_RATES {
                            ui.selectable_value(
                                &mut self.rate,
                                rate,
                                format!("{} rows/beat", rate.rows_per_beat()),
                            );
                        }
                    })
                    .response
                    .on_hover_text("Import grid for MIDI files without Leviathan pattern markers");
                ui.checkbox(&mut self.use_tempo, "Use file tempo");
                let (import_label, destination) = match self.scope {
                    Scope::Pattern => ("Replace pattern", MidiDestination::Pattern),
                    Scope::Chain => ("Append to chain", MidiDestination::AppendChain),
                };
                if ui
                    .add_enabled(!path.is_empty(), egui::Button::new(import_label))
                    .clicked()
                {
                    import_to = Some(destination);
                }
                if self.scope == Scope::Chain
                    && ui
                        .add_enabled(!path.is_empty(), egui::Button::new("Open as new project"))
                        .on_hover_text("Load the chain into a new pattern bank with the current audio and shader settings")
                        .clicked()
                {
                    import_to = Some(MidiDestination::OpenChain);
                }
            });
            if let Some(destination) = import_to {
                action = Some(MidiFileAction::Import {
                    path: PathBuf::from(path),
                    destination,
                    rate: self.rate,
                    use_tempo: self.use_tempo,
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.format, Format::SingleTrack, "Format 0");
                ui.selectable_value(&mut self.format, Format::MultiTrack, "Format 1");
                if ui
                    .add_enabled(can_export, egui::Button::new("Export MIDI"))
                    .on_hover_text("Write notes on the tracker grid to a new MIDI file")
                    .clicked()
                {
                    let mut path = PathBuf::from(path);
                    if path.extension().is_none() {
                        path.set_extension("mid");
                    }
                    action = Some(MidiFileAction::Export {
                        path,
                        scope: self.scope,
                        format: self.format,
                    });
                }
            });
            match &self.status {
                Some(Ok(message)) => {
                    ui.small(message);
                }
                Some(Err(error)) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                None => {}
            }
        });
        action
    }

    pub fn finish(&mut self, result: Result<String, String>) {
        self.status = Some(result);
    }
}
