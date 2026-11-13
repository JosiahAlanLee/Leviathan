//! Converter word formats and host-rate rendering for opt-in DAC coloration.

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

const YM_OPM_RATE_HZ: f32 = 3_579_545.0 / 64.0;
const CZ_RATE_HZ: f32 = 40_000.0;

/// Range selector for the provisional Casio-style compander.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CzRange {
    Instantaneous,
    Syllabic,
}

/// Word format and reconstruction settings for a single audio stream.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ConverterProfile {
    #[default]
    Bypass,
    R2rOffset {
        skew: f32,
    },
    Ym3012Fp,
    CzProvisional {
        range: CzRange,
        track: f32,
    },
    LinearSigned {
        bits: u8,
    },
}

/// Encodes signed PCM into the YM3012 serial fields, with the first three bits cleared.
/// Bits 3–12 hold the offset-binary mantissa and bits 13–15 hold the exponent.
#[inline]
pub const fn encode_ym3012_word(pcm: i16) -> u16 {
    let sign_removed = if pcm < 0 { !pcm } else { pcm } as u16;
    let shift = (u16::BITS - sign_removed.leading_zeros()).saturating_sub(9);
    let mantissa = ((pcm >> shift) + 512) as u16;
    (((7 - shift) as u16) << 13) | (mantissa << 3)
}

/// Reconstructs the integer PCM level. Exponent zero is reserved.
#[inline]
pub const fn decode_ym3012_word(word: u16) -> Option<i16> {
    let exponent = word >> 13;
    if exponent == 0 {
        return None;
    }
    let mantissa = ((word >> 3) & 0x03ff) as i16 - 512;
    Some(mantissa << (7 - exponent))
}

/// Keeps 4–16 signed bits, left-aligned in the original 16-bit range.
#[inline]
pub fn quantize_linear_i16(pcm: i16, bits: u8) -> i16 {
    let drop = 16 - bits.clamp(4, 16);
    (pcm >> drop) << drop
}

#[inline]
fn pcm16(input: f32) -> i16 {
    let scaled = input * 32_768.0;
    let half = if scaled >= 0.0 { 0.5 } else { -0.5 };
    (scaled + half) as i16
}

#[inline]
fn normalized(value: f32) -> f32 {
    value.max(0.0).min(1.0)
}

fn cz_quantize(input: f32, reference: f32, track: f32) -> f32 {
    let pcm = (input * 8_192.0).clamp(-8_192.0, 8_191.0) as i32;
    let shift = if reference >= 0.5 {
        2
    } else if reference >= 0.25 {
        1
    } else {
        0
    };
    let track = normalized(track);
    let gain = match shift {
        1 => 1.0 + 0.085 * track,
        2 => 1.0 - 0.115 * track,
        _ => 1.0,
    };
    let truncated = ((pcm >> shift) << shift) as f32;
    truncated * (gain / 8_192.0)
}

/// Inline state for prefilter, latch, compander and output reconstruction.
#[derive(Clone, Copy, Debug)]
pub struct DacStage {
    phase: f32,
    prefiltered: f32,
    held: f32,
    envelope: f32,
    reconstructed: f32,
}

const _: () = assert!(core::mem::size_of::<DacStage>() <= 32);

impl Default for DacStage {
    fn default() -> Self {
        Self {
            phase: 1.0,
            prefiltered: 0.0,
            held: 0.0,
            envelope: 0.0,
            reconstructed: 0.0,
        }
    }
}

impl DacStage {
    /// Processes a normalized sample. Clock hold reduces the host-capped device rate
    /// by up to 32×. A one-pole prefilter reduces foldover when the latch rate drops.
    /// A separate one-pole reconstruction filter follows the held word.
    #[inline]
    pub fn step(
        &mut self,
        input: f32,
        sample_rate: f32,
        profile: ConverterProfile,
        clock_hold: f32,
    ) -> f32 {
        let (device_rate, reconstruction_hz) = match profile {
            ConverterProfile::Bypass => {
                *self = Self::default();
                return input;
            }
            ConverterProfile::Ym3012Fp => (YM_OPM_RATE_HZ, 21_000.0),
            ConverterProfile::CzProvisional { .. } => (CZ_RATE_HZ, 20_000.0),
            _ => (sample_rate, 22_000.0),
        };
        let rate = if sample_rate.is_finite() {
            sample_rate.max(8_000.0)
        } else {
            48_000.0
        };
        let hold = normalized(clock_hold);
        let clock = (device_rate.min(rate) / (1.0 + 31.0 * hold * hold)).clamp(500.0, rate);
        let input = if input.is_finite() {
            input.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if clock < rate {
            self.prefiltered +=
                one_pole_coefficient(0.45 * clock, rate) * (input - self.prefiltered);
        } else {
            self.prefiltered = input;
        }

        let magnitude = self.prefiltered.abs();
        self.envelope = if matches!(
            profile,
            ConverterProfile::CzProvisional {
                range: CzRange::Syllabic,
                ..
            }
        ) {
            let seconds = if magnitude > self.envelope {
                0.001
            } else {
                0.050
            };
            approach(self.envelope, magnitude, seconds, rate)
        } else {
            0.0
        };
        self.phase += clock / rate;
        if self.phase >= 1.0 {
            self.phase -= self.phase as u32 as f32;
            let sample = self.prefiltered;
            self.held = match profile {
                ConverterProfile::Bypass => sample,
                ConverterProfile::R2rOffset { skew } => {
                    let code = (sample * 128.0 + 128.5) as u8;
                    ladder(code, normalized(skew))
                }
                ConverterProfile::Ym3012Fp => {
                    let word = encode_ym3012_word(pcm16(sample));
                    f32::from(decode_ym3012_word(word).unwrap_or(0)) / 32_768.0
                }
                ConverterProfile::CzProvisional { track, .. } => {
                    cz_quantize(sample, magnitude.max(self.envelope), track)
                }
                ConverterProfile::LinearSigned { bits } => {
                    f32::from(quantize_linear_i16(pcm16(sample), bits)) / 32_768.0
                }
            };
        }
        self.reconstructed +=
            one_pole_coefficient(reconstruction_hz, rate) * (self.held - self.reconstructed);
        self.reconstructed
    }
}

#[inline]
fn one_pole_coefficient(corner_hz: f32, sample_rate: f32) -> f32 {
    let frequency = core::f32::consts::TAU * corner_hz;
    frequency / (sample_rate + frequency)
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

    #[test]
    fn ym3012_serial_fields_match_signed_pcm_vectors() {
        for (pcm, word, reconstructed) in [
            (0, 0xf000, 0),
            (1, 0xf008, 1),
            (-1, 0xeff8, -1),
            (511, 0xfff8, 511),
            (512, 0xd800, 512),
            (-512, 0xe000, -512),
            (-513, 0xc7f8, -514),
            (1_023, 0xdff8, 1_022),
            (1_024, 0xb800, 1_024),
            (16_384, 0x3800, 16_384),
            (-16_384, 0x4000, -16_384),
            (i16::MAX, 0x3ff8, 32_704),
            (i16::MIN, 0x2000, i16::MIN),
        ] {
            assert_eq!(encode_ym3012_word(pcm), word);
            assert_eq!(decode_ym3012_word(word | 0b111), Some(reconstructed));
        }
        assert_eq!(decode_ym3012_word(0x1fff), None);
    }

    #[test]
    fn ym3012_pcm_roundtrip_floors_to_the_smallest_available_step() {
        for pcm in i16::MIN..=i16::MAX {
            let input = i32::from(pcm);
            let step = [1, 2, 4, 8, 16, 32, 64]
                .into_iter()
                .find(|&step| (-512..=511).contains(&input.div_euclid(step)))
                .unwrap();
            let expected = input.div_euclid(step) * step;
            let word = encode_ym3012_word(pcm);
            assert_eq!(decode_ym3012_word(word), Some(expected as i16), "{input}");
        }
    }

    #[test]
    fn signed_word_truncation_preserves_msb_alignment() {
        for (input, bits, output) in [
            (0x1234, 16, 0x1234),
            (i16::MIN, 16, i16::MIN),
            (0x123f, 12, 0x1230),
            (-1, 12, -16),
            (0x12ff, 8, 0x1200),
            (-128, 8, -256),
            (0x3fff, 4, 0x3000),
            (0x3fff, 0, 0x3000),
            (i16::MAX, 32, i16::MAX),
        ] {
            assert_eq!(quantize_linear_i16(input, bits), output);
        }
    }

    #[test]
    fn cz_range_reference_and_tracking_are_independent() {
        let mut instant = DacStage::default();
        let mut syllabic = DacStage::default();
        let i = ConverterProfile::CzProvisional {
            range: CzRange::Instantaneous,
            track: 0.0,
        };
        let s = ConverterProfile::CzProvisional {
            range: CzRange::Syllabic,
            track: 0.0,
        };
        for _ in 0..480 {
            instant.step(0.9, 40_000.0, i, 0.0);
            syllabic.step(0.9, 40_000.0, s, 0.0);
        }
        let quiet = 3.0 / 8_192.0;
        instant.step(quiet, 40_000.0, i, 0.0);
        syllabic.step(quiet, 40_000.0, s, 0.0);
        assert_eq!(instant.held, quiet);
        assert_eq!(syllabic.held, 0.0);
        assert!(cz_quantize(0.35, 0.35, 1.0) > cz_quantize(0.35, 0.35, 0.0));
        assert!(cz_quantize(0.75, 0.75, 1.0) < cz_quantize(0.75, 0.75, 0.0));
    }

    #[test]
    fn prefilter_attenuates_a_tone_that_folds_into_the_passband() {
        let rate = 48_000.0_f32;
        let hold = 0.8_f32;
        let profile = ConverterProfile::LinearSigned { bits: 16 };
        let clock = rate / (1.0 + 31.0 * hold * hold);
        let passband = 220.0;
        let amplitude = |input_hz: f32| {
            let mut stage = DacStage::default();
            let (mut real, mut imaginary) = (0.0_f32, 0.0_f32);
            for n in 0..52_800 {
                let time = n as f32 / rate;
                let input = 0.8 * (core::f32::consts::TAU * input_hz * time).sin();
                let output = stage.step(input, rate, profile, hold);
                if n >= 4_800 {
                    let angle = core::f32::consts::TAU * passband * time;
                    real += output * angle.cos();
                    imaginary += output * angle.sin();
                }
            }
            real.hypot(imaginary) / 24_000.0
        };
        let pass = amplitude(passband);
        let folded = amplitude(clock + passband);
        assert!(pass > 0.7 && folded < 0.5 * pass, "{pass} {folded}");
    }

    #[test]
    fn reconstruction_moves_between_latches_and_bypass_resets_state() {
        let mut stage = DacStage::default();
        let profile = ConverterProfile::R2rOffset { skew: 0.0 };
        for _ in 0..256 {
            stage.step(0.5, 48_000.0, profile, 1.0);
        }
        // Start at a latch boundary. Keep changing input until the next latch.
        stage.phase = 1.0 - 1.0 / 32.0;
        let previous = stage.step(-0.5, 48_000.0, profile, 1.0);
        let held = stage.held;
        let output = stage.step(-0.5, 48_000.0, profile, 1.0);
        assert_eq!(stage.held, held);
        assert!(held < output && output < previous);
        assert_eq!(
            stage.step(-0.3, 48_000.0, ConverterProfile::Bypass, 1.0),
            -0.3
        );
        assert_eq!(stage.step(0.0, 48_000.0, profile, 1.0), 0.0);
    }
}
