use crate::voice::{approach, noise, phase_step, pulse, saw, triangle, StateVariable};
use crate::{fast_tanh, v_oct_to_hz, wrap_turns};
use leviathan_core::SOUND_PARAMS;

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    master_phase: f32,
    slave_phase: f32,
    sub_phase: f32,
    amp: f32,
    filter_env: f32,
    filter: StateVariable,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 256);

impl Voice {
    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
        rng: &mut u32,
    ) -> f32 {
        let [wave_mix, interval, cross_sync, cutoff, resonance, mode_drive, env_mod, decay] =
            *params;
        if let Some(velocity) = trigger {
            self.amp = velocity;
            self.filter_env = 0.25 + 0.75 * velocity;
        }
        self.amp = approach(self.amp, 0.0, 0.020 + 1.5 * decay * decay, sample_rate);
        self.filter_env = approach(
            self.filter_env,
            0.0,
            0.006 + 0.6 * decay * decay,
            sample_rate,
        );

        let master_step = phase_step(
            hz * (v_oct_to_hz(interval * 2.0 - 1.0) / 261.625_58),
            sample_rate,
        );
        let next_master = self.master_phase + master_step;
        self.master_phase = wrap_turns(next_master);
        let osc2 = wave(self.master_phase, master_step, 1.0 - wave_mix);
        let fm = 1.8 * cross_sync.min(0.5) * osc2;
        let slave_step = phase_step(hz * (1.0 + fm), sample_rate);
        self.slave_phase = wrap_turns(self.slave_phase + slave_step);
        if next_master >= 1.0 && cross_sync > 0.5 {
            // Advance from the fractional position of the master's wrap.
            let reset_phase = self.master_phase * (slave_step / master_step);
            let phase_delta = wrap_turns(reset_phase - self.slave_phase + 0.5) - 0.5;
            self.slave_phase =
                wrap_turns(self.slave_phase + (2.0 * cross_sync - 1.0) * phase_delta);
        }
        let osc1 = wave(self.slave_phase, slave_step, wave_mix);
        let sub_step = phase_step(hz * 0.5, sample_rate);
        self.sub_phase = wrap_turns(self.sub_phase + sub_step);
        let sub_noise = 0.18 * (1.0 - wave_mix) * pulse(self.sub_phase, sub_step, 0.5)
            + 0.18 * wave_mix * noise(rng);
        let drive = if mode_drive < 0.5 {
            mode_drive * 2.0
        } else {
            (mode_drive - 0.5) * 2.0
        };
        let input = fast_tanh((1.0 + 4.0 * drive) * (0.55 * osc1 + 0.32 * osc2 + sub_noise));
        let cutoff = (cutoff + self.filter_env * env_mod * 0.70).min(1.0);
        let g = 0.008 + 0.84 * cutoff * cutoff;
        let damping = 1.55 - 1.46 * resonance;
        // Keep positive damping at full resonance and apply op-amp slew and
        // saturation in the integrator feedback.
        let filtered = self.filter.step(input, g, damping, 0.1 + 0.9 * drive);
        let output = if mode_drive < 0.5 {
            filtered.low
        } else {
            1.4 * filtered.band
        };
        self.amp * fast_tanh(output).clamp(-1.0, 1.0)
    }
}

#[inline]
fn wave(phase: f32, step: f32, shape: f32) -> f32 {
    let square = pulse(phase, step, 0.5);
    if shape < 0.5 {
        let saw = saw(phase, step);
        saw + (2.0 * shape) * (square - saw)
    } else {
        square + (2.0 * shape - 1.0) * (triangle(phase, step) - square)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::pitch_hz;
    use crate::MachineKind;

    #[test]
    fn maximum_resonance_and_cross_fm_stay_bounded() {
        for sample_rate in [8_000.0, 44_100.0, 48_000.0, 96_000.0] {
            for mode in [0.49, 1.0] {
                for cross_sync in [0.5, 1.0] {
                    let params = [0.82, 1.0, cross_sync, 0.5, 1.0, mode, 1.0, 1.0];
                    let mut voice = Voice::default();
                    let mut rng = 123;
                    for note in 12..=120 {
                        let hz = pitch_hz((note as f32 - 60.0) / 12.0);
                        for i in 0..512 {
                            let sample = voice.step(
                                hz,
                                sample_rate,
                                (i % 256 == 0).then_some(1.0),
                                &params,
                                &mut rng,
                            );
                            assert!(
                                (-1.0..=1.0).contains(&sample),
                                "note {note} at {sample_rate}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bandpass_loses_more_fundamental_than_lowpass() {
        let power_ratio = |mode| {
            let mut params = MachineKind::PolivoksSub.spec().default_params();
            params[1] = 0.5;
            params[2] = 0.0;
            params[3] = 0.45;
            params[4] = 0.35;
            params[5] = mode;
            params[6] = 0.0;
            let mut voice = Voice::default();
            let mut rng = 123;
            let mut low = [0.0_f32; 2];
            let mut high = [0.0_f32; 2];
            for i in 0..8192 {
                let sample =
                    voice.step(187.5, 48_000.0, (i == 0).then_some(1.0), &params, &mut rng);
                if i >= 4096 {
                    let phase = (i % 256) as f32 / 256.0;
                    for (sums, harmonic) in [(&mut low, 1.0), (&mut high, 5.0)] {
                        sums[0] += sample * crate::voice::sine(phase * harmonic);
                        sums[1] += sample * crate::voice::cosine(phase * harmonic);
                    }
                }
            }
            let power = |v: [f32; 2]| v[0] * v[0] + v[1] * v[1];
            power(high) / power(low)
        };
        let lowpass = power_ratio(0.20);
        let bandpass = power_ratio(0.70);
        assert!(bandpass > lowpass * 3.0, "LP {lowpass}, BP {bandpass}");
    }
}
