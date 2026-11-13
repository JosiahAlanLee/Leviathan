use crate::voice::{approach, choice, noise, phase_step, pulse, sine, StateVariable};
use crate::{fast_tanh, wrap_turns};
use leviathan_core::SOUND_PARAMS;

const CLAP_SPACING: f32 = 0.010;

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    phases: [f32; 6],
    age: u32,
    velocity: f32,
    body: f32,
    wire: f32,
    sweep: f32,
    band: StateVariable,
    highpass: StateVariable,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 128);

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
        let [decay, mode, tune, snappy, spread, tone, drive, pitch_env] = *params;
        if let Some(velocity) = trigger {
            self.age = 0;
            self.velocity = velocity;
            self.body = velocity;
            self.wire = velocity;
            self.sweep = 1.0;
            self.phases = [0.0; 6];
        }
        let clap_spacing = (sample_rate * CLAP_SPACING + 0.5) as u32;
        let mode = choice(mode, 4);
        let clap_pending = mode == 1 && self.velocity > 0.0 && self.age < 4 * clap_spacing;
        if self.body == 0.0 && self.wire == 0.0 && !clap_pending {
            return 0.0;
        }
        let decay = decay * decay;
        self.sweep = approach(self.sweep, 0.0, 0.007, sample_rate);
        let body_hz = (hz * (0.45 + 1.3 * tune)).clamp(40.0, 1800.0);
        let cutoff = 0.035 + 0.70 * tone * tone;
        let raw = match mode {
            0 => {
                let swept = body_hz * (1.0 + 3.2 * pitch_env * self.sweep);
                let first = sine(self.advance(0, swept, sample_rate));
                let second = sine(self.advance(1, swept * 1.52, sample_rate));
                let wires = self.band.step(noise(rng), cutoff, 0.82, 0.0).band;
                self.body = approach(self.body, 0.0, 0.018 + 0.34 * decay, sample_rate);
                self.wire = approach(self.wire, 0.0, 0.027 + 0.58 * decay, sample_rate);
                let membrane = 0.66 * first + 0.25 * (0.36 + 0.64 * self.sweep) * second;
                membrane * self.body * (1.0 - 0.64 * snappy) + 1.5 * snappy * wires * self.wire
            }
            1 => {
                let texture = self.band.step(noise(rng), cutoff, 0.53, 0.0).band;
                let burst = clap_burst(self.age, clap_spacing);
                self.wire = approach(self.wire, 0.0, 0.037 + 0.50 * decay, sample_rate);
                self.body = 0.0;
                texture * ((0.8 + 0.9 * snappy) * self.velocity * burst + 0.28 * self.wire)
            }
            2 => {
                let swept = 2.3 * body_hz * (1.0 + 0.5 * pitch_env * self.sweep);
                let first = sine(self.advance(0, swept, sample_rate));
                let second = sine(self.advance(1, swept * (1.55 + 0.5 * spread), sample_rate));
                let click = self.band.step(noise(rng), cutoff, 0.44, 0.0).band;
                self.body = approach(self.body, 0.0, 0.009 + 0.17 * decay, sample_rate);
                self.wire = approach(self.wire, 0.0, 0.003 + 0.014 * decay, sample_rate);
                (0.53 * first + 0.39 * second) * self.body + 0.80 * snappy * click * self.wire
            }
            _ => {
                const RATIOS: [f32; 6] = [1.0, 1.342, 1.637, 2.043, 2.488, 2.937];
                let base = (body_hz * 2.5).clamp(160.0, 1800.0);
                let mut square = [0.0; 6];
                for (lane, (out, ratio)) in square.iter_mut().zip(RATIOS).enumerate() {
                    let step =
                        phase_step(base * (ratio + spread * lane as f32 * 0.127), sample_rate);
                    self.phases[lane] = wrap_turns(self.phases[lane] + step);
                    *out = pulse(self.phases[lane], step, 0.5);
                }
                let metal = 0.37 * square[0] * square[3]
                    + 0.32 * square[1] * square[4]
                    + 0.31 * square[2] * square[5];
                let high = self.highpass.step(metal, 0.14 + 0.38 * tone, 1.2, 0.0).high;
                let band = self.band.step(high, 0.34 + 0.43 * tone, 0.85, 0.0).band;
                self.body = approach(self.body, 0.0, 0.012 + 1.25 * decay, sample_rate);
                self.wire = 0.0;
                (0.31 * high + (0.55 + 0.37 * snappy) * band) * self.body
            }
        };
        self.age = self.age.saturating_add(1);
        fast_tanh(raw * (1.0 + 4.0 * drive)).clamp(-1.0, 1.0)
    }

    #[inline]
    fn advance(&mut self, oscillator: usize, hz: f32, sample_rate: f32) -> f32 {
        let phase = wrap_turns(self.phases[oscillator] + phase_step(hz, sample_rate));
        self.phases[oscillator] = phase;
        phase
    }
}

fn clap_burst(age: u32, spacing: u32) -> f32 {
    if age >= 4 * spacing {
        return 0.0;
    }
    let elapsed = (age % spacing) as f32 / spacing as f32;
    let attack = (elapsed * 24.0).min(1.0);
    let decay = (1.0 - elapsed).max(0.0);
    attack * decay * decay * (1.0 - 0.12 * (age / spacing) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{voice::pitch_hz, MachineKind};

    #[test]
    fn clap_has_four_separate_envelope_peaks() {
        let spacing = 480;
        for burst in 0..4 {
            let start = burst * spacing;
            assert_eq!(clap_burst(start, spacing), 0.0);
            assert!(clap_burst(start + 24, spacing) > 0.5);
            assert!(clap_burst(start + 460, spacing) < 0.003);
        }
        assert_eq!(clap_burst(4 * spacing, spacing), 0.0);
    }

    #[test]
    fn modes_are_distinct_and_metal_has_many_more_zero_crossings() {
        let waves = core::array::from_fn::<_, 4, _>(|mode| {
            let mut params = MachineKind::ElectroPerc.spec().default_params();
            params[1] = (mode as f32 + 0.5) / 4.0;
            params[3] = 0.1;
            let mut voice = Voice::default();
            let mut rng = 12_345;
            core::array::from_fn::<_, 2048, _>(|i| {
                voice.step(240.0, 48_000.0, (i == 0).then_some(0.8), &params, &mut rng)
            })
        });
        for (mode, wave) in waves.iter().enumerate() {
            assert!(wave.iter().any(|sample| sample.abs() > 0.15));
            for other in &waves[..mode] {
                assert!(
                    wave.iter()
                        .zip(other)
                        .map(|(a, b)| (a - b).abs())
                        .sum::<f32>()
                        > 10.0
                );
            }
        }
        let crossings = |wave: &[f32; 2048]| {
            wave.windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        assert!(crossings(&waves[3]) > 2 * crossings(&waves[0]));
    }

    #[test]
    fn four_retriggers_stay_bounded_at_every_midi_pitch() {
        for sample_rate in [8_000.0, 48_000.0, 96_000.0] {
            for mode in 0..4 {
                let mut params = [1.0; SOUND_PARAMS];
                params[1] = (mode as f32 + 0.5) / 4.0;
                for note in 0..=127 {
                    let hz = pitch_hz((note as f32 - 60.0) / 12.0);
                    let mut voice = Voice::default();
                    let mut rng = 42;
                    for i in 0..512 {
                        let sample = voice.step(
                            hz,
                            sample_rate,
                            (i % 128 == 0).then_some(1.0),
                            &params,
                            &mut rng,
                        );
                        assert!((-1.0..=1.0).contains(&sample));
                    }
                }
            }
        }
    }
}
