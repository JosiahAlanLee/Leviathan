use crate::{fast_tanh, v_oct_to_hz, wrap_turns, ParamSpec};
use core::f32::consts::PI;
use leviathan_core::SOUND_PARAMS;

#[inline]
pub(super) fn controls(
    mut params: [f32; SOUND_PARAMS],
    specs: &[ParamSpec; SOUND_PARAMS],
) -> [f32; SOUND_PARAMS] {
    for (value, spec) in params.iter_mut().zip(specs) {
        *value = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            spec.default
        };
    }
    params
}

/// Divide a normalized macro into equal-width selector positions.
#[inline]
pub(super) fn choice(value: f32, count: usize) -> usize {
    ((value * count as f32) as usize).min(count - 1)
}

#[inline]
pub(super) fn approach(value: f32, target: f32, seconds: f32, sample_rate: f32) -> f32 {
    let difference = target - value;
    if difference.abs() < 1e-6 {
        target
    } else {
        value + difference / (1.0 + seconds * sample_rate)
    }
}

#[inline]
pub(super) fn noise(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as i32) as f32 * (1.0 / 2_147_483_648.0)
}

#[inline]
pub(super) const fn sine_cycle(phase: f32) -> f32 {
    let turns = if phase <= 0.25 {
        phase
    } else if phase <= 0.75 {
        0.5 - phase
    } else {
        phase - 1.0
    };
    let x = turns * (2.0 * PI);
    let x2 = x * x;
    x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 - x2 / 5040.0)))
}

#[inline]
pub(super) fn sine(phase: f32) -> f32 {
    sine_cycle(wrap_turns(phase))
}

#[inline]
pub(super) fn cosine(phase: f32) -> f32 {
    sine(phase + 0.25)
}

#[inline]
pub(super) fn phase_step(hz: f32, sample_rate: f32) -> f32 {
    (hz / sample_rate).clamp(0.0, 0.45)
}

#[inline]
pub(super) fn pitch_hz(pitch: f32) -> f32 {
    let pitch = if pitch.is_finite() { pitch } else { 0.0 };
    // Reuse the tracker tuning while extending the legacy upper CV clamp by an octave.
    if pitch > 5.0 {
        v_oct_to_hz(pitch - 1.0) * 2.0
    } else {
        v_oct_to_hz(pitch)
    }
}

// Quadratic correction on either side of a unit step.
#[inline]
fn poly_blep(phase: f32, width: f32) -> f32 {
    if phase < width {
        let x = 1.0 - phase / width;
        -x * x
    } else if phase > 1.0 - width {
        let x = 1.0 - (1.0 - phase) / width;
        x * x
    } else {
        0.0
    }
}

#[inline]
pub(super) fn saw(phase: f32, step: f32) -> f32 {
    2.0 * phase - 1.0 - poly_blep(phase, step)
}

#[inline]
pub(super) fn pulse(phase: f32, step: f32, duty: f32) -> f32 {
    let duty = duty.clamp(step, 1.0 - step);
    let level = if phase < duty { 1.0 } else { -1.0 };
    level + poly_blep(phase, step) - poly_blep(wrap_turns(phase - duty), step)
}

// Integrating the quadratic edge correction gives a cubic correction for slope changes.
#[inline]
fn poly_blamp(phase: f32, width: f32) -> f32 {
    let distance = phase.min(1.0 - phase);
    if distance < width {
        let x = 1.0 - distance / width;
        width * x * x * x / 3.0
    } else {
        0.0
    }
}

#[inline]
pub(super) fn triangle(phase: f32, step: f32) -> f32 {
    1.0 - 4.0 * (phase - 0.5).abs()
        + 4.0 * (poly_blamp(phase, step) - poly_blamp(wrap_turns(phase + 0.5), step))
}

#[derive(Clone, Debug, Default)]
pub(super) struct StateVariable {
    band_state: f32,
    low_state: f32,
}

pub(super) struct FilterOutput {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

impl StateVariable {
    #[inline]
    pub(super) fn step(&mut self, input: f32, g: f32, damping: f32, drive: f32) -> FilterOutput {
        let a = 1.0 / (1.0 + g * (g + damping));
        let band = a * (self.band_state + g * (input - self.low_state));
        let low = self.low_state + g * band;
        let next_band = 2.0 * band - self.band_state;
        let next_low = 2.0 * low - self.low_state;
        let [next_band, next_low] = if drive > 0.0 {
            let rail = 1.25 + 1.75 * (1.0 - drive);
            let slew = (1.0 + 2.0 * g) * (1.0 - 0.65 * drive);
            [
                op_amp_step(self.band_state, next_band, rail, slew),
                op_amp_step(self.low_state, next_low, rail, slew),
            ]
        } else {
            [next_band, next_low]
        };
        self.band_state = flush_denormal(next_band);
        self.low_state = flush_denormal(next_low);
        FilterOutput {
            low,
            band,
            high: input - damping * band - low,
        }
    }
}

#[inline]
fn op_amp_step(previous: f32, target: f32, rail: f32, slew: f32) -> f32 {
    let limited = previous + (target - previous).clamp(-slew, slew);
    rail * fast_tanh(limited / rail)
}

#[inline]
fn flush_denormal(value: f32) -> f32 {
    if value.abs() < 1e-20 {
        0.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_has_unit_amplitude_and_the_expected_period() {
        for (phase, expected) in [(0.0, 0.0), (0.25, 1.0), (0.5, 0.0), (0.75, -1.0)] {
            assert!((sine(phase) - expected).abs() < 0.001);
            assert!((sine(phase + 3.0) - expected).abs() < 0.001);
        }
    }

    #[test]
    fn polynomial_edge_smooths_the_saw_reset() {
        let step = 0.2_f32;
        let mut raw_jump = 0.0_f32;
        let mut corrected_jump = 0.0_f32;
        let mut previous = 0.0_f32;
        let mut previous_saw = 0.0_f32;
        for i in 0..1024 {
            let phase = wrap_turns(0.07 + (i % 5) as f32 * step);
            let raw = 2.0 * phase - 1.0;
            let corrected = saw(phase, step);
            raw_jump = raw_jump.max((raw - previous).abs());
            corrected_jump = corrected_jump.max((corrected - previous_saw).abs());
            previous = raw;
            previous_saw = corrected;
        }
        assert!(corrected_jump < raw_jump * 0.75);
    }
}

// ---- sample-rate independence ----------------------------------------------
//
// Many voices were tuned with per-sample coefficients at 48 kHz. These map a
// coefficient tuned at 48 kHz to the same time constant or cutoff at `rate`.
// At exactly 48 kHz each returns its input unchanged, so 48 kHz output stays
// bit-identical.

pub(super) const REFERENCE_RATE: f32 = 48_000.0;

#[inline]
fn log2_fast(x: f32) -> f32 {
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000); // [1, 2)
    // log2 on [1, 2) by a series in t = (m - 1) / (m + 1): within 1e-6.
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let series = t * (2.0 + t2 * (2.0 / 3.0 + t2 * (2.0 / 5.0 + t2 * (2.0 / 7.0 + t2 * (2.0 / 9.0)))));
    exponent as f32 + series * core::f32::consts::LOG2_E
}

#[inline]
fn exp2_fast(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let whole = if x >= 0.0 { x as i32 } else { x as i32 - 1 };
    let f = x - whole as f32;
    let poly = 1.0
        + f * (0.693_147_2
            + f * (0.240_226_5
                + f * (0.055_504_1
                    + f * (0.009_618_13
                        + f * (0.001_333_36 + f * (0.000_154_04 + f * (0.000_015_25 + f * 0.000_001_32)))))));
    poly * f32::from_bits(((whole + 127) as u32) << 23)
}

/// `base` raised to the 48 kHz / `rate` power: a per-sample multiplier tuned
/// at 48 kHz (`y *= base`) giving the same decay time at `rate`.
#[inline]
pub(super) fn rate_mult(base: f32, rate: f32) -> f32 {
    if rate == REFERENCE_RATE || base <= 0.0 || base == 1.0 || !base.is_finite() {
        return base;
    }
    exp_fast(ln_fast(base) * (REFERENCE_RATE / rate))
}

/// e^y with full relative precision for small |y| (where decays live).
#[inline]
fn exp_fast(y: f32) -> f32 {
    if y.abs() < 0.5 {
        let mut term = 1.0;
        let mut sum = 1.0;
        let mut n = 1.0;
        while n < 12.0 {
            term *= y / n;
            sum += term;
            n += 1.0;
        }
        sum
    } else {
        exp2_fast(y * core::f32::consts::LOG2_E)
    }
}

/// Natural log with full relative precision near 1, where per-sample
/// multipliers live.
#[inline]
fn ln_fast(x: f32) -> f32 {
    let e = 1.0 - x;
    if e.abs() < 0.3 {
        // -ln(1 - e) = e + e^2/2 + e^3/3 + ...; terms fall below 1e-7 by e^14 at |e| < 0.3.
        let mut term = e;
        let mut sum = 0.0;
        let mut n = 1.0;
        while n < 16.0 {
            sum += term / n;
            term *= e;
            n += 1.0;
        }
        -sum
    } else {
        log2_fast(x) * core::f32::consts::LN_2
    }
}

/// One-pole smoothing coefficient tuned at 48 kHz (`y += c * (x - y)`), at `rate`.
#[inline]
pub(super) fn rate_coef(c: f32, rate: f32) -> f32 {
    if rate == REFERENCE_RATE || c <= 0.0 || c >= 1.0 || !c.is_finite() {
        return c;
    }
    1.0 - rate_mult(1.0 - c, rate)
}

/// TPT / state-variable `g` (tan of the warped cutoff) tuned at 48 kHz, at `rate`.
#[inline]
pub(super) fn rate_g(g: f32, rate: f32) -> f32 {
    if rate == REFERENCE_RATE || g <= 0.0 || !g.is_finite() {
        return g;
    }
    // Cutoff in turns of the sample rate, then re-warp at the new rate.
    let angle = atan_fast(g) * (REFERENCE_RATE / rate);
    let angle = angle.min(0.49 * core::f32::consts::PI);
    let turns = angle / (2.0 * core::f32::consts::PI);
    sine(turns) / cosine(turns).max(1e-6)
}

/// A count of samples tuned at 48 kHz, at `rate`.
#[inline]
pub(super) fn rate_samples(n: f32, rate: f32) -> f32 {
    if rate == REFERENCE_RATE {
        n
    } else {
        n * (rate / REFERENCE_RATE)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct GritClock(f32);

impl GritClock {
    pub(super) const fn new() -> Self {
        Self(0.0)
    }

    /// Return the effective scan rate on an update. Carry the sub-sample
    /// remainder so the whole interval scales, including the update sample.
    #[inline]
    pub(super) fn step(&mut self, skipped_at_48k: u8, rate: f32) -> Option<f32> {
        if self.0 > 0.0 {
            self.0 -= 1.0;
            return None;
        }
        if skipped_at_48k == 0 {
            self.0 = 0.0;
            return Some(rate);
        }
        let period = rate_samples(f32::from(skipped_at_48k) + 1.0, rate).max(1.0);
        self.0 += period - 1.0;
        Some(rate / period)
    }
}

/// Amplitude for per-sample white noise so its in-band level matches 48 kHz.
#[inline]
pub(super) fn rate_noise(rate: f32) -> f32 {
    if rate == REFERENCE_RATE {
        1.0
    } else {
        // sqrt(rate / 48 kHz)
        exp2_fast(0.5 * log2_fast(rate / REFERENCE_RATE))
    }
}

#[inline]
fn atan_fast(x: f32) -> f32 {
    // atan for x >= 0 by range reduction to [0, 1]: within 1e-5 rad.
    let (y, flip) = if x > 1.0 { (1.0 / x, true) } else { (x, false) };
    let y2 = y * y;
    let r = y * (0.999_866 + y2 * (-0.330_299_5 + y2 * (0.180_141 + y2 * (-0.085_133 + y2 * 0.020_835_1))));
    if flip { core::f32::consts::FRAC_PI_2 - r } else { r }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn grit_clock_keeps_the_full_update_period_at_each_sample_rate() {
        for rate in [44_100, 48_000, 96_000] {
            for skipped in [0, 1, 4, 15] {
                let target = if skipped == 0 {
                    rate as f32
                } else {
                    REFERENCE_RATE / (f32::from(skipped) + 1.0)
                };
                let mut clock = GritClock::new();
                let mut updates = 0;
                for sample in 0..rate {
                    let update = clock.step(skipped, rate as f32);
                    if rate == 48_000 {
                        assert_eq!(update.is_some(), sample % (u32::from(skipped) + 1) == 0);
                    }
                    if let Some(scan_rate) = update {
                        assert!((scan_rate - target).abs() < 0.01);
                        updates += 1;
                    }
                }
                assert!((updates as f32 - target).abs() <= 1.0);
            }
        }
    }

    #[test]
    fn conversions_are_identity_at_48k_and_preserve_time_constants() {
        for c in [0.001, 0.01, 0.2, 0.7] {
            assert_eq!(rate_coef(c, 48_000.0).to_bits(), c.to_bits());
            // Two steps at 96 kHz decay as much as one at 48 kHz.
            let c96 = rate_coef(c, 96_000.0);
            assert!(((1.0 - c96) * (1.0 - c96) - (1.0 - c)).abs() < 1e-4, "{c}");
        }
        for g in [0.01f32, 0.1, 0.5, 0.78] {
            assert_eq!(rate_g(g, 48_000.0).to_bits(), g.to_bits());
            let f48 = g.atan() * 48_000.0;
            let f96 = rate_g(g, 96_000.0).atan() * 96_000.0;
            assert!((f96 / f48 - 1.0).abs() < 2e-3, "{g}: {f48} vs {f96}");
        }
        assert!((rate_noise(96_000.0) - 2f32.sqrt()).abs() < 1e-4);
        for d in [0.5f32, 0.9, 0.999, 0.99999, 0.999_999] {
            let got = rate_mult(d, 96_000.0);
            let want = d.sqrt();
            // Compare decay times (1 / -ln), not raw values.
            let (t_got, t_want) = (-1.0 / got.ln(), -1.0 / want.ln());
            assert!((t_got / t_want - 1.0).abs() < 1e-3, "{d}: {got} vs {want}");
        }
    }
}
