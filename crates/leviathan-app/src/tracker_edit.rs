use std::collections::VecDeque;

use leviathan_core::{AutomationRow, NoteCell, Pattern, SoundLock, NUM_LANES, ROWS_PER_PATTERN};

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

    pub fn available_steps(self, pattern_length: usize) -> usize {
        let end = if self.min_row == self.max_row {
            pattern_length
        } else {
            (self.max_row + 1).min(pattern_length)
        };
        end.min(ROWS_PER_PATTERN).saturating_sub(self.min_row)
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

#[derive(Clone, Copy)]
pub struct EuclideanFill {
    pub pulses: usize,
    pub steps: usize,
    pub rotation: i32,
}

impl Default for EuclideanFill {
    fn default() -> Self {
        Self {
            pulses: 5,
            steps: 16,
            rotation: 0,
        }
    }
}

impl EuclideanFill {
    pub fn apply(self, pattern: &mut Pattern, rect: BlockRect) -> bool {
        let steps = self.steps.min(rect.available_steps(pattern.length));
        let rhythm = euclidean_rhythm(self.pulses.min(steps), steps, self.rotation);
        let mut changed = false;
        for lane in &mut pattern.lanes[rect.min_lane..=rect.max_lane] {
            let seed = lane[rect.min_row];
            for (cell, hit) in lane[rect.min_row..].iter_mut().zip(rhythm.iter().copied()) {
                let before = *cell;
                if hit && !cell.active {
                    cell.midi = seed.midi;
                    cell.velocity = seed.velocity;
                    cell.air = seed.air;
                }
                cell.active = hit;
                if !hit {
                    cell.air = false;
                }
                changed |= *cell != before;
            }
        }
        changed
    }
}

fn euclidean_rhythm(pulses: usize, steps: usize, rotation: i32) -> Vec<bool> {
    if pulses == 0 || pulses == steps {
        return vec![pulses != 0; steps];
    }

    // Bjorklund's Euclidean divisions describe how to join groups of hits and rests.
    let mut remainders = vec![pulses];
    let mut counts = Vec::new();
    let mut divisor = steps - pulses;
    let mut remainder = pulses;
    while remainder > 1 {
        counts.push(divisor / remainder);
        (divisor, remainder) = (remainder, divisor % remainder);
        remainders.push(remainder);
    }
    counts.push(divisor);

    fn build(level: isize, counts: &[usize], remainders: &[usize], rhythm: &mut Vec<bool>) {
        match level {
            -2 => rhythm.push(true),
            -1 => rhythm.push(false),
            _ => {
                for _ in 0..counts[level as usize] {
                    build(level - 1, counts, remainders, rhythm);
                }
                if remainders[level as usize] != 0 {
                    build(level - 2, counts, remainders, rhythm);
                }
            }
        }
    }

    let mut rhythm = Vec::with_capacity(steps);
    build(counts.len() as isize - 1, &counts, &remainders, &mut rhythm);
    let first_hit = rhythm.iter().position(|&hit| hit).unwrap();
    rhythm.rotate_left(first_hit);
    rhythm.rotate_right(rotation.rem_euclid(steps as i32) as usize);
    rhythm
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RampTarget {
    #[default]
    Velocity,
    Z,
    Roll,
    Feedback,
}

impl RampTarget {
    pub const ALL: [Self; 4] = [Self::Velocity, Self::Z, Self::Roll, Self::Feedback];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Velocity => "Velocity",
            Self::Z => "Z",
            Self::Roll => "Roll",
            Self::Feedback => "Feedback",
        }
    }

    fn automation(self, row: &mut AutomationRow) -> &mut f32 {
        match self {
            Self::Z => &mut row.z,
            Self::Roll => &mut row.roll,
            Self::Feedback => &mut row.feedback,
            Self::Velocity => unreachable!("velocity is stored per lane"),
        }
    }

    pub fn apply(self, pattern: &mut Pattern, rect: BlockRect) -> bool {
        let span = rect.max_row - rect.min_row;
        if span < 2 {
            return false;
        }
        let mut changed = false;
        let lerp =
            |from: f32, to: f32, offset: usize| from + (to - from) * offset as f32 / span as f32;
        if self == Self::Velocity {
            for lane in &mut pattern.lanes[rect.min_lane..=rect.max_lane] {
                let rows = &mut lane[rect.min_row..=rect.max_row];
                let from = f32::from(rows[0].velocity);
                let to = f32::from(rows[span].velocity);
                for (offset, cell) in rows.iter_mut().enumerate().skip(1).take(span - 1) {
                    let velocity = lerp(from, to, offset).round() as u8;
                    changed |= cell.velocity != velocity;
                    cell.velocity = velocity;
                }
            }
        } else {
            let rows = &mut pattern.automation[rect.min_row..=rect.max_row];
            let from = *self.automation(&mut rows[0]);
            let to = *self.automation(&mut rows[span]);
            for (offset, row) in rows.iter_mut().enumerate().skip(1).take(span - 1) {
                let field = self.automation(row);
                let value = lerp(from, to, offset);
                changed |= *field != value;
                *field = value;
            }
        }
        changed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scale {
    #[default]
    Chromatic,
    Major,
    Minor,
    Dorian,
    Phrygian,
    MajorPentatonic,
    MinorPentatonic,
}

impl Scale {
    pub const ALL: [Self; 7] = [
        Self::Chromatic,
        Self::Major,
        Self::Minor,
        Self::Dorian,
        Self::Phrygian,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Chromatic => "Chromatic",
            Self::Major => "Major",
            Self::Minor => "Natural minor",
            Self::Dorian => "Dorian",
            Self::Phrygian => "Phrygian",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScaleLock {
    pub root: u8,
    pub scale: Scale,
}

impl ScaleLock {
    pub fn quantize(self, midi: u8) -> u8 {
        if self.scale == Scale::Chromatic {
            return midi.min(127);
        }
        let root = self.root % 12;
        // Ascending candidates choose the lower pitch when two notes are equally close.
        (0_u8..=127)
            .filter(|&note| self.scale.intervals().contains(&((note + 12 - root) % 12)))
            .min_by_key(|note| note.abs_diff(midi))
            .unwrap_or(midi.min(127))
    }

    pub fn apply(self, pattern: &mut Pattern, rect: BlockRect) -> bool {
        let mut changed = false;
        for lane in &mut pattern.lanes[rect.min_lane..=rect.max_lane] {
            for cell in &mut lane[rect.min_row..=rect.max_row] {
                if cell.active && !cell.air {
                    let midi = self.quantize(cell.midi);
                    changed |= cell.midi != midi;
                    cell.midi = midi;
                }
            }
        }
        changed
    }
}

struct EditSnapshot {
    pattern: Box<Pattern>,
    bpm: Option<f64>,
}

#[derive(Default)]
pub struct EditHistory {
    undo_stack: VecDeque<EditSnapshot>,
    redo_stack: VecDeque<EditSnapshot>,
}

impl EditHistory {
    pub fn record_before_edit(&mut self, current: &Pattern) {
        self.record_before_import(current, None);
    }

    pub fn record_before_import(&mut self, current: &Pattern, bpm: Option<f64>) {
        Self::push_snapshot(&mut self.undo_stack, current.clone(), bpm);
        self.redo_stack.clear();
    }

    pub fn edit(
        &mut self,
        current: &mut Pattern,
        apply: impl FnOnce(&mut Pattern) -> bool,
    ) -> bool {
        let mut edited = current.clone();
        if !apply(&mut edited) {
            return false;
        }
        Self::push_snapshot(
            &mut self.undo_stack,
            std::mem::replace(current, edited),
            None,
        );
        self.redo_stack.clear();
        true
    }

    fn push_snapshot(stack: &mut VecDeque<EditSnapshot>, pattern: Pattern, bpm: Option<f64>) {
        if stack.len() >= MAX_UNDO_STEPS {
            stack.pop_front();
        }
        stack.push_back(EditSnapshot {
            pattern: Box::new(pattern),
            bpm,
        });
    }

    fn restore(
        from: &mut VecDeque<EditSnapshot>,
        to: &mut VecDeque<EditSnapshot>,
        current: &mut Pattern,
        bpm: &mut f64,
    ) -> bool {
        let Some(snapshot) = from.pop_back() else {
            return false;
        };
        let previous_bpm = snapshot.bpm.map(|value| std::mem::replace(bpm, value));
        Self::push_snapshot(
            to,
            std::mem::replace(current, *snapshot.pattern),
            previous_bpm,
        );
        true
    }

    pub fn undo(&mut self, current: &mut Pattern, bpm: &mut f64) -> bool {
        Self::restore(&mut self.undo_stack, &mut self.redo_stack, current, bpm)
    }

    pub fn redo(&mut self, current: &mut Pattern, bpm: &mut f64) -> bool {
        Self::restore(&mut self.redo_stack, &mut self.undo_stack, current, bpm)
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_restore_exact_pattern_states() {
        let mut pat = Pattern::default();
        let mut hist = EditHistory::default();
        let mut bpm = 120.0;

        hist.record_before_edit(&pat);
        pat.lanes[0][0].active = true;
        pat.lanes[0][0].midi = 48;

        hist.record_before_edit(&pat);
        pat.lanes[0][0].midi = 60;

        assert_eq!(hist.undo_len(), 2);
        assert!(hist.undo(&mut pat, &mut bpm));
        assert_eq!(pat.lanes[0][0].midi, 48);
        assert!(hist.undo(&mut pat, &mut bpm));
        assert!(!pat.lanes[0][0].active);

        assert!(hist.redo(&mut pat, &mut bpm));
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

    #[test]
    fn bjorklund_distributes_hits_with_wrapped_rotation() {
        let hits = |pulses, steps, rotation| {
            euclidean_rhythm(pulses, steps, rotation)
                .into_iter()
                .enumerate()
                .filter_map(|(row, hit)| hit.then_some(row))
                .collect::<Vec<_>>()
        };
        assert_eq!(hits(3, 8, 0), [0, 3, 6]);
        assert_eq!(hits(3, 8, -1), [2, 5, 7]);
        assert_eq!(hits(5, 16, 2), [2, 5, 8, 11, 14]);
        for steps in 1..=ROWS_PER_PATTERN {
            for pulses in 0..=steps {
                let hits = hits(pulses, steps, 17);
                assert_eq!(hits.len(), pulses);
                if let [first, .., last] = hits.as_slice() {
                    let wrap = steps + first - last;
                    let (min, max) = hits.windows(2).fold((wrap, wrap), |(min, max), pair| {
                        let gap = pair[1] - pair[0];
                        (min.min(gap), max.max(gap))
                    });
                    assert!(max - min <= 1, "{pulses}/{steps}");
                }
            }
        }
    }

    #[test]
    fn euclidean_fill_preserves_locks_and_is_one_undoable_edit() {
        let mut pattern = Pattern {
            length: 8,
            ..Pattern::default()
        };
        pattern.lanes[3][1].midi = 42;
        pattern.lanes[3][1].velocity = 111;
        pattern.lanes[3][3].lock.mask = 4;
        pattern.lanes[3][7].active = true;
        let original = pattern.lanes[3];
        let rect = BlockRect::from_corners((3, 1), (3, 6));
        let fill = EuclideanFill {
            pulses: 3,
            steps: 16,
            rotation: 0,
        };
        let mut history = EditHistory::default();
        assert!(history.edit(&mut pattern, |pattern| fill.apply(pattern, rect)));
        let filled = pattern.lanes[3];
        for row in [1, 3, 5] {
            assert!(filled[row].active);
            assert_eq!((filled[row].midi, filled[row].velocity), (42, 111));
        }
        assert_eq!(filled[3].lock.mask, 4);
        assert_eq!(filled[7], original[7]);
        assert!(!history.edit(&mut pattern, |pattern| fill.apply(pattern, rect)));
        assert_eq!(history.undo_len(), 1);
        let mut bpm = 120.0;
        assert!(history.undo(&mut pattern, &mut bpm));
        assert_eq!(pattern.lanes[3], original);
        assert!(history.redo(&mut pattern, &mut bpm));
        assert_eq!(pattern.lanes[3], filled);
    }

    #[test]
    fn ramps_keep_endpoints_and_only_write_the_selected_field() {
        let mut pattern = Pattern::default();
        let rect = BlockRect::from_corners((0, 2), (1, 6));
        for (lane, from, to) in [(0, 10, 110), (1, 100, 20)] {
            pattern.lanes[lane][2].velocity = from;
            pattern.lanes[lane][6].velocity = to;
        }
        assert!(RampTarget::Velocity.apply(&mut pattern, rect));
        assert_eq!(
            pattern.lanes[0][2..=6]
                .iter()
                .map(|cell| cell.velocity)
                .collect::<Vec<_>>(),
            [10, 35, 60, 85, 110]
        );
        assert_eq!(
            pattern.lanes[1][2..=6]
                .iter()
                .map(|cell| cell.velocity)
                .collect::<Vec<_>>(),
            [100, 80, 60, 40, 20]
        );
        assert!(!pattern.lanes[0][4].active);
        assert_eq!(pattern.lanes[2][4].velocity, 100);

        pattern.automation[2] = AutomationRow {
            z: 0.125,
            roll: -0.5,
            feedback: 0.75,
            byte_word: 42,
        };
        pattern.automation[6] = AutomationRow {
            z: 0.625,
            roll: 0.75,
            feedback: 0.25,
            byte_word: 21,
        };
        let endpoints = (pattern.automation[2], pattern.automation[6]);
        let before = pattern.automation[4];
        assert!(RampTarget::Z.apply(&mut pattern, rect));
        assert_eq!(pattern.automation[4], AutomationRow { z: 0.375, ..before });
        assert!(RampTarget::Roll.apply(&mut pattern, rect));
        assert!(RampTarget::Feedback.apply(&mut pattern, rect));
        assert_eq!(
            (pattern.automation[4].roll, pattern.automation[4].feedback),
            (0.125, 0.5)
        );
        assert_eq!(pattern.automation[4].byte_word, before.byte_word);
        assert_eq!((pattern.automation[2], pattern.automation[6]), endpoints);
    }

    #[test]
    fn scale_lock_uses_the_nearest_in_range_pitch_and_preserves_air() {
        let c_major = ScaleLock {
            root: 0,
            scale: Scale::Major,
        };
        let d_major = ScaleLock {
            root: 2,
            scale: Scale::Major,
        };
        assert_eq!(c_major.quantize(61), 60);
        assert_eq!(c_major.quantize(66), 65);
        assert_eq!(d_major.quantize(66), 66);
        assert_eq!(d_major.quantize(0), 1);
        assert_eq!(c_major.quantize(127), 127);
        let mut pattern = Pattern::default();
        pattern.lanes[0][0] = NoteCell {
            active: true,
            midi: 61,
            ..NoteCell::default()
        };
        pattern.lanes[0][1] = NoteCell {
            air: true,
            ..pattern.lanes[0][0]
        };
        pattern.lanes[0][2] = NoteCell {
            active: false,
            ..pattern.lanes[0][0]
        };
        assert!(c_major.apply(&mut pattern, BlockRect::from_corners((0, 0), (0, 2))));
        assert_eq!(pattern.lanes[0][0].midi, 60);
        assert_eq!(pattern.lanes[0][1].midi, 61);
        assert_eq!(pattern.lanes[0][2].midi, 61);
    }
}
