use eframe::egui::{self, Key, Modifiers};
use leviathan_core::{NoteCell, Pattern, NUM_LANES, ROWS_PER_PATTERN};

use crate::lane_mix::{LaneMix, LaneMixAction};
use crate::tracker_edit::{
    clear_rect, transpose_rect, BlockClipboard, BlockRect, EditHistory, EuclideanFill, RampTarget,
    ScaleLock,
};

const CMD_ONLY: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: false,
    mac_cmd: false,
    command: true,
};

const CMD_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

const CTRL_ONLY: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: false,
};

const CTRL_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: true,
    mac_cmd: false,
    command: false,
};

pub fn apply_lane_mix_shortcuts(ctx: &egui::Context, mix: &mut LaneMix) {
    const DIGITS: [Key; NUM_LANES] = [
        Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6,
    ];
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            let egui::Event::Key {
                key, physical_key, pressed: true, repeat, modifiers,
            } = event
            else {
                return true;
            };
            if !modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
                return true;
            }
            let action = if !modifiers.shift
                && (*key == Key::Num0 || *physical_key == Some(Key::Num0))
            {
                LaneMixAction::Clear
            } else if let Some(lane) = DIGITS
                .iter()
                .position(|&digit| digit == *key)
                .or_else(|| DIGITS.iter().position(|&digit| Some(digit) == *physical_key))
            {
                if modifiers.shift {
                    LaneMixAction::Solo(lane)
                } else {
                    LaneMixAction::Mute(lane)
                }
            } else {
                return true;
            };
            if !*repeat {
                mix.apply(action);
            }
            false
        });
    });
}

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
pub enum PatternSync {
    None,
    Single(CellUpdate),
    FullPattern,
    Undo,
    Redo,
}

#[derive(Clone, Copy)]
pub enum GeneratorAction {
    Euclidean(EuclideanFill),
    Ramp,
    Quantize,
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
    pub anchor: Option<(usize, usize)>,
    pub clipboard: Option<BlockClipboard>,
    pub ramp_target: RampTarget,
    pub scale_lock: ScaleLock,
    pub last_edit_msg: String,
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
            anchor: None,
            clipboard: None,
            ramp_target: RampTarget::default(),
            scale_lock: ScaleLock::default(),
            last_edit_msg: "Ready".to_string(),
        }
    }
}

const PIANO_BINDINGS: [(Key, i32); 29] = [
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

#[cfg(test)]
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

fn consume_cmd(i: &mut egui::InputState, shift: bool, key: Key) -> bool {
    if shift {
        i.consume_key(CMD_SHIFT, key) || i.consume_key(CTRL_SHIFT, key)
    } else {
        i.consume_key(CMD_ONLY, key) || i.consume_key(CTRL_ONLY, key)
    }
}

impl TrackerCursor {
    pub fn select(&mut self, lane: usize, row: usize, col: SubColumn, extend_block: bool) {
        if extend_block {
            if self.anchor.is_none() {
                self.anchor = Some((self.lane, self.row));
            }
        } else {
            self.anchor = None;
        }
        self.lane = lane.min(NUM_LANES - 1);
        self.row = row.min(ROWS_PER_PATTERN - 1);
        self.column = col;
        self.vel_high_nibble = true;
    }

    pub fn active_rect(&self) -> BlockRect {
        match self.anchor {
            Some(a) => BlockRect::from_corners(a, (self.lane, self.row)),
            None => BlockRect::single(self.lane, self.row),
        }
    }

    pub fn copy_selection(&mut self, pattern: &Pattern) {
        let rect = self.active_rect();
        let (w, h) = rect.dimensions();
        self.clipboard = Some(BlockClipboard::capture(pattern, rect));
        self.last_edit_msg = format!("Copied {w}x{h} block");
    }

    pub fn cut_selection(&mut self, pattern: &mut Pattern, history: &mut EditHistory) {
        let rect = self.active_rect();
        let (w, h) = rect.dimensions();
        history.record_before_edit(pattern);
        self.clipboard = Some(BlockClipboard::capture(pattern, rect));
        clear_rect(pattern, rect, true);
        self.last_edit_msg = format!("Cut {w}x{h} block");
    }

    pub fn paste_selection(&mut self, pattern: &mut Pattern, history: &mut EditHistory) -> bool {
        let Some(clip) = self.clipboard.as_ref() else {
            self.last_edit_msg = "Clipboard empty".to_string();
            return false;
        };
        history.record_before_edit(pattern);
        clip.paste(pattern, self.lane, self.row);
        self.last_edit_msg = format!("Pasted {}x{} block", clip.width_lanes, clip.height_rows);
        true
    }

    pub fn transpose_selection(
        &mut self,
        pattern: &mut Pattern,
        history: &mut EditHistory,
        semitones: i32,
    ) -> bool {
        let rect = self.active_rect();
        history.record_before_edit(pattern);
        let n = transpose_rect(pattern, rect, semitones);
        self.last_edit_msg = format!("Transposed {n} note(s) by {semitones:+} st");
        n > 0
    }

    pub fn generate(
        &mut self,
        pattern: &mut Pattern,
        history: &mut EditHistory,
        action: GeneratorAction,
    ) -> bool {
        let rect = self.active_rect();
        let changed = history.edit(pattern, |edited| match action {
            GeneratorAction::Euclidean(fill) => fill.apply(edited, rect),
            GeneratorAction::Ramp => self.ramp_target.apply(edited, rect),
            GeneratorAction::Quantize => self.scale_lock.apply(edited, rect),
        });
        self.last_edit_msg = if changed {
            match action {
                GeneratorAction::Euclidean(_) => "Filled Euclidean rhythm".to_string(),
                GeneratorAction::Ramp => format!("Interpolated {}", self.ramp_target.label()),
                GeneratorAction::Quantize => "Quantized pitches".to_string(),
            }
        } else if matches!(action, GeneratorAction::Ramp) && rect.max_row - rect.min_row < 2 {
            "Select at least three rows to interpolate".to_string()
        } else {
            "Selection already matches".to_string()
        };
        changed
    }

    fn ensure_anchor(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some((self.lane, self.row));
        }
    }

    fn advance_row(&mut self) {
        self.anchor = None;
        self.row = (self.row + self.edit_step) % ROWS_PER_PATTERN;
        self.vel_high_nibble = true;
        self.moved_this_frame = true;
    }

    pub fn enter_note(
        &mut self,
        pattern: &mut Pattern,
        history: &mut EditHistory,
        midi: u8,
        velocity: Option<u8>,
    ) -> CellUpdate {
        history.record_before_edit(pattern);
        let (lane, row) = (self.lane, self.row);
        let cell = &mut pattern.lanes[lane][row];
        cell.active = true;
        cell.air = false;
        cell.midi = self.scale_lock.quantize(midi);
        cell.velocity = velocity.unwrap_or(if cell.velocity == 0 { 100 } else { cell.velocity });
        let update = CellUpdate {
            lane,
            row,
            cell: *cell,
        };
        self.advance_row();
        update
    }

    pub fn handle_input(
        &mut self,
        ctx: &egui::Context,
        pattern: &mut Pattern,
        history: &mut EditHistory,
    ) -> PatternSync {
        self.moved_this_frame = false;
        if ctx.wants_keyboard_input() {
            return PatternSync::None;
        }

        let mut sync = PatternSync::None;

        ctx.input_mut(|i| {
            let mut ev_copy = false;
            let mut ev_cut = false;
            let mut ev_paste = false;
            i.events.retain(|ev| match ev {
                egui::Event::Copy => {
                    ev_copy = true;
                    false
                }
                egui::Event::Cut => {
                    ev_cut = true;
                    false
                }
                egui::Event::Paste(_) => {
                    ev_paste = true;
                    false
                }
                _ => true,
            });

            // 1. Undo / Redo / Select-All / Clipboard / Transpose shortcuts (most-specific first)
            if consume_cmd(i, true, Key::Z) || consume_cmd(i, false, Key::Y) {
                sync = PatternSync::Redo;
                return;
            }
            if consume_cmd(i, false, Key::Z) {
                sync = PatternSync::Undo;
                return;
            }
            if consume_cmd(i, false, Key::I) {
                if self.generate(pattern, history, GeneratorAction::Ramp) {
                    sync = PatternSync::FullPattern;
                }
                return;
            }
            if ev_copy || consume_cmd(i, false, Key::C) {
                self.copy_selection(pattern);
                return;
            }
            if ev_cut || consume_cmd(i, false, Key::X) {
                self.cut_selection(pattern, history);
                sync = PatternSync::FullPattern;
                return;
            }
            if ev_paste || consume_cmd(i, false, Key::V) {
                if self.paste_selection(pattern, history) {
                    sync = PatternSync::FullPattern;
                }
                return;
            }
            if consume_cmd(i, false, Key::A) {
                let r = self.active_rect();
                if r.min_row == 0 && r.max_row == ROWS_PER_PATTERN - 1 && r.min_lane == r.max_lane {
                    self.anchor = Some((0, 0));
                    self.lane = NUM_LANES - 1;
                    self.row = ROWS_PER_PATTERN - 1;
                    self.last_edit_msg = "Selected all 6 lanes".to_string();
                } else {
                    self.anchor = Some((self.lane, 0));
                    self.row = ROWS_PER_PATTERN - 1;
                    self.last_edit_msg = format!("Selected lane {}", self.lane);
                }
                self.moved_this_frame = true;
                return;
            }
            if consume_cmd(i, true, Key::ArrowUp) {
                self.transpose_selection(pattern, history, 12);
                sync = PatternSync::FullPattern;
                return;
            }
            if consume_cmd(i, true, Key::ArrowDown) {
                self.transpose_selection(pattern, history, -12);
                sync = PatternSync::FullPattern;
                return;
            }
            if consume_cmd(i, false, Key::ArrowUp) {
                self.transpose_selection(pattern, history, 1);
                sync = PatternSync::FullPattern;
                return;
            }
            if consume_cmd(i, false, Key::ArrowDown) {
                self.transpose_selection(pattern, history, -1);
                sync = PatternSync::FullPattern;
                return;
            }

            // 2. Shift + Navigation -> Rectangular Block Selection
            if i.consume_key(Modifiers::SHIFT, Key::ArrowUp) {
                self.ensure_anchor();
                self.row = self.row.saturating_sub(1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowDown) {
                self.ensure_anchor();
                self.row = (self.row + 1).min(ROWS_PER_PATTERN - 1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
                self.ensure_anchor();
                self.lane = self.lane.saturating_sub(1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
                self.ensure_anchor();
                self.lane = (self.lane + 1).min(NUM_LANES - 1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::PageUp) {
                self.ensure_anchor();
                self.row = self.row.saturating_sub(16);
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::PageDown) {
                self.ensure_anchor();
                self.row = (self.row + 16).min(ROWS_PER_PATTERN - 1);
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::Home) {
                self.ensure_anchor();
                self.row = 0;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::SHIFT, Key::End) {
                self.ensure_anchor();
                self.row = ROWS_PER_PATTERN - 1;
                self.moved_this_frame = true;
                return;
            }

            // 3. Octave & Edit-Step hotkeys
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                self.anchor = None;
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Minus) {
                self.base_octave = (self.base_octave - 1).clamp(1, 6);
            }
            if i.consume_key(Modifiers::NONE, Key::Equals) {
                self.base_octave = (self.base_octave + 1).clamp(1, 6);
            }
            if i.consume_key(Modifiers::NONE, Key::OpenBracket) {
                self.edit_step = self.edit_step.saturating_sub(1);
            }
            if i.consume_key(Modifiers::NONE, Key::CloseBracket) {
                self.edit_step = (self.edit_step + 1).min(16);
            }

            // 4. Plain Cursor Navigation (clears block anchor)
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.anchor = None;
                self.row = (self.row + ROWS_PER_PATTERN - 1) % ROWS_PER_PATTERN;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.anchor = None;
                self.row = (self.row + 1) % ROWS_PER_PATTERN;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::PageUp) {
                self.anchor = None;
                self.row = self.row.saturating_sub(16);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::PageDown) {
                self.anchor = None;
                self.row = (self.row + 16).min(ROWS_PER_PATTERN - 1);
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                self.anchor = None;
                self.row = 0;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::End) {
                self.anchor = None;
                self.row = ROWS_PER_PATTERN - 1;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                self.anchor = None;
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
                self.anchor = None;
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
                self.anchor = None;
                self.lane = (self.lane + NUM_LANES - 1) % NUM_LANES;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                self.anchor = None;
                self.lane = (self.lane + 1) % NUM_LANES;
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
            }

            // 5. Delete / Backspace / Enter / Backtick
            if i.consume_key(Modifiers::SHIFT, Key::Delete) {
                let rect = self.active_rect();
                history.record_before_edit(pattern);
                clear_rect(pattern, rect, true);
                if rect.is_multi_cell() {
                    self.last_edit_msg = "Cleared block + locks".to_string();
                    sync = PatternSync::FullPattern;
                } else {
                    let cell = pattern.lanes[self.lane][self.row];
                    sync = PatternSync::Single(CellUpdate {
                        lane: self.lane,
                        row: self.row,
                        cell,
                    });
                    self.advance_row();
                }
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Delete) {
                let rect = self.active_rect();
                history.record_before_edit(pattern);
                if rect.is_multi_cell() {
                    clear_rect(pattern, rect, false);
                    self.last_edit_msg = "Cleared selected block".to_string();
                    sync = PatternSync::FullPattern;
                } else {
                    let cell = &mut pattern.lanes[self.lane][self.row];
                    match self.column {
                        SubColumn::Note => {
                            cell.active = false;
                            cell.air = false;
                        }
                        SubColumn::Velocity => {
                            cell.velocity = 100;
                        }
                    }
                    sync = PatternSync::Single(CellUpdate {
                        lane: self.lane,
                        row: self.row,
                        cell: *cell,
                    });
                    self.advance_row();
                }
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Backspace) {
                history.record_before_edit(pattern);
                self.anchor = None;
                self.row = (self.row + ROWS_PER_PATTERN - 1) % ROWS_PER_PATTERN;
                let cell = &mut pattern.lanes[self.lane][self.row];
                cell.active = false;
                cell.air = false;
                sync = PatternSync::Single(CellUpdate {
                    lane: self.lane,
                    row: self.row,
                    cell: *cell,
                });
                self.vel_high_nibble = true;
                self.moved_this_frame = true;
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                history.record_before_edit(pattern);
                let cell = &mut pattern.lanes[self.lane][self.row];
                cell.active = !cell.active;
                sync = PatternSync::Single(CellUpdate {
                    lane: self.lane,
                    row: self.row,
                    cell: *cell,
                });
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Backtick) {
                history.record_before_edit(pattern);
                let l = self.lane;
                let r = self.row;
                let cell = &mut pattern.lanes[l][r];
                cell.active = true;
                cell.air = true;
                sync = PatternSync::Single(CellUpdate {
                    lane: l,
                    row: r,
                    cell: *cell,
                });
                self.advance_row();
                return;
            }

            // 6. Piano Note Entry vs. Hex Velocity Entry (only when Ctrl/Cmd/Alt are NOT held)
            if i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt {
                return;
            }

            match self.column {
                SubColumn::Note => {
                    for &(k, _) in PIANO_BINDINGS.iter().chain(UPPER_EXT_BINDINGS.iter()) {
                        if i.consume_key(Modifiers::NONE, k) {
                            if let Some(midi) = midi_for_piano_key(k, self.base_octave) {
                                sync = PatternSync::Single(
                                    self.enter_note(pattern, history, midi, None),
                                );
                            }
                            break;
                        }
                    }
                }
                SubColumn::Velocity => {
                    for &(k, nib) in &HEX_BINDINGS {
                        if i.consume_key(Modifiers::NONE, k) {
                            if self.vel_high_nibble {
                                history.record_before_edit(pattern);
                            }
                            let l = self.lane;
                            let r = self.row;
                            let cell = &mut pattern.lanes[l][r];
                            let (vel, step_done) =
                                apply_hex_nibble(cell.velocity, nib, &mut self.vel_high_nibble);
                            cell.velocity = vel;
                            cell.active = true;
                            sync = PatternSync::Single(CellUpdate {
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

        sync
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
