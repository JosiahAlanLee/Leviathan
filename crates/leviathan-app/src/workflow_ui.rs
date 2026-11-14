use eframe::egui;
use leviathan_core::{CvFrame, LaunchTarget, SongStatus};

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum Deck {
    Grid,
    #[default]
    Voice,
    Mixer,
    Routing,
    Samples,
    Scene,
    Song,
}

impl Deck {
    const SHORTCUTS: [(Self, egui::Key, &str); 7] = [
        (Self::Grid, egui::Key::F1, "Grid"),
        (Self::Voice, egui::Key::F2, "Voice / Trig"),
        (Self::Mixer, egui::Key::F3, "Mixer"),
        (Self::Routing, egui::Key::F4, "Routing"),
        (Self::Samples, egui::Key::F5, "Samples"),
        (Self::Scene, egui::Key::F6, "Scene"),
        (Self::Song, egui::Key::F7, "Song"),
    ];

    pub fn read_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        for (deck, key, _) in Self::SHORTCUTS {
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                *self = deck;
            }
        }
    }

    pub fn draw_tabs(&mut self, ui: &mut egui::Ui) {
        for (index, (deck, _, name)) in Self::SHORTCUTS.into_iter().enumerate() {
            if ui
                .selectable_label(*self == deck, name)
                .on_hover_text(format!("F{}", index + 1))
                .clicked()
            {
                *self = deck;
            }
        }
    }
}

pub enum TransportShortcut {
    Toggle,
    FromCursor,
}

impl TransportShortcut {
    pub fn read(ctx: &egui::Context) -> Option<Self> {
        let mut shortcut = None;
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                let egui::Event::Key {
                    key: egui::Key::Space,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                else {
                    return true;
                };
                if *modifiers == egui::Modifiers::NONE || *modifiers == egui::Modifiers::SHIFT {
                    if !repeat {
                        shortcut = Some(if modifiers.shift {
                            Self::FromCursor
                        } else {
                            Self::Toggle
                        });
                    }
                    false
                } else {
                    true
                }
            });
        });
        shortcut
    }
}

pub fn transport_status(
    ui: &mut egui::Ui,
    editing: usize,
    song: SongStatus,
    playing: bool,
    position: (f64, usize),
) {
    let (beat, row) = position;
    ui.monospace(format!("Edit {editing:02X}"));
    let state = if playing { "▶" } else { "Paused" };
    ui.monospace(format!("{state} {:02X}/{row:02X}", song.pattern))
        .on_hover_text("Transport pattern / row");
    ui.monospace(format!("Beat {beat:.2}"));
    if let Some(entry) = song.chain_entry {
        ui.monospace(format!("Chain {:02}", entry + 1));
    }
    if let Some(target) = song.queued {
        let label = match target {
            LaunchTarget::Pattern(index) => format!("Queued {index:02X}"),
            LaunchTarget::Chain => "Queued chain".to_owned(),
        };
        ui.strong(label);
    }
}

pub fn bytebus_status(ui: &mut egui::Ui, cv: &CvFrame, routed_byte: Option<u8>) {
    let word = cv.bytebus_v[..8]
        .iter()
        .enumerate()
        .fold(0u8, |word, (bit, voltage)| {
            word | (u8::from(*voltage > 5.0) << bit)
        });
    ui.monospace(format!("BUS {word:02X}"))
        .on_hover_text("Tracker ByteBus word before routing");
    for (bit, voltage) in cv.bytebus_v.iter().enumerate() {
        let label = if bit == 8 {
            "CLK".to_owned()
        } else {
            bit.to_string()
        };
        ui.label(if *voltage > 5.0 {
            egui::RichText::new(label)
                .monospace()
                .strong()
                .color(ui.visuals().selection.stroke.color)
        } else {
            egui::RichText::new(label).monospace().weak()
        });
    }
    if let Some(routed) = routed_byte {
        ui.monospace(format!("Held {routed:02X}"))
            .on_hover_text("Word held by the clocked register after lookup");
    }
}
