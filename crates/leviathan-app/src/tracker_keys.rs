use eframe::egui::{self, Key, Modifiers};
use leviathan_core::{NoteCell, Pattern, SoundLock, NUM_LANES, ROWS_PER_PATTERN};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubColumn {
    Note,
    Velocity,
}

impl SubColumn {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Velocity => "VEL",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CellUpdate {
    pub lane: usize,
    pub row: usize,
    pub cell: NoteCell,
}

#[derive(Clone, Debug)]
pub struct TrackerCursor {
    pub lane: usize,
    pub row: usize,
    pub column: SubColumn,
    pub base_octave: i32,
    pub edit_step: usize,
    pub vel_high_nibble: bool,
    pub moved_this_frame: bool,
}

impl Default for TrackerCursor {
    fn default() -> Self {
        Self {
            lane: 0,
            row: 0,
            column: SubColumn::Note,
            base_octave: 4,
            edit_step: 1,
            vel_high_nibble: true,
            moved_this_frame: false,
        }
    }
}

const PIANO_BINDINGS: [(Key, i32); 29] = [
    // Lower octave (Z-M white keys, S D G H J L ; black keys)
    (Key::Z, 0),
    (Key::S, 1),
    (Key::X, 2),
    (Key::D, 3),
    (Key::C, 4),
    (Key::V, 5),
    (Key::G, 6),
    (Key::B, 7),
    (Key::H, 8),
    (Key::N, 9),
    (Key::J, 10),
    (Key::M, 11),
    (Key::Comma, 12),
    (Key::L, 13),
    (Key::Period, 14),
    (Key::Semicolon, 15),
    (Key::Slash, 16),
    // Upper octave (Q-P white keys, 2 3 5 6 7 9 0 black keys)
    (Key::Q, 12),
    (Key::Num2, 13),
    (Key::W, 14),
    (Key::Num3, 15),
    (Key::E, 16),
    (Key::R, 17),
    (Key::Num5, 18),
    (Key::T, 19),
    (Key::Num6, 20),
    (Key::Y, 21),
    (Key::Num7, 22),
    (Key::U, 23),
];

const UPPER_EXT_BINDINGS: [(Key, i32); 5] = [
    (Key::I, 24),
    (Key::Num9, 25),
    (Key::O, 26),
    (Key::Num0, 27),
    (Key::P, 28),
];

const HEX_BINDINGS: [(Key, u8); 16] = [
    (Key::Num0, 0x0),
    (Key::Num1, 0x1),
    (Key::Num2, 0x2),
    (Key::Num3, 0x3),
    (Key::Num4, 0x4),
    (Key::Num5, 0x5),
    (Key::Num6, 0x6),
    (Key::Num7, 0x7),
    (Key::Num8, 0x8),
    (Key::Num9, 0x9),
    (Key::A, 0xA),
    (Key::B, 0xB),
    (Key::C, 0xC),
    (Key::D, 0xD),
    (Key::E, 0xE),
    (Key::F, 0xF),
];

pub fn midi_for_piano_key(key: Key, base_octave: i32) -> Option<u8> {
    for &(k, semi) in PIANO_BINDINGS.iter().chain(UPPER_EXT_BINDINGS.iter()) {
        if k == key {
            let midi = ((base_octave + 1) * 12 + semi).clamp(12, 108);
            return Some(midi as u8);
        }
    }
    None
}

pub fn hex_key_to_nibble(key: Key) -> Option<u8> {
    for &(k, nib) in &HEX_BINDINGS {
        if k == key {
            return Some(nib);
        }
    }
    None
}

pub fn apply_hex_nibble(current_vel: u8, nib: u8, high_nibble: &mut bool) -> (u8, bool) {
    if *high_nibble {
        let merged = ((nib << 4) | (current_vel & 0x0F)).clamp(1, 127);
        *high_nibble = false;
        (merged, false)
    } else {
        let merged = ((current_vel & 0xF0) | (nib & 0x0F)).clamp(1, 127);
        *high_nibble = true;
        (merged, true)
    }
}

impl TrackerCursor {
    pub fn select(&mut self, lane: usize, row: usize, col: SubColumn) {
        self.lane = lane.min(NUM_LANES - 1);
        self.row = row.min(ROWS_PER_PATTERN - 1);
        self.column = col;
        self.vel_high_nibble = true;
    }

    fn advance_row(&mut self) {
        self.row = (self.row + self.edit_step) % ROWS_PER_PATTERN;
        self.vel_high_nibble = true;
        self.moved_this_frame = true;
    }

    pub fn handle_input(&mut self, ctx: &egui::Context, pattern: &mut Pattern) -> Vec<CellUpdate> {
        self.moved_this_frame = false;
        if ctx.egui_wants_keyboard_input() {
            return Vec::new();
        }

        let mut updates = Vec::new();

        ctx.input_mut(|i| {
            // Octave & step-size hotkeys
            if i.consume_key(Modifiers::NONE, Key::F1)
                || i.consume_key(Modifiers::NONE, Key::Minus)
            {
                self.base_octave = (self.base_octave - 1).clamp(1, 6);
            }
            if i.consume_key(Modifiers::NONE, Key::F2)
                || i.consume_key(Modifiers::NONE, Key::Equals)
            {
                self.base_octave = (self.base_octave + 1).clamp(1, 6);
            }
            if i.consume_key(Modifiers::NONE, Key::F3)
                || i.consume_key(Modifiers::NONE, Key::OpenBracket)
            {
                self.edit_step = self.edit_step.saturating_sub(1);
            }
            if i.consume_key(Modifiers::NONE, Key::F4)
                || i.consume_key(Modifiers::NONE, Key::CloseBracket)
            {
                self.edit_step = (self.edit_step + 1).min(16);
            }

            // Cursor navigation (consume keys so egui widget focus never jumps)
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.row = (self.row + ROWS_PER_PATTERN - 1) % ROWS_PER_PATTERN;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.row = (self.row + 1) % ROWS_PER_PATTERN;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::PageUp) {
                self.row = self.row.saturating_sub(16);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::PageDown) {
                self.row = (self.row + 16).min(ROWS_PER_PATTERN - 1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                self.row = 0;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::End) {
                self.row = ROWS_PER_PATTERN - 1;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                match self.column {
                    SubColumn::Velocity => self.column = SubColumn::Note,
                    SubColumn::Note => {
                        self.lane = (self.lane + NUM_LANES - 1) % NUM_LANES;
                        self.column = SubColumn::Velocity;
                    }
                }
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                match self.column {
                    SubColumn::Note => self.column = SubColumn::Velocity,
                    SubColumn::Velocity => {
                        self.lane = (self.lane + 1) % NUM_LANES;
                        self.column = SubColumn::Note;
                    }
                }
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                self.lane = (self.lane + NUM_LANES - 1) % NUM_LANES;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                self.lane = (self.lane + 1) % NUM_LANES;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }

            // Delete / Backspace / Enter / Air trigger
            if i.consume_key(Modifiers::SHIFT, Key::Delete) {
                let r = self.row;
                let l = self.lane;
                let cleared = NoteCell {
                    lock: SoundLock::default(),
                    ..NoteCell::default()
                };
                pattern.lanes[l][r] = cleared;
                updates.push(CellUpdate {
                    lane: l,
                    row: r,
                    cell: cleared,
                });
                self.advance_row();
            } else if i.consume_key(Modifiers::NONE, Key::Delete) {
                let r = self.row;
                let l = self.lane;
                let cell = &mut pattern.lanes[l][r];
                match self.column {
                    SubColumn::Note => {
                        cell.active = false;
                        cell.air = false;
                    }
                    SubColumn::Velocity => {
                        cell.velocity = 100;
                    }
                }
                updates.push(CellUpdate {
                    lane: l,
                    row: r,
                    cell: *cell,
                });
                self.advance_row();
            } else if i.consume_key(Modifiers::NONE, Key::Backspace) {
                self.row = (self.row + ROWS_PER_PATTERN - 1) % ROWS_PER_PATTERN;
                let r = self.row;
                let l = self.lane;
                let cell = &mut pattern.lanes[l][r];
                cell.active = false;
                cell.air = false;
                updates.push(CellUpdate {
                    lane: l,
                    row: r,
                    cell: *cell,
                });
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                let r = self.row;
                let l = self.lane;
                let cell = &mut pattern.lanes[l][r];
                cell.active = !cell.active;
                updates.push(CellUpdate {
                    lane: l,
                    row: r,
                    cell: *cell,
                });
            } else if i.consume_key(Modifiers::NONE, Key::Backtick) {
                let r = self.row;
                let l = self.lane;
                let cell = &mut pattern.lanes[l][r];
                cell.active = true;
                cell.air = true;
                updates.push(CellUpdate {
                    lane: l,
                    row: r,
                    cell: *cell,
                });
                self.advance_row();
            }

            // Subcolumn-specific note or hex velocity entry
            match self.column {
                SubColumn::Note => {
                    for &(k, _) in PIANO_BINDINGS.iter().chain(UPPER_EXT_BINDINGS.iter()) {
                        if i.consume_key(Modifiers::NONE, k) {
                            if let Some(midi) = midi_for_piano_key(k, self.base_octave) {
                                let r = self.row;
                                let l = self.lane;
                                let cell = &mut pattern.lanes[l][r];
                                cell.active = true;
                                cell.air = false;
                                cell.midi = midi;
                                if cell.velocity == 0 {
                                    cell.velocity = 100;
                                }
                                updates.push(CellUpdate {
                                    lane: l,
                                    row: r,
                                    cell: *cell,
                                });
                                self.advance_row();
                            }
                            break;
                        }
                    }
                }
                SubColumn::Velocity => {
                    for &(k, nib) in &HEX_BINDINGS {
                        if i.consume_key(Modifiers::NONE, k) {
                            let r = self.row;
                            let l = self.lane;
                            let cell = &mut pattern.lanes[l][r];
                            let (vel, step_done) =
                                apply_hex_nibble(cell.velocity, nib, &mut self.vel_high_nibble);
                            cell.velocity = vel;
                            cell.active = true;
                            updates.push(CellUpdate {
                                lane: l,
                                row: r,
                                cell: *cell,
                            });
                            if step_done {
                                self.advance_row();
                            }
                            break;
                        }
                    }
                }
            }
        });

        updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piano_keys_map_two_octaves_accurately() {
        assert_eq!(midi_for_piano_key(Key::Z, 4), Some(60));
        assert_eq!(midi_for_piano_key(Key::S, 4), Some(61));
        assert_eq!(midi_for_piano_key(Key::M, 4), Some(71));
        assert_eq!(midi_for_piano_key(Key::Q, 4), Some(72));
        assert_eq!(midi_for_piano_key(Key::Num5, 4), Some(78));
        assert_eq!(midi_for_piano_key(Key::I, 4), Some(84));
    }

    #[test]
    fn hex_velocity_nibbles_merge_and_clamp_to_midi_max() {
        let mut hi = true;
        let (v1, done1) = apply_hex_nibble(0x64, 0x05, &mut hi);
        assert_eq!(v1, 0x54);
        assert!(!done1);

        let (v2, done2) = apply_hex_nibble(v1, 0x0C, &mut hi);
        assert_eq!(v2, 0x5C);
        assert!(done2);

        let (v_max, _) = apply_hex_nibble(0x7F, 0x0F, &mut hi);
        assert_eq!(v_max, 127);
        assert_eq!(hex_key_to_nibble(Key::F), Some(15));
    }
}
