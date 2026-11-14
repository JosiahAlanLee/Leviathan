use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::export::{ExportJob, ExportRequest, ExportSettings, ExportSource, SAMPLE_RATES};
use crate::project::Project;
use crate::wav::WavEncoding;

#[derive(Default)]
pub struct ExportUi {
    settings: ExportSettings,
    path: String,
    dialog: bool,
    focus_path: bool,
    job: Option<ExportJob>,
    result: Option<Result<PathBuf, String>>,
}

impl ExportUi {
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.job.as_ref().is_some_and(ExportJob::is_finished) {
            if let Some(job) = self.job.take() {
                self.result = Some(job.finish().map_err(|error| error.to_string()));
            }
        }
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, available: bool) {
        if let Some(job) = &self.job {
            let progress = job.progress();
            let label = if progress < 1.0 {
                format!("Exporting {:.0}%", progress * 100.0)
            } else {
                "Finalizing WAV export".to_string()
            };
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(152.0)
                    .text(label),
            );
        } else if ui
            .add_enabled(available, egui::Button::new("Export WAV…"))
            .clicked()
        {
            if self.path.is_empty() {
                self.path = default_path(self.settings.stems).to_string();
            }
            self.dialog = true;
            self.focus_path = true;
        }
        match &self.result {
            Some(Ok(path)) => {
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                ui.label(format!("Exported {name}"))
                    .on_hover_text(path.display().to_string());
            }
            Some(Err(error)) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Export failed — {error}"),
                );
            }
            None => {}
        }
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog
    }

    pub fn show_dialog(
        &mut self,
        ctx: &egui::Context,
        selected: usize,
        has_chain: bool,
    ) -> Option<ExportRequest> {
        if !self.dialog {
            return None;
        }
        if !has_chain {
            self.settings.source = ExportSource::Pattern;
        }
        let modal = egui::Modal::new(egui::Id::new("wav_export")).show(ctx, |ui| {
            ui.heading("Export WAV");
            egui::Grid::new("wav_export_settings")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Source");
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.settings.source,
                            ExportSource::Pattern,
                            format!("Pattern {selected:02X}"),
                        );
                        ui.add_enabled_ui(has_chain, |ui| {
                            ui.selectable_value(
                                &mut self.settings.source,
                                ExportSource::Chain,
                                "Song chain",
                            );
                        });
                    });
                    ui.end_row();

                    ui.label("Format");
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.settings.encoding,
                            WavEncoding::Pcm16,
                            "16-bit PCM",
                        );
                        ui.selectable_value(
                            &mut self.settings.encoding,
                            WavEncoding::Float32,
                            "32-bit float",
                        );
                    });
                    ui.end_row();

                    ui.label("Sample rate");
                    ui.horizontal(|ui| {
                        for (rate, label) in SAMPLE_RATES
                            .into_iter()
                            .zip(["44.1 kHz", "48 kHz", "96 kHz"])
                        {
                            ui.selectable_value(&mut self.settings.sample_rate, rate, label);
                        }
                    });
                    ui.end_row();

                    ui.label("Loops");
                    ui.add(egui::DragValue::new(&mut self.settings.loops).range(1..=u32::MAX));
                    ui.end_row();

                    ui.label("Tail");
                    let max_tail = f64::from(self.settings.encoding.max_frames())
                        / f64::from(self.settings.sample_rate);
                    ui.add(
                        egui::DragValue::new(&mut self.settings.tail_seconds)
                            .range(0.0..=max_tail)
                            .speed(0.05_f64)
                            .suffix(" s"),
                    );
                    ui.end_row();
                });

            let stems_before = self.settings.stems;
            if ui
                .checkbox(&mut self.settings.stems, "Include stems")
                .on_hover_text("Exports each lane and a shared BEND return when a lane sends to it.")
                .changed()
                && self.path == default_path(stems_before)
            {
                self.path = default_path(self.settings.stems).to_string();
            }
            ui.label(if self.settings.stems {
                "New folder path"
            } else {
                "WAV file path"
            });
            let path = ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(400.0));
            if std::mem::take(&mut self.focus_path) {
                path.request_focus();
            }
            let enter = (path.has_focus() || path.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                let valid = !self.path.trim().is_empty();
                let confirmed = ui.add_enabled(valid, egui::Button::new("Export")).clicked()
                    || (valid && enter);
                (confirmed, ui.button("Cancel").clicked())
            })
            .inner
        });
        let (confirmed, cancelled) = modal.inner;
        if confirmed {
            self.dialog = false;
            let mut destination = PathBuf::from(self.path.trim());
            if !self.settings.stems
                && !destination
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
            {
                let mut name = destination.into_os_string();
                name.push(".wav");
                destination = PathBuf::from(name);
            }
            return Some(ExportRequest {
                settings: self.settings,
                destination,
            });
        }
        if cancelled || modal.should_close() {
            self.dialog = false;
        }
        None
    }

    pub fn start(
        &mut self,
        ctx: &egui::Context,
        project: Project,
        fill: bool,
        request: ExportRequest,
    ) {
        self.result = None;
        match ExportJob::start(project, fill, request) {
            Ok(job) => {
                self.job = Some(job);
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Err(error) => self.result = Some(Err(error.to_string())),
        }
    }
}

fn default_path(stems: bool) -> &'static str {
    if stems {
        "leviathan-stems"
    } else {
        "leviathan.wav"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{MidiEvent, MidiKey};
    use crate::LeviathanApp;
    use eframe::egui::{Event, Key, Modifiers, RawInput};

    #[test]
    fn opening_export_dialog_blocks_midi_step_recording_but_keeps_transport() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app =
            LeviathanApp::with_audio(&cc, |_, _, _, _, _| Err("Offline test clock".into()));
        app.export_ui.dialog = true;
        app.midi.record_armed = true;
        let (lane, row) = (app.cursor.lane, app.cursor.row);
        let before = app.patterns[0].lanes[lane][row];
        app.midi.inject(MidiEvent::NoteOn {
            key: MidiKey {
                channel: 0,
                note: 67,
            },
            velocity: 91,
        });
        app.midi.inject(MidiEvent::Start);

        let _ = ctx.run(RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut eframe::Frame::_new_kittest());
        });

        assert_eq!(app.patterns[0].lanes[lane][row], before);
        assert_eq!(app.cursor.row, row);
        assert!(app.export_ui.dialog_open());
        assert!(app.playing);
    }

    #[test]
    fn export_dialog_submits_the_path_and_selected_options() {
        let ctx = egui::Context::default();
        let mut dialog = ExportUi {
            settings: ExportSettings {
                source: ExportSource::Chain,
                encoding: WavEncoding::Float32,
                sample_rate: 96_000,
                loops: 3,
                tail_seconds: 1.25,
                stems: false,
            },
            dialog: true,
            focus_path: true,
            ..ExportUi::default()
        };
        let enter = Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let mut request = None;
        for events in [
            vec![],
            vec![Event::Text("chain-bounce".to_string())],
            vec![enter],
        ] {
            let _ = ctx.run(
                RawInput {
                    events,
                    ..RawInput::default()
                },
                |ctx| {
                    request = dialog.show_dialog(ctx, 2, true);
                },
            );
        }
        let request = request.expect("Enter in the path field submits the export");
        assert_eq!(request.destination, PathBuf::from("chain-bounce.wav"));
        assert!(request.settings.source == ExportSource::Chain);
        assert_eq!(request.settings.encoding, WavEncoding::Float32);
        assert_eq!(request.settings.sample_rate, 96_000);
        assert_eq!(request.settings.loops, 3);
        assert_eq!(request.settings.tail_seconds, 1.25);
        assert!(!dialog.dialog_open());
    }
}
