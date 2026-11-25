//! Fixed-point policies and streaming, band-limited rate conversion for BEND.

use crate::voice::sine_cycle;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Rate336,
    Rate224,
    Rate128,
}

impl Mode {
    pub const ALL: [Self; 4] = [Self::Off, Self::Rate336, Self::Rate224, Self::Rate128];

    pub const fn rate(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Rate336 => Some(10_000_000.0 / 336.0),
            Self::Rate224 => Some(10_000_000.0 / 224.0),
            Self::Rate128 => Some(10_000_000.0 / 128.0),
        }
    }
}

#[inline]
fn quantize(x: f32, scale: f32) -> f32 {
    let scaled = x.clamp(-1.0, 1.0 - 1.0 / scale) * scale;
    let truncated = scaled as i32;
    let floored = truncated - i32::from((truncated as f32) > scaled);
    floored as f32 / scale
}

#[inline]
pub(super) fn node(x: f32, fixed: bool) -> f32 {
    if fixed {
        quantize(x, 8_388_608.0)
    } else {
        x
    }
}

#[inline]
pub(super) fn memory(x: f32, fixed: bool) -> f32 {
    if fixed {
        quantize(x, 32_768.0)
    } else {
        x
    }
}

#[inline]
pub(super) fn coefficient(x: f32, fixed: bool) -> f32 {
    // Preserve exactly unity where the algorithm calls for a bypass or Freeze.
    if x == 1.0 {
        x
    } else {
        node(x, fixed)
    }
}

const TAPS: usize = 32;
const PHASES: usize = 64;
const CUTOFFS: [f32; 17] = [
    0.015625, 0.03125, 0.0625, 0.09375, 0.125, 0.15625, 0.1875, 0.21875, 0.25, 0.28125, 0.3125,
    0.34375, 0.375, 0.40625, 0.4375, 0.46875, 0.5,
];
static COEFFICIENTS: [[[f32; TAPS]; PHASES]; CUTOFFS.len()] = coefficients();

const fn sin(turns: f32) -> f32 {
    let turns = turns - (turns as i32) as f32;
    sine_cycle(if turns < 0.0 { turns + 1.0 } else { turns })
}

const fn coefficients() -> [[[f32; TAPS]; PHASES]; CUTOFFS.len()] {
    let mut tables = [[[0.0; TAPS]; PHASES]; CUTOFFS.len()];
    let mut band = 0;
    while band < CUTOFFS.len() {
        let bandwidth = 2.0 * CUTOFFS[band];
        let mut phase = 0;
        while phase < PHASES {
            let fraction = phase as f32 / PHASES as f32;
            let mut sum = 0.0;
            let mut tap = 0;
            while tap < TAPS {
                let x = tap as f32 - (TAPS / 2 - 1) as f32 - fraction;
                let sinc = if x == 0.0 {
                    bandwidth
                } else {
                    sin(x * CUTOFFS[band]) / (core::f32::consts::PI * x)
                };
                let window = 0.5 + 0.5 * sin(0.25 + x / TAPS as f32);
                let y = sinc * window;
                tables[band][phase][tap] = y;
                sum += y;
                tap += 1;
            }
            tap = 0;
            while tap < TAPS {
                tables[band][phase][tap] /= sum;
                tap += 1;
            }
            phase += 1;
        }
        band += 1;
    }
    tables
}

#[derive(Default)]
struct History {
    buffer: [[f32; 2]; TAPS],
    next: usize,
    band: usize,
}

impl History {
    fn new(input: f64, output: f64) -> Self {
        let cutoff = (0.475 * (output / input).min(1.0)) as f32;
        let band = CUTOFFS.iter().rposition(|f| *f <= cutoff).unwrap_or(0);
        Self {
            band,
            ..Self::default()
        }
    }

    fn push(&mut self, frame: [f32; 2]) {
        self.buffer[self.next] = frame;
        self.next = (self.next + 1) % TAPS;
    }

    // Fraction of a source frame after this history window's reference time.
    fn read(&self, fraction: f64) -> [f32; 2] {
        let phase = ((fraction.max(0.0) * PHASES as f64) as usize).min(PHASES - 1);
        let mut output = [0.0; 2];
        for (i, &coef) in COEFFICIENTS[self.band][phase].iter().enumerate() {
            let pair = self.buffer[(self.next + i) % TAPS];
            output[0] += pair[0] * coef;
            output[1] += pair[1] * coef;
        }
        output
    }
}

pub(super) struct Converter {
    input: History,
    output: [History; 2],
    phase: f64,
    ratio: f64,
    dc_in: [[f32; 2]; 2],
    dc_out: [[f32; 2]; 2],
    dc_pole: f32,
}

impl Converter {
    pub(super) fn new(host_rate: f32, chip_rate: f32) -> Self {
        let (host, chip) = (f64::from(host_rate), f64::from(chip_rate));
        let pole = 1.0 / (1.0 + core::f32::consts::TAU * 5.0 / host_rate);
        Self {
            input: History::new(host, chip),
            output: core::array::from_fn(|_| History::new(chip, host)),
            phase: 0.0,
            ratio: chip / host,
            dc_in: [[0.0; 2]; 2],
            dc_out: [[0.0; 2]; 2],
            dc_pole: pole,
        }
    }

    /// Convert one host frame, retaining separate Echo and Space wet signals.
    pub(super) fn step(
        &mut self,
        input: [f32; 2],
        mut process: impl FnMut([f32; 2]) -> [[f32; 2]; 2],
    ) -> [[f32; 2]; 2] {
        self.input.push(input);
        self.phase += self.ratio;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            // Internal ticks arrived within this host interval. The filter
            // history provides one sample of causal margin for these positions.
            let wet = process(self.input.read(1.0 - self.phase / self.ratio));
            for (output, frame) in self.output.iter_mut().zip(wet) {
                output.push(frame);
            }
        }
        let mut wet = self
            .output
            .each_ref()
            .map(|history| history.read(self.phase));
        for (bus, pair) in wet.iter_mut().enumerate() {
            for (ch, value) in pair.iter_mut().enumerate() {
                let filtered = *value - self.dc_in[bus][ch] + self.dc_pole * self.dc_out[bus][ch];
                self.dc_in[bus][ch] = *value;
                self.dc_out[bus][ch] = if filtered.abs() < 1e-20 {
                    0.0
                } else {
                    filtered
                };
                *value = filtered;
            }
        }
        wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturates_q23_and_floors_negative_delay_writes() {
        assert_eq!(node(-2.0, true), -1.0);
        assert_eq!(node(2.0, true), 8_388_607.0 / 8_388_608.0);
        assert_eq!(memory(-0.00001, true), -1.0 / 32768.0);
        assert_eq!(memory(0.00001, true), 0.0);
    }

    #[test]
    fn all_chip_rates_run_at_the_declared_ratio_and_reject_dc() {
        for mode in Mode::ALL.into_iter().skip(1) {
            let rate = mode.rate().unwrap();
            let mut converter = Converter::new(48_000.0, rate);
            let mut count = 0;
            let mut tail = [[0.0; 2]; 2];
            for _ in 0..48_000 {
                tail = converter.step([0.25; 2], |s| {
                    count += 1;
                    [s, [-1.0 / 32768.0; 2]]
                });
            }
            assert!((count as f32 - rate).abs() <= 1.0);
            assert!(tail.iter().flatten().all(|sample| sample.abs() < 1.58e-5));
        }
    }

    #[test]
    fn mode_all_lists_every_variant_in_declared_order() {
        assert_eq!(
            Mode::ALL,
            [Mode::Off, Mode::Rate336, Mode::Rate224, Mode::Rate128]
        );
        assert_eq!(Mode::ALL.len(), 4);
        for mode in Mode::ALL {
            assert!(Mode::ALL.contains(&mode));
        }
    }

    #[test]
    fn off_mode_has_no_rate_and_other_modes_declare_chip_rates() {
        assert_eq!(Mode::Off.rate(), None);
        assert_eq!(Mode::Rate336.rate(), Some(10_000_000.0 / 336.0));
        assert_eq!(Mode::Rate224.rate(), Some(10_000_000.0 / 224.0));
        assert_eq!(Mode::Rate128.rate(), Some(10_000_000.0 / 128.0));
    }

    #[test]
    fn node_and_memory_and_coefficient_pass_through_when_not_fixed() {
        assert_eq!(node(0.37, false), 0.37);
        assert_eq!(memory(-0.37, false), -0.37);
        assert_eq!(coefficient(0.37, false), 0.37);
    }

    #[test]
    fn coefficient_preserves_exact_unity_even_when_fixed() {
        assert_eq!(coefficient(1.0, true), 1.0);
    }

    #[test]
    fn node_quantizes_to_q23_with_clamping_and_floor_semantics() {
        assert_eq!(node(0.0, true), 0.0);
        assert_eq!(node(-1.5, true), -1.0);
        assert_eq!(node(1.5, true), 8_388_607.0 / 8_388_608.0);
        // Positive values truncate toward negative infinity in 1/2^23 steps.
        assert_eq!(node(0.5, true), 4_194_304.0 / 8_388_608.0);
        // Negative values floor as well (see existing saturates test).
        assert_eq!(node(-0.5, true), -4_194_304.0 / 8_388_608.0);
    }

    #[test]
    fn memory_quantizes_to_q15() {
        assert_eq!(memory(0.5, true), 16_384.0 / 32_768.0);
        assert_eq!(memory(-0.5, true), -16_384.0 / 32_768.0);
    }

    #[test]
    fn non_finite_node_input_is_clamped_to_zero_when_fixed() {
        assert_eq!(node(f32::NAN, true), 0.0);
        assert_eq!(node(f32::INFINITY, true), 8_388_607.0 / 8_388_608.0);
        assert_eq!(node(f32::NEG_INFINITY, true), -1.0);
        // Non-fixed path passes non-finite values through unchanged.
        assert!(node(f32::NAN, false).is_nan());
        assert!(node(f32::INFINITY, false).is_infinite());
    }

    #[test]
    fn history_read_clamps_out_of_range_fractions() {
        let mut history = History::new(48_000.0, 48_000.0);
        history.push([0.5; 2]);
        let negative = history.read(-10.0);
        let positive = history.read(10.0);
        assert!(negative.iter().all(|v| v.is_finite()));
        assert!(positive.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn converter_step_with_zero_input_yields_finite_outputs() {
        let mut converter = Converter::new(48_000.0, 10_000_000.0 / 224.0);
        let wet = converter.step([0.0; 2], |s| [s, [0.0; 2]]);
        assert!(wet.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn converter_runs_one_internal_tick_per_host_frame_at_unity_ratio() {
        let mut converter = Converter::new(48_000.0, 48_000.0);
        let mut count = 0;
        for _ in 0..64 {
            converter.step([0.1; 2], |s| {
                count += 1;
                [s, [0.0; 2]]
            });
        }
        assert_eq!(count, 64);
    }

    #[test]
    fn converter_is_deterministic_for_identical_inputs() {
        let mut a = Converter::new(48_000.0, 10_000_000.0 / 336.0);
        let mut b = Converter::new(48_000.0, 10_000_000.0 / 336.0);
        let mut state = 0x1234_5678_u32;
        for _ in 0..256 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0;
            let input = [x; 2];
            let wa = a.step(input, |s| [s, [s[0] * 0.5, s[1] * 0.5]]);
            let wb = b.step(input, |s| [s, [s[0] * 0.5, s[1] * 0.5]]);
            assert_eq!(wa, wb);
        }
    }

    #[test]
    fn converter_outputs_stay_finite_over_pseudo_random_signal() {
        let mut converter = Converter::new(48_000.0, 10_000_000.0 / 128.0);
        let mut state = 0x9e37_79b9_u32;
        for _ in 0..512 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0;
            let wet = converter.step([x, -x], |s| [s, [s[0] * 0.25, s[1] * 0.25]]);
            assert!(wet.iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn converter_step_keeps_phase_bounded_and_state_finite() {
        let mut converter = Converter::new(48_000.0, 10_000_000.0 / 224.0);
        for i in 0..256 {
            let x = ((i % 7) as f32 - 3.0) * 0.1;
            let wet = converter.step([x; 2], |s| [s, [s[0] * 0.5, s[1] * 0.5]]);
            assert!(wet.iter().flatten().all(|v| v.is_finite()));
            assert!(converter.phase >= 0.0 && converter.phase < 1.0);
            assert!(converter.dc_in.iter().flatten().all(|v| v.is_finite()));
            assert!(converter.dc_out.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
