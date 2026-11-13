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
