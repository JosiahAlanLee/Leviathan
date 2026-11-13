use leviathan_core::SoundLock;

use crate::{fast_sin_turns, lock_param, wrap_turns};

const RATIO_TABLE: [f32; 10] = [
    0.5, 1.0, 1.414_213_5, 1.5, 2.0, 2.732_050_8, 3.0, 4.0, 5.196_152_4, 7.0,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmAlgorithm {
    /// `Op4 -> Op3 -> Op2 -> Op1` (deep metallic/industrial cascade)
    Cascade = 0,
    /// `(Op4 -> Op3) + (Op2 -> Op1)` (dual 2-op pluck + warm body)
    DualPairs = 1,
    /// `(Op4 + Op3 + Op2) -> Op1` (3 parallel modulators into 1 carrier)
    TripleStack = 2,
    /// `Op1 + Op2 + Op3 + Op4` (4-carrier harmonic/organ cluster)
    Additive = 3,
}

impl FmAlgorithm {
    #[inline]
    pub fn from_norm(v: f32) -> Self {
        match ((v.clamp(0.0, 0.999) * 4.0) as usize) % 4 {
            0 => Self::Cascade,
            1 => Self::DualPairs,
            2 => Self::TripleStack,
            _ => Self::Additive,
        }
    }
}

/// Clean-room 4-operator phase-modulation synth voice (`#![no_std]`).
///
/// SoundLock mapping (`0.0..=1.0`):
/// - `P0 (bit 0)`: Carrier decay (handled with voice envelope)
/// - `P1 (bit 1)`: FM modulation index (`0.0..2.2` turns)
/// - `P2 (bit 2)`: Post-voice filter cutoff (handled by host rack)
/// - `P3 (bit 3)`: FM algorithm (`0..=3`: Cascade, DualPairs, TripleStack, Additive)
/// - `P4 (bit 4)`: Modulator harmonic/inharmonic ratio (`0.5x..7.0x`)
/// - `P5 (bit 5)`: Modulator envelope decay (`0.02s..0.75s` pluck vs sustain)
/// - `P6 (bit 6)`: Op4 self-feedback (`0.0..0.45` turns)
/// - `P7 (bit 7)`: Upper operator detune spread
#[derive(Clone, Debug, Default)]
pub struct FourOpFmVoice {
    phases: [f32; 4],
    mod_env: f32,
    prev_op4: f32,
}

impl FourOpFmVoice {
    pub fn trigger(&mut self) {
        self.mod_env = 1.0;
    }

    pub fn step_sample(&mut self, base_hz: f32, host_sr: f32, amp_env: f32, lock: &SoundLock) -> f32 {
        self.render(base_hz, host_sr, amp_env, lock, false)
    }

    pub(super) fn step_calibrated(
        &mut self,
        base_hz: f32,
        host_sr: f32,
        amp_env: f32,
        lock: &SoundLock,
    ) -> f32 {
        self.render(base_hz, host_sr, amp_env, lock, true)
    }

    fn render(
        &mut self,
        base_hz: f32,
        host_sr: f32,
        amp_env: f32,
        lock: &SoundLock,
        calibrated: bool,
    ) -> f32 {
        let dt = 1.0 / host_sr.max(8_000.0);

        let mod_idx = lock_param(lock, 1, 0.35).clamp(0.0, 1.0) * 1.65;
        let alg = FmAlgorithm::from_norm(lock_param(lock, 3, 0.28));

        let ratio_idx = ((lock_param(lock, 4, 0.45).clamp(0.0, 0.999) * RATIO_TABLE.len() as f32)
            as usize)
            .min(RATIO_TABLE.len() - 1);
        let mod_ratio = RATIO_TABLE[ratio_idx];

        let mod_decay = 0.025 + 0.70 * lock_param(lock, 5, 0.28).clamp(0.0, 1.0);
        self.mod_env *= 1.0 - (dt / mod_decay);

        let fb = lock_param(lock, 6, 0.18).clamp(0.0, 1.0) * 0.38;
        let ratios = operator_ratios(mod_ratio, lock, calibrated);

        for (phase, &r) in self.phases.iter_mut().zip(ratios.iter()) {
            let f = (base_hz * r).clamp(16.0, host_sr * 0.44);
            *phase = wrap_turns(*phase + f * dt);
        }

        let op4 = fast_sin_turns(self.phases[3] + fb * self.prev_op4);
        self.prev_op4 = op4;

        let idx_now = mod_idx * (0.20 + 0.80 * self.mod_env);

        let out = match alg {
            FmAlgorithm::Cascade => {
                let op3 = fast_sin_turns(self.phases[2] + idx_now * 0.75 * op4);
                let op2 = fast_sin_turns(self.phases[1] + idx_now * op3);
                fast_sin_turns(self.phases[0] + idx_now * op2)
            }
            FmAlgorithm::DualPairs => {
                let pair_hi = fast_sin_turns(self.phases[2] + idx_now * 0.85 * op4) * self.mod_env;
                let pair_lo = fast_sin_turns(self.phases[0] + idx_now * fast_sin_turns(self.phases[1]));
                0.62 * pair_lo + 0.38 * pair_hi
            }
            FmAlgorithm::TripleStack => {
                let mod_sum = (op4
                    + fast_sin_turns(self.phases[2])
                    + fast_sin_turns(self.phases[1]))
                    * (idx_now * 0.45);
                fast_sin_turns(self.phases[0] + mod_sum)
            }
            FmAlgorithm::Additive => {
                let c1 = fast_sin_turns(self.phases[0]);
                let c2 = fast_sin_turns(self.phases[1]) * 0.65;
                let c3 = fast_sin_turns(self.phases[2]) * 0.45 * self.mod_env;
                let c4 = op4 * 0.35 * self.mod_env;
                (c1 + c2 + c3 + c4) * 0.48
            }
        };

        out * amp_env
    }
}

fn operator_ratios(mod_ratio: f32, lock: &SoundLock, calibrated: bool) -> [f32; 4] {
    let high_ratio = (mod_ratio * 2.0).min(12.0);
    let top_ratio = (mod_ratio * 1.5).min(12.0);
    if calibrated {
        let detune = 2.0 * lock_param(lock, 7, 0.5).clamp(0.0, 1.0) - 1.0;
        // Modulators are reciprocal around the center. The base carrier stays at the played pitch.
        let spread = 1.0 + 0.015 * detune.abs();
        let up = if detune >= 0.0 { spread } else { 1.0 / spread };
        return [1.0, mod_ratio * up, high_ratio / up, top_ratio * up];
    }
    let detune = lock_param(lock, 7, 0.12).clamp(0.0, 1.0) * 0.018;
    [
        1.0,
        mod_ratio * (1.0 + detune),
        high_ratio * (1.0 - detune),
        top_ratio * (1.0 + 1.5 * detune),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_detune_keeps_the_carrier_at_the_played_pitch() {
        let mut lock = SoundLock {
            mask: 1 << 7,
            ..SoundLock::default()
        };
        lock.values[7] = 0.5;
        assert_eq!(operator_ratios(2.0, &lock, true), [1.0, 2.0, 4.0, 3.0]);
        lock.values[7] = 0.0;
        let flat = operator_ratios(2.0, &lock, true);
        lock.values[7] = 1.0;
        let sharp = operator_ratios(2.0, &lock, true);
        assert_eq!(flat[0], 1.0);
        assert_eq!(sharp[0], 1.0);
        for (index, nominal) in [2.0, 4.0, 3.0].into_iter().enumerate() {
            assert!((flat[index + 1] * sharp[index + 1] - nominal * nominal).abs() < 1e-6);
        }
    }

    #[test]
    fn all_four_fm_algorithms_produce_finite_distinct_waveforms() {
        let mut energies = [0.0_f32; 4];
        for (alg_idx, slot) in energies.iter_mut().enumerate() {
            let mut voice = FourOpFmVoice::default();
            voice.trigger();
            let lock = SoundLock {
                machine: 1,
                mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 6),
                values: [0.5, 0.75, 0.6, (alg_idx as f32 + 0.1) / 4.0, 0.55, 0.4, 0.65, 0.2],
            };
            let mut sum = 0.0_f32;
            for _ in 0..256 {
                let s = voice.step_sample(110.0, 48_000.0, 0.9, &lock);
                assert!(s.is_finite() && s.abs() <= 1.2);
                sum += s.abs();
            }
            assert!(sum > 1.0);
            *slot = sum;
        }
        assert!((energies[0] - energies[3]).abs() > 0.05);
    }
}
