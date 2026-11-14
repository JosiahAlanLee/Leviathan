use std::path::PathBuf;

use eframe::egui::{self, Key, Modifiers};
use leviathan_core::{
    LaunchTarget, Pattern, PatternChain, PlaybackRate, SongStatus, MAX_CHAIN_ENTRIES,
    ROWS_PER_PATTERN,
};

use crate::export_ui::ExportUi;
use crate::project::PATTERN_BANK_CAPACITY;

pub enum ProjectAction {
    Open(PathBuf),
    Save(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileOperation {
    Open,
    Save,
}

struct FileDialog {
    operation: FileOperation,
    path: String,
    focus_path: bool,
}

#[derive(Default)]
pub struct ProjectUi {
    current: Option<PathBuf>,
    dialog: Option<FileDialog>,
    status: String,
    failed: bool,
}

impl ProjectUi {
    fn choose(&mut self, operation: FileOperation) {
        let path = match (&self.current, operation) {
            (Some(path), _) => path.to_string_lossy().into_owned(),
            (None, FileOperation::Save) => "untitled.leviathan.json".to_string(),
            (None, FileOperation::Open) => String::new(),
        };
        self.dialog = Some(FileDialog {
            operation,
            path,
            focus_path: true,
        });
    }

    pub fn toolbar(&mut self, ui: &mut egui::Ui, export: &mut ExportUi) -> Option<ProjectAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            let (open, save_as, save) = if self.dialog.is_none() && !export.dialog_open() {
                ui.input_mut(|input| {
                    (
                        input.consume_key(Modifiers::COMMAND, Key::O),
                        input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
                        input.consume_key(Modifiers::COMMAND, Key::S),
                    )
                })
            } else {
                (false, false, false)
            };
            let open_clicked = ui.button("Open…").on_hover_text("Ctrl/Cmd+O").clicked();
            let save_clicked = ui.button("Save").on_hover_text("Ctrl/Cmd+S").clicked();
            let save_as_clicked = ui
                .button("Save As…")
                .on_hover_text("Ctrl/Cmd+Shift+S")
                .clicked();
            if open || open_clicked {
                self.choose(FileOperation::Open);
            } else if save_as || save_as_clicked {
                self.choose(FileOperation::Save);
            } else if save || save_clicked {
                if let Some(path) = &self.current {
                    action = Some(ProjectAction::Save(path.clone()));
                } else {
                    self.choose(FileOperation::Save);
                }
            }
            export.toolbar(ui, self.dialog.is_none());
            if !self.status.is_empty() {
                if self.failed {
                    ui.colored_label(ui.visuals().error_fg_color, &self.status);
                } else {
                    ui.label(&self.status);
                }
            }
        });
        action
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn show_dialog(&mut self, ctx: &egui::Context) -> Option<ProjectAction> {
        let dialog = self.dialog.as_mut()?;
        let label = match dialog.operation {
            FileOperation::Open => "Open Project",
            FileOperation::Save => "Save Project",
        };
        let modal = egui::Modal::new(egui::Id::new("project_path")).show(ctx, |ui| {
            ui.heading(label);
            let response = ui.add(
                egui::TextEdit::singleline(&mut dialog.path)
                    .desired_width(400.0)
                    .hint_text("Path to a .leviathan.json file"),
            );
            if std::mem::take(&mut dialog.focus_path) {
                response.request_focus();
            }
            ui.horizontal(|ui| {
                let valid = !dialog.path.trim().is_empty();
                let confirmed = ui.add_enabled(valid, egui::Button::new(label)).clicked()
                    || (valid && ui.input(|input| input.key_pressed(Key::Enter)));
                (confirmed, ui.button("Cancel").clicked())
            })
            .inner
        });
        let (confirmed, cancelled) = modal.inner;
        if confirmed {
            let dialog = self.dialog.take()?;
            let path = PathBuf::from(dialog.path.trim());
            return Some(match dialog.operation {
                FileOperation::Open => ProjectAction::Open(path),
                FileOperation::Save => {
                    let mut path = path.into_os_string();
                    if !path.to_string_lossy().ends_with(".leviathan.json") {
                        path.push(".leviathan.json");
                    }
                    ProjectAction::Save(PathBuf::from(path))
                }
            });
        }
        if cancelled || modal.should_close() {
            self.dialog = None;
        }
        None
    }

    pub fn finish(&mut self, path: PathBuf, verb: &str, result: Result<(), String>) {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        match result {
            Ok(()) => {
                self.status = format!("{verb} {name}");
                self.failed = false;
                self.current = Some(path);
            }
            Err(error) => {
                self.status = format!("{name} — {error}");
                self.failed = true;
            }
        }
    }
}

pub enum ArrangementAction {
    Select(usize),
    AddPattern { clone: bool },
    SetLength(usize),
    SetRate(PlaybackRate),
    SetSwing(f32),
    Launch(LaunchTarget),
    AppendToChain,
    PopChain,
    ClearChain,
}

fn rate_label(rate: PlaybackRate) -> &'static str {
    match rate {
        PlaybackRate::Quarter => "¼×",
        PlaybackRate::Half => "½×",
        PlaybackRate::OneX => "1×",
        PlaybackRate::TwoX => "2×",
        PlaybackRate::FourX => "4×",
        PlaybackRate::EightX => "8×",
    }
}

pub fn draw_pattern_bar(
    ui: &mut egui::Ui,
    patterns: &[Pattern],
    selected: usize,
    playing: bool,
) -> Option<ArrangementAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(selected > 0, egui::Button::new("Prev"))
            .clicked()
        {
            action = Some(ArrangementAction::Select(selected - 1));
        }
        egui::ComboBox::from_id_salt("pattern_bank")
            .selected_text(format!("Pattern {selected:02X}"))
            .show_ui(ui, |ui| {
                for index in 0..patterns.len() {
                    if ui
                        .selectable_label(index == selected, format!("{index:02X}"))
                        .clicked()
                    {
                        action = Some(ArrangementAction::Select(index));
                    }
                }
            });
        if ui
            .add_enabled(selected + 1 < patterns.len(), egui::Button::new("Next"))
            .clicked()
        {
            action = Some(ArrangementAction::Select(selected + 1));
        }
        let room = patterns.len() < PATTERN_BANK_CAPACITY;
        if ui
            .add_enabled(room, egui::Button::new("New Pattern"))
            .clicked()
        {
            action = Some(ArrangementAction::AddPattern { clone: false });
        }
        if ui.add_enabled(room, egui::Button::new("Clone")).clicked() {
            action = Some(ArrangementAction::AddPattern { clone: true });
        }
        let mut rows = patterns[selected].length;
        if ui
            .add(
                egui::DragValue::new(&mut rows)
                    .range(1..=ROWS_PER_PATTERN)
                    .prefix("Rows "),
            )
            .on_hover_text("Active rows in the selected pattern")
            .changed()
        {
            action = Some(ArrangementAction::SetLength(rows));
        }
        let current_rate = patterns[selected].rate;
        egui::ComboBox::from_id_salt("pattern_rate")
            .selected_text(format!("Rate {}", rate_label(current_rate)))
            .show_ui(ui, |ui| {
                for rate in [
                    PlaybackRate::Quarter,
                    PlaybackRate::Half,
                    PlaybackRate::OneX,
                    PlaybackRate::TwoX,
                    PlaybackRate::FourX,
                    PlaybackRate::EightX,
                ] {
                    if ui
                        .selectable_label(rate == current_rate, rate_label(rate))
                        .clicked()
                        && rate != current_rate
                    {
                        action = Some(ArrangementAction::SetRate(rate));
                    }
                }
            });
        let mut swing = patterns[selected].swing * 100.0;
        if ui
            .add(
                egui::Slider::new(&mut swing, 50.0..=leviathan_core::MAX_SWING * 100.0)
                    .clamping(egui::SliderClamping::Edits)
                    .step_by(0.5)
                    .suffix("%")
                    .text("Swing"),
            )
            .on_hover_text("Delay odd rows while keeping the pattern length fixed")
            .changed()
        {
            action = Some(ArrangementAction::SetSwing(swing / 100.0));
        }
        let launch = if playing {
            "Queue Pattern"
        } else {
            "Cue Pattern"
        };
        if ui.button(launch).clicked() {
            action = Some(ArrangementAction::Launch(LaunchTarget::Pattern(selected)));
        }
    });
    action
}

pub fn draw_song(
    ui: &mut egui::Ui,
    patterns: &[Pattern],
    selected: usize,
    chain: &PatternChain,
    status: SongStatus,
    playing: bool,
) -> Option<ArrangementAction> {
    let entries = chain.as_slice();
    let mut action = None;
    ui.heading("Song chain");
    ui.horizontal_wrapped(|ui| {
        let launch = if playing { "Queue Chain" } else { "Cue Chain" };
        if ui
            .add_enabled(!entries.is_empty(), egui::Button::new(launch))
            .on_hover_text(
                "Start at the first chain entry, or queue it at the next pattern boundary",
            )
            .clicked()
        {
            action = Some(ArrangementAction::Launch(LaunchTarget::Chain));
        }
        if ui
            .add_enabled(
                entries.len() < MAX_CHAIN_ENTRIES,
                egui::Button::new(format!("Add {selected:02X}")),
            )
            .clicked()
        {
            action = Some(ArrangementAction::AppendToChain);
        }
        if ui
            .add_enabled(!entries.is_empty(), egui::Button::new("Remove Last"))
            .clicked()
        {
            action = Some(ArrangementAction::PopChain);
        }
        if ui
            .add_enabled(!entries.is_empty(), egui::Button::new("Clear"))
            .clicked()
        {
            action = Some(ArrangementAction::ClearChain);
        }
    });
    if entries.is_empty() {
        ui.label("Add a pattern to start the chain.");
        return action;
    }
    let mut total = 0.0;
    egui::Grid::new("song_entries")
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Entry");
            ui.strong("Pattern");
            ui.strong("Starts (beat)");
            ui.strong("Beats");
            ui.end_row();
            for (entry, &id) in entries.iter().enumerate() {
                let index = usize::from(id);
                let Some(pattern) = patterns.get(index) else {
                    continue;
                };
                let current = status.chain_entry == Some(entry);
                ui.monospace(format!(
                    "{}{:02}",
                    if current { "▶ " } else { "  " },
                    entry + 1
                ));
                if ui
                    .selectable_label(index == selected, format!("{id:02X}"))
                    .on_hover_text("Select this pattern for editing")
                    .clicked()
                {
                    action = Some(ArrangementAction::Select(index));
                }
                ui.monospace(format!("{total:.2}"));
                let length = pattern.duration_beats();
                ui.monospace(format!("{length:.2}"));
                ui.end_row();
                total += length;
            }
        });
    ui.small(format!("{} entries · {total:.2} beats", entries.len()));
    action
}
