use crate::voice::approach;

/// Eight-bit offset-binary transfer with a calibrated midscale and skewed bit weights.
fn ladder(code: u8, skew: f32) -> f32 {
    const WEIGHT_ERROR: [f32; 8] = [0.35, -0.48, 0.9, -1.8, 3.1, -4.4, 7.0, -5.4];
    let mut error = 0.0;
    for (bit, weight) in WEIGHT_ERROR.into_iter().enumerate() {
        error += weight * f32::from((code >> bit) & 1);
    }
    let centered_error = error - WEIGHT_ERROR[7];
    ((f32::from(code) - 128.0 + skew * centered_error) / 128.0).clamp(-1.0, 1.0)
}

#[derive(Clone, Debug)]
pub(super) struct R2r {
    clock_phase: f32,
    code: u8,
    held: f32,
    slew: f32,
    glitch: f32,
}

impl Default for R2r {
    fn default() -> Self {
        Self {
            clock_phase: 1.0,
            code: 0x80,
            held: 0.0,
            slew: 0.0,
            glitch: 0.0,
        }
    }
}

impl R2r {
    #[inline]
    pub(super) fn step(&mut self, input: f32, rate: f32, hold: f32, skew: f32, color: f32) -> f32 {
        self.clock_phase += 1.0 / (1.0 + 31.0 * hold * hold);
        if self.clock_phase >= 1.0 {
            self.clock_phase -= self.clock_phase as u32 as f32;
            let code = (input * 128.0 + 128.5).clamp(0.0, 255.0) as u8;
            let flipped = code ^ self.code;
            if flipped & 0x80 != 0 {
                let carry = flipped.count_ones().saturating_sub(3) as f32;
                let polarity = if code > self.code { 1.0 } else { -1.0 };
                self.glitch = (self.glitch + 0.035 * color * carry * polarity).clamp(-0.35, 0.35);
            }
            self.code = code;
            self.held = ladder(code, skew);
        }
        self.slew += (self.held - self.slew) / (1.0 + rate * 0.000_35 * color);
        let output = self.slew + self.glitch;
        self.glitch = approach(self.glitch, 0.0, 0.000_13, rate);
        output.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_uses_256_codes_and_is_finite_and_dc_bounded_with_skew() {
        for skew in [0.0, 0.2, 1.0] {
            let mut sum = 0.0;
            for code in 0..=255 {
                let level = ladder(code, skew);
                assert!((-1.0..=1.0).contains(&level));
                sum += level;
                if skew == 0.0 {
                    assert_eq!(level, (f32::from(code) - 128.0) / 128.0);
                }
            }
            assert_eq!(ladder(0x80, skew), 0.0);
            assert!((sum / 256.0).abs() < 0.04);
        }
    }

    #[test]
    fn major_carry_adds_only_a_transient_and_hold_rejects_intermediate_codes() {
        let mut dac = R2r::default();
        for _ in 0..256 {
            dac.step(-1.0 / 128.0, 48_000.0, 0.0, 0.0, 1.0);
        }
        let edge = dac.step(0.0, 48_000.0, 0.0, 0.0, 1.0);
        assert!(edge > 0.15);
        let mut settled = edge;
        for _ in 0..1_024 {
            settled = dac.step(0.0, 48_000.0, 0.0, 0.0, 1.0);
        }
        assert!(settled.abs() < 1e-6);

        let mut dac = R2r::default();
        dac.step(0.0, 48_000.0, 1.0, 0.0, 0.0);
        for _ in 0..30 {
            assert_eq!(dac.step(0.5, 48_000.0, 1.0, 0.0, 0.0), 0.0);
        }
        assert_eq!(dac.code, 0x80);
        dac.step(0.5, 48_000.0, 1.0, 0.0, 0.0);
        assert_eq!(dac.code, 0xc0);
    }
}
