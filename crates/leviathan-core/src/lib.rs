#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "serde")]
mod pattern_serde;
mod song;

pub use song::{LaunchTarget, PatternChain, SongPlayer, SongStatus, MAX_CHAIN_ENTRIES};

pub const MAX_PATTERNS: usize = 128;
pub const NUM_LANES: usize = 6;
pub const ROWS_PER_PATTERN: usize = 64;
pub const SOUND_PARAMS: usize = 8;

pub const SOUND_LINK_MAGIC: u64 = 0x474f_5247_534e_4431; // "GORGSND1"
pub const SOUND_LINK_VERSION: u32 = 2;
pub const PROB_SALT: u32 = 0x7072_6f62; // "prob"

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SoundLock {
    pub machine: i16,
    pub mask: u16,
    pub values: [f32; SOUND_PARAMS],
}

impl Default for SoundLock {
    fn default() -> Self {
        Self {
            machine: -1,
            mask: 0,
            values: [0.0; SOUND_PARAMS],
        }
    }
}
impl SoundLock {
    #[inline]
    pub const fn has_param(&self, slot: usize) -> bool {
        slot < SOUND_PARAMS && ((self.mask >> slot) & 1) != 0
    }

    #[inline]
    pub const fn resolved_with_base(
        &self,
        default_machine: i16,
        base_values: &[f32; SOUND_PARAMS],
    ) -> Self {
        let mut values = *base_values;
        let mut slot = 0;
        while slot < SOUND_PARAMS {
            if ((self.mask >> slot) & 1) != 0 {
                values[slot] = self.values[slot];
            }
            slot += 1;
        }
        Self {
            machine: if self.machine >= 0 {
                self.machine
            } else {
                default_machine
            },
            mask: 0x00ff,
            values,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoundLane {
    pub sound: SoundLock,
    pub gate: f32,
    pub velocity: f32,
    pub pitch: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundLinkPacket {
    pub magic: u64,
    pub version: u32,
    pub bytes: u32,
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub playing: u8,
    pub pad: [u8; 7],
    pub beat: f64,
    pub lanes: [SoundLane; NUM_LANES],
}

const _: () = assert!(core::mem::size_of::<SoundLock>() == 36);
const _: () = assert!(core::mem::size_of::<SoundLane>() == 48);
const _: () = assert!(core::mem::size_of::<SoundLinkPacket>() == 336);

impl Default for SoundLinkPacket {
    fn default() -> Self {
        Self {
            magic: SOUND_LINK_MAGIC,
            version: SOUND_LINK_VERSION,
            bytes: core::mem::size_of::<Self>() as u32,
            sequence: 0,
            timestamp_ns: 0,
            playing: 0,
            pad: [0; 7],
            beat: 0.0,
            lanes: [SoundLane::default(); NUM_LANES],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PlaybackRate {
    Quarter,
    Half,
    OneX,
    TwoX,
    FourX,
    EightX,
}

impl PlaybackRate {
    #[inline]
    pub const fn rows_per_beat(self) -> f64 {
        match self {
            Self::Quarter => 1.0,
            Self::Half => 2.0,
            Self::OneX => 4.0,
            Self::TwoX => 8.0,
            Self::FourX => 16.0,
            Self::EightX => 32.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FillCondition {
    Always,
    WhenFill,
    WhenNoFill,
}

pub const MAX_NUDGE: i8 = 24;
pub const MAX_RETRIG: u8 = 4;
pub const MAX_SWING: f32 = 0.75;

const fn straight_swing() -> f32 {
    0.5
}

const fn single_trigger() -> u8 {
    1
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NoteCell {
    pub active: bool,
    pub air: bool,
    pub midi: u8,
    pub velocity: u8,
    pub gate_fraction: f32,
    pub probability: u8, // 0..=100
    pub fill: FillCondition,
    pub lock: SoundLock,
    #[cfg_attr(feature = "serde", serde(default))]
    pub micro_offset: i8,
    #[cfg_attr(feature = "serde", serde(default = "single_trigger"))]
    pub retrig: u8,
}

impl Default for NoteCell {
    fn default() -> Self {
        Self {
            active: false,
            air: false,
            midi: 60,
            velocity: 100,
            gate_fraction: 0.5,
            probability: 100,
            fill: FillCondition::Always,
            lock: SoundLock::default(),
            micro_offset: 0,
            retrig: single_trigger(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AutomationRow {
    pub z: f32,        // 0.0..=1.0
    pub roll: f32,     // -1.0..=1.0
    pub feedback: f32, // 0.0..=1.0
    pub byte_word: u8,
}

impl Default for AutomationRow {
    fn default() -> Self {
        Self {
            z: 0.25,
            roll: 0.0,
            feedback: 0.35,
            byte_word: 0,
        }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pattern {
    pub length: usize,
    pub rate: PlaybackRate,
    #[cfg_attr(feature = "serde", serde(default = "straight_swing"))]
    pub swing: f32,
    #[cfg_attr(feature = "serde", serde(with = "pattern_serde::lanes"))]
    pub lanes: [[NoteCell; ROWS_PER_PATTERN]; NUM_LANES],
    #[cfg_attr(feature = "serde", serde(with = "pattern_serde::automation"))]
    pub automation: [AutomationRow; ROWS_PER_PATTERN],
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            length: ROWS_PER_PATTERN,
            rate: PlaybackRate::OneX,
            swing: straight_swing(),
            lanes: [[NoteCell::default(); ROWS_PER_PATTERN]; NUM_LANES],
            automation: [AutomationRow::default(); ROWS_PER_PATTERN],
        }
    }
}

impl Pattern {
    pub fn duration_beats(&self) -> f64 {
        self.timing().duration_beats()
    }

    fn timing(&self) -> PatternTiming {
        let swing = if self.swing.is_finite() {
            self.swing.clamp(straight_swing(), MAX_SWING)
        } else {
            straight_swing()
        };
        PatternTiming {
            rows: self.length.clamp(1, ROWS_PER_PATTERN),
            rate: self.rate,
            odd_delay: f64::from(swing) * 2.0 - 1.0,
        }
    }
}

#[derive(Clone, Copy)]
struct PatternTiming {
    rows: usize,
    rate: PlaybackRate,
    odd_delay: f64,
}

impl PatternTiming {
    fn duration_beats(&self) -> f64 {
        self.rows as f64 / self.rate.rows_per_beat()
    }

    fn row_start(self, row: usize) -> f64 {
        row as f64
            + if row & 1 != 0 && row < self.rows {
                self.odd_delay
            } else {
                0.0
            }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub bpm: f64,
    pub sample_rate: f64,
    pub ppqn: u32,
    pub reset_ms: f64,
    pub pitch_zero: i32, // 60 (C4 = 0V) or 48 (C3 = 0V)
    pub key_offset: i32,
    pub v_high: f32,     // 5.0V or 10.0V
    pub seed: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            bpm: 132.0,
            sample_rate: 48_000.0,
            ppqn: 4,
            reset_ms: 1.0,
            pitch_zero: 60,
            key_offset: 0,
            v_high: 10.0,
            seed: 0x1337_c0de,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CvFrame {
    pub beat: f64,
    pub row: usize,
    pub clock_v: f32,
    pub reset_v: f32,
    pub run_v: f32,
    pub pitch_v: [f32; NUM_LANES],
    pub gate_v: [f32; NUM_LANES],
    /// Lanes with a new nudged or ratcheted attack on this sample.
    pub retrigger_mask: u8,
    pub velocity_v: [f32; NUM_LANES],
    pub locks: [SoundLock; NUM_LANES],
    pub z_v: f32,
    pub roll_v: f32,
    pub feedback_v: f32,
    pub bytebus_v: [f32; 9],
}

#[inline]
pub const fn lowbias32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
pub const fn chance_u32(seed: u32, pattern: u32, lane: u32, row: u32, bar_count: u32, sub: u32) -> u32 {
    let mut h = lowbias32(seed ^ PROB_SALT);
    let vals = [pattern, lane, row, bar_count, sub];
    let mut i = 0;
    while i < vals.len() {
        h = lowbias32(h ^ vals[i]);
        i += 1;
    }
    h
}

#[inline]
pub fn passes_probability(prob_pct: u8, seed: u32, pattern: u32, lane: u32, row: u32, bar_count: u32) -> bool {
    if prob_pct >= 100 {
        return true;
    }
    if prob_pct == 0 {
        return false;
    }
    let roll = (chance_u32(seed, pattern, lane, row, bar_count, 0) as u64 * 100) >> 32;
    (roll as u8) < prob_pct
}

#[inline]
pub const fn encode_bytebus_frame(word: u8, clock_high: bool) -> [f32; 9] {
    let mut out = [0.0_f32; 9];
    let mut bit = 0;
    while bit < 8 {
        out[bit] = if ((word >> bit) & 1) != 0 { 10.0 } else { 0.0 };
        bit += 1;
    }
    out[8] = if clock_high { 10.0 } else { 0.0 };
    out
}

#[inline]
fn fract_positive(x: f64) -> f64 {
    let f = x - (x as i64 as f64);
    if f < 0.0 { f + 1.0 } else { f }
}

pub struct TrackerEvaluator {
    pub config: EngineConfig,
    pub playing: bool,
    pub fill_active: bool,
    pub active_pattern: usize,
    clock: SampleClock,
    cycle_start_beat: f64,
    held_pitch_v: [f32; NUM_LANES],
    held_vel_v: [f32; NUM_LANES],
    held_locks: [SoundLock; NUM_LANES],
    last_pulse: [Option<(u32, usize, u8)>; NUM_LANES],
    sequence: u64,
}

struct SampleClock {
    anchor_beat: f64,
    elapsed_samples: u64,
    bpm: f64,
    sample_rate: f64,
}

impl SampleClock {
    fn new(config: &EngineConfig) -> Self {
        Self {
            anchor_beat: 0.0,
            elapsed_samples: 0,
            bpm: config.bpm,
            sample_rate: config.sample_rate,
        }
    }

    fn beat(&self) -> f64 {
        self.anchor_beat + (self.elapsed_samples as f64 * self.bpm) / (60.0 * self.sample_rate)
    }

    fn take_sample(&mut self, config: &EngineConfig) -> f64 {
        let beat = self.beat();
        if self.bpm != config.bpm || self.sample_rate != config.sample_rate {
            self.anchor_beat = beat;
            self.elapsed_samples = 0;
            self.bpm = config.bpm;
            self.sample_rate = config.sample_rate;
        }
        self.elapsed_samples = self.elapsed_samples.wrapping_add(1);
        beat
    }
}

impl Default for TrackerEvaluator {
    fn default() -> Self {
        Self::new(EngineConfig::default())
    }
}

impl TrackerEvaluator {
    pub fn new(config: EngineConfig) -> Self {
        let clock = SampleClock::new(&config);
        Self {
            config,
            playing: false,
            fill_active: false,
            active_pattern: 0,
            clock,
            cycle_start_beat: 0.0,
            held_pitch_v: [0.0; NUM_LANES],
            held_vel_v: [0.0; NUM_LANES],
            held_locks: [SoundLock::default(); NUM_LANES],
            last_pulse: [None; NUM_LANES],
            sequence: 0,
        }
    }

    pub fn reset_transport(&mut self) {
        self.clock = SampleClock::new(&self.config);
        self.cycle_start_beat = 0.0;
        self.last_pulse = [None; NUM_LANES];
    }

    pub fn step_sample(&mut self, pattern: &Pattern) -> CvFrame {
        let beat = self.take_sample();
        self.evaluate_sample(pattern, pattern.timing(), beat, 0.0, 0)
    }

    fn take_sample(&mut self) -> f64 {
        if self.playing {
            self.clock.take_sample(&self.config)
        } else {
            self.clock.beat()
        }
    }

    fn evaluate_sample(
        &mut self,
        pattern: &Pattern,
        timing: PatternTiming,
        beat: f64,
        pattern_start_beat: f64,
        previous_plays: u32,
    ) -> CvFrame {
        let v_high = self.config.v_high;
        if !self.playing {
            self.last_pulse = [None; NUM_LANES];
            return CvFrame {
                pitch_v: self.held_pitch_v,
                velocity_v: self.held_vel_v,
                locks: self.held_locks,
                bytebus_v: encode_bytebus_frame(0, false),
                ..CvFrame::default()
            };
        }

        let clock_high = fract_positive(beat * self.config.ppqn as f64) < 0.5;
        let clock_v = if clock_high { v_high } else { 0.0 };

        let elapsed_ms = (beat - self.cycle_start_beat) * (60_000.0 / self.config.bpm);
        let reset_v = if elapsed_ms >= 0.0 && elapsed_ms < self.config.reset_ms {
            v_high
        } else {
            0.0
        };

        let total_rows = (beat - pattern_start_beat).max(0.0) * timing.rate.rows_per_beat();
        let cycle = total_rows as u64 / timing.rows as u64;
        let position = total_rows - (cycle * timing.rows as u64) as f64;
        let grid_row = (position as usize).min(timing.rows - 1);
        let row = if position < timing.row_start(grid_row) {
            grid_row.saturating_sub(1)
        } else {
            grid_row
        };
        let bar_count = previous_plays.wrapping_add(cycle as u32);
        let sample_rows =
            self.config.bpm * timing.rate.rows_per_beat() / (60.0 * self.config.sample_rate);
        let sample_rows = if sample_rows.is_finite() {
            sample_rows.max(0.0)
        } else {
            0.0
        };
        // Leave a full sample on either side of each ratchet edge, with
        // room for rounding when the gate width lands on a sample boundary.
        let edge_width = sample_rows * 1.000_001;

        let mut gate_v = [0.0_f32; NUM_LANES];
        let mut hit_mask: u8 = 0;
        let mut retrigger_mask: u8 = 0;

        for (lane, gate) in gate_v.iter_mut().enumerate() {
            let mut latest: Option<(usize, f64, f64)> = None;
            // A nudge can bring the next row forward or extend the previous
            // row. Keep one more row for notes whose onsets have traded order.
            for candidate in grid_row.saturating_sub(2)..=(grid_row + 1).min(timing.rows - 1) {
                let cell = &pattern.lanes[lane][candidate];
                if !cell.active {
                    continue;
                }
                let nominal = timing.row_start(candidate);
                let span = timing.row_start(candidate + 1) - nominal;
                let nudge = f64::from(cell.micro_offset.clamp(-MAX_NUDGE, MAX_NUDGE)) / 48.0;
                // Keep even the latest hit inside the pattern and leave
                // enough time for each pulse and its gap at the loop edge.
                let minimum_span =
                    (edge_width * 2.0 * f64::from(cell.retrig.clamp(1, MAX_RETRIG))).min(span);
                let start = (nominal + nudge).clamp(0.0, timing.rows as f64 - minimum_span);
                let fill_ok = match cell.fill {
                    FillCondition::Always => true,
                    FillCondition::WhenFill => self.fill_active,
                    FillCondition::WhenNoFill => !self.fill_active,
                };
                if start <= position
                    && latest.is_none_or(|(_, previous, _)| start >= previous)
                    && fill_ok
                    && passes_probability(
                        cell.probability,
                        self.config.seed,
                        self.active_pattern as u32,
                        lane as u32,
                        candidate as u32,
                        bar_count,
                    )
                {
                    latest = Some((candidate, start, span.min(timing.rows as f64 - start)));
                }
            }
            let previous_pulse = self.last_pulse[lane].take();
            let Some((note_row, start, span)) = latest else {
                continue;
            };
            let cell = &pattern.lanes[lane][note_row];
            let elapsed = position - start;
            if elapsed >= span {
                continue;
            }
            if !cell.air {
                let semis = cell.midi as i32 + self.config.key_offset - self.config.pitch_zero;
                self.held_pitch_v[lane] = semis as f32 / 12.0;
            }
            self.held_vel_v[lane] = (cell.velocity.min(127) as f32 / 127.0) * v_high;
            self.held_locks[lane] = cell.lock;

            let count = cell.retrig.clamp(1, MAX_RETRIG);
            let interval = span / f64::from(count);
            let pulse = ((elapsed / interval) as u8).min(count - 1);
            let phase = elapsed - f64::from(pulse) * interval;
            let width = interval * f64::from(cell.gate_fraction.clamp(0.01, 1.0));
            let width = if count > 1 {
                let gap = edge_width.min(interval * 0.5);
                width.clamp(gap, interval - gap)
            } else if cell.micro_offset != 0 {
                width.max(edge_width.min(interval))
            } else {
                width
            };
            if phase < width {
                *gate = v_high;
                hit_mask |= 1 << lane;
                if count > 1 || cell.micro_offset != 0 {
                    let pulse = Some((bar_count, note_row, pulse));
                    if pulse != previous_pulse {
                        retrigger_mask |= 1 << lane;
                    }
                    self.last_pulse[lane] = pulse;
                }
            }
        }

        let auto = &pattern.automation[row];
        let bus_word = if auto.byte_word != 0 { auto.byte_word } else { hit_mask };

        CvFrame {
            beat,
            row,
            clock_v,
            reset_v,
            run_v: v_high,
            pitch_v: self.held_pitch_v,
            gate_v,
            retrigger_mask,
            velocity_v: self.held_vel_v,
            locks: self.held_locks,
            z_v: auto.z.clamp(0.0, 1.0) * v_high,
            roll_v: auto.roll.clamp(-1.0, 1.0) * 5.0,
            feedback_v: auto.feedback.clamp(0.0, 1.0) * v_high,
            bytebus_v: encode_bytebus_frame(bus_word, clock_high),
        }
    }

    pub fn build_sound_link(&mut self, frame: &CvFrame, timestamp_ns: u64) -> SoundLinkPacket {
        self.sequence = self.sequence.wrapping_add(1);
        let mut lanes = [SoundLane::default(); NUM_LANES];
        for i in 0..NUM_LANES {
            lanes[i] = SoundLane {
                sound: frame.locks[i],
                gate: frame.gate_v[i],
                velocity: frame.velocity_v[i],
                pitch: frame.pitch_v[i],
            };
        }
        SoundLinkPacket {
            magic: SOUND_LINK_MAGIC,
            version: SOUND_LINK_VERSION,
            bytes: core::mem::size_of::<SoundLinkPacket>() as u32,
            sequence: self.sequence,
            timestamp_ns,
            playing: u8::from(self.playing),
            pad: [0; 7],
            beat: frame.beat,
            lanes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowbias32_is_deterministic_and_bijective_zero() {
        assert_eq!(lowbias32(0), 0);
        let a = chance_u32(0x1234_5678, 0, 2, 16, 4, 0);
        let b = chance_u32(0x1234_5678, 0, 2, 16, 4, 0);
        let c = chance_u32(0x1234_5678, 0, 2, 17, 4, 0);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn bytebus_strictly_uses_10v_rails() {
        let frame = encode_bytebus_frame(0b1010_0001, true);
        assert_eq!(frame[0], 10.0);
        assert_eq!(frame[1], 0.0);
        assert_eq!(frame[5], 10.0);
        assert_eq!(frame[7], 10.0);
        assert_eq!(frame[8], 10.0);
    }

    #[test]
    fn pitch_holds_across_rests_and_air_steps() {
        let mut pat = Pattern::default();
        pat.lanes[0][0] = NoteCell {
            active: true,
            midi: 72, // C5 -> +1.0V when pitch_zero = 60
            velocity: 127,
            ..NoteCell::default()
        };
        pat.lanes[0][1] = NoteCell {
            active: true,
            air: true,
            midi: 36, // ignored because air = true
            velocity: 64,
            ..NoteCell::default()
        };

        let mut eval = TrackerEvaluator::new(EngineConfig {
            bpm: 120.0,
            sample_rate: 8.0, // 4 samples per beat -> 1 sample per 16th row at 1x
            ..EngineConfig::default()
        });
        eval.playing = true;

        let f0 = eval.step_sample(&pat);
        assert!((f0.pitch_v[0] - 1.0).abs() < 1e-6);
        assert!((f0.velocity_v[0] - 10.0).abs() < 1e-5);

        let f1 = eval.step_sample(&pat);
        assert!((f1.pitch_v[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sound_lock_resolves_masked_overrides_over_lane_base() {
        let base = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let mut lock = SoundLock::default();
        lock.mask = (1 << 1) | (1 << 6);
        lock.values[1] = 0.95;
        lock.values[6] = 0.05;

        let resolved = lock.resolved_with_base(4, &base);
        assert_eq!(resolved.machine, 4);
        assert_eq!(resolved.mask, 0x00ff);
        assert_eq!(resolved.values, [0.1, 0.95, 0.3, 0.4, 0.5, 0.6, 0.05, 0.8]);

        lock.machine = 6;
        assert_eq!(lock.resolved_with_base(4, &base).machine, 6);
        assert!(lock.has_param(1));
        assert!(!lock.has_param(0));
    }
}
