use std::collections::VecDeque;
use std::sync::Arc;

use leviathan_core::{NoteCell, Pattern, SoundLock, NUM_LANES, ROWS_PER_PATTERN};

pub const MAX_UNDO_STEPS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRect {
    pub min_lane: usize,
    pub max_lane: usize,
    pub min_row: usize,
    pub max_row: usize,
}

impl BlockRect {
    pub fn single(lane: usize, row: usize) -> Self {
        let l = lane.min(NUM_LANES - 1);
        let r = row.min(ROWS_PER_PATTERN - 1);
        Self {
            min_lane: l,
            max_lane: l,
            min_row: r,
            max_row: r,
        }
    }

    pub fn from_corners(a: (usize, usize), b: (usize, usize)) -> Self {
        let min_lane = a.0.min(b.0).min(NUM_LANES - 1);
        let max_lane = a.0.max(b.0).min(NUM_LANES - 1);
        let min_row = a.1.min(b.1).min(ROWS_PER_PATTERN - 1);
        let max_row = a.1.max(b.1).min(ROWS_PER_PATTERN - 1);
        Self {
            min_lane,
            max_lane,
            min_row,
            max_row,
        }
    }

    #[inline]
    pub fn contains(self, lane: usize, row: usize) -> bool {
        lane >= self.min_lane
            && lane <= self.max_lane
            && row >= self.min_row
            && row <= self.max_row
    }

    #[inline]
    pub fn is_multi_cell(self) -> bool {
        self.min_lane != self.max_lane || self.min_row != self.max_row
    }

    #[inline]
    pub fn dimensions(self) -> (usize, usize) {
        (
            self.max_lane - self.min_lane + 1,
            self.max_row - self.min_row + 1,
        )
    }
}

#[derive(Clone, Debug)]
pub struct BlockClipboard {
    pub width_lanes: usize,
    pub height_rows: usize,
    pub cells: Vec<Vec<NoteCell>>,
}

impl BlockClipboard {
    pub fn capture(pattern: &Pattern, rect: BlockRect) -> Self {
        let (width_lanes, height_rows) = rect.dimensions();
        let mut cells = Vec::with_capacity(width_lanes);
        for l in rect.min_lane..=rect.max_lane {
            let mut col = Vec::with_capacity(height_rows);
            for r in rect.min_row..=rect.max_row {
                col.push(pattern.lanes[l][r]);
            }
            cells.push(col);
        }
        Self {
            width_lanes,
            height_rows,
            cells,
        }
    }

    pub fn paste(&self, pattern: &mut Pattern, dst_lane: usize, dst_row: usize) {
        for (dl, col) in self.cells.iter().enumerate() {
            let target_lane = dst_lane + dl;
            if target_lane >= NUM_LANES {
                break;
            }
            for (dr, &cell) in col.iter().enumerate() {
                let target_row = dst_row + dr;
                if target_row >= ROWS_PER_PATTERN {
                    break;
                }
                pattern.lanes[target_lane][target_row] = cell;
            }
        }
    }
}

pub fn clear_rect(pattern: &mut Pattern, rect: BlockRect, wipe_locks: bool) {
    for l in rect.min_lane..=rect.max_lane {
        for r in rect.min_row..=rect.max_row {
            if wipe_locks {
                pattern.lanes[l][r] = NoteCell {
                    lock: SoundLock::default(),
                    ..NoteCell::default()
                };
            } else {
                pattern.lanes[l][r].active = false;
                pattern.lanes[l][r].air = false;
            }
        }
    }
}

pub fn transpose_rect(pattern: &mut Pattern, rect: BlockRect, semitones: i32) -> usize {
    let mut count = 0usize;
    for l in rect.min_lane..=rect.max_lane {
        for r in rect.min_row..=rect.max_row {
            let cell = &mut pattern.lanes[l][r];
            if cell.active && !cell.air {
                cell.midi = (cell.midi as i32 + semitones).clamp(12, 108) as u8;
                count += 1;
            }
        }
    }
    count
}

#[derive(Default)]
pub struct EditHistory {
    undo_stack: VecDeque<Box<Pattern>>,
    redo_stack: VecDeque<Box<Pattern>>,
    retained_rt_arcs: Vec<Arc<Pattern>>,
}

impl EditHistory {
    pub fn record_before_edit(&mut self, current: &Pattern) {
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(Box::new(current.clone()));
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, current: &mut Pattern) -> bool {
        let Some(prev) = self.undo_stack.pop_back() else {
            return false;
        };
        if self.redo_stack.len() >= MAX_UNDO_STEPS {
            self.redo_stack.pop_front();
        }
        self.redo_stack.push_back(Box::new(current.clone()));
        *current = *prev;
        true
    }

    pub fn redo(&mut self, current: &mut Pattern) -> bool {
        let Some(next) = self.redo_stack.pop_back() else {
            return false;
        };
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(Box::new(current.clone()));
        *current = *next;
        true
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Allocates an `Arc<Pattern>` retained on the UI thread so the RT audio thread never frees memory.
    pub fn make_rt_snapshot(&mut self, pattern: &Pattern) -> Arc<Pattern> {
        let snap = Arc::new(pattern.clone());
        self.retained_rt_arcs.push(Arc::clone(&snap));
        if self.retained_rt_arcs.len() > 32 {
            self.retained_rt_arcs.retain(|a| Arc::strong_count(a) > 1);
        }
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_restore_exact_pattern_states() {
        let mut pat = Pattern::default();
        let mut hist = EditHistory::default();

        hist.record_before_edit(&pat);
        pat.lanes[0][0].active = true;
        pat.lanes[0][0].midi = 48;

        hist.record_before_edit(&pat);
        pat.lanes[0][0].midi = 60;

        assert_eq!(hist.undo_len(), 2);
        assert!(hist.undo(&mut pat));
        assert_eq!(pat.lanes[0][0].midi, 48);
        assert!(hist.undo(&mut pat));
        assert!(!pat.lanes[0][0].active);

        assert!(hist.redo(&mut pat));
        assert!(pat.lanes[0][0].active);
        assert_eq!(pat.lanes[0][0].midi, 48);
    }

    #[test]
    fn block_copy_cut_and_paste_preserve_notes_and_sound_locks() {
        let mut pat = Pattern::default();
        pat.lanes[1][2] = NoteCell {
            active: true,
            midi: 64,
            velocity: 112,
            lock: SoundLock {
                machine: 1,
                mask: 0b0101,
                values: [0.8, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0],
            },
            ..NoteCell::default()
        };

        let src = BlockRect::from_corners((2, 4), (1, 2));
        assert_eq!(src.min_lane, 1);
        assert_eq!(src.max_lane, 2);
        let clip = BlockClipboard::capture(&pat, src);
        clear_rect(&mut pat, src, true);
        assert!(!pat.lanes[1][2].active);

        clip.paste(&mut pat, 4, 62);
        assert!(pat.lanes[4][62].active);
        assert_eq!(pat.lanes[4][62].midi, 64);
        assert_eq!(pat.lanes[4][62].lock.mask, 0b0101);
    }

    #[test]
    fn semitone_and_octave_transpose_skip_inactive_and_air_cells() {
        let mut pat = Pattern::default();
        pat.lanes[0][0] = NoteCell {
            active: true,
            midi: 60,
            ..NoteCell::default()
        };
        pat.lanes[0][1] = NoteCell {
            active: true,
            air: true,
            midi: 60,
            ..NoteCell::default()
        };
        let rect = BlockRect::from_corners((0, 0), (0, 3));
        let changed = transpose_rect(&mut pat, rect, 12);
        assert_eq!(changed, 1);
        assert_eq!(pat.lanes[0][0].midi, 72);
        assert_eq!(pat.lanes[0][1].midi, 60);
    }
}
