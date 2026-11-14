use crate::voice::{approach, choice, phase_step, pulse, saw, sine, triangle, StateVariable};
use crate::{fast_tanh, wrap_turns};
use leviathan_core::SOUND_PARAMS;

const COMB_SAMPLES: usize = 256;
const ALGORITHM_COUNT: usize = 4;
const EQUATION_COUNT: usize = 5;

#[derive(Clone, Copy)]
pub(super) enum Input {
    Oscillator,
    Gated(f32),
    Through(f32),
}

#[derive(Clone, Debug)]
pub(super) struct Voice {
    phase: f32,
    lfo_phase: f32,
    amp: f32,
    velocity: f32,
    held: f32,
    filter: StateVariable,
    comb: [f32; COMB_SAMPLES],
    write: usize,
    history: usize,
    algorithm: usize,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            phase: 0.0,
            lfo_phase: 0.0,
            amp: 0.0,
            velocity: 0.0,
            held: 0.0,
            filter: StateVariable::default(),
            comb: [0.0; COMB_SAMPLES],
            write: 0,
            history: 0,
            algorithm: 0,
        }
    }
}

impl Voice {
    #[inline]
    pub(super) fn reset(&mut self) {
        self.phase = 0.0;
        self.lfo_phase = 0.0;
        self.amp = 0.0;
        self.velocity = 0.0;
        self.held = 0.0;
        self.reset_shaper();
    }

    #[inline]
    fn reset_shaper(&mut self) {
        self.filter = StateVariable::default();
        // Delayed reads ignore the old contents until enough new samples have arrived.
        self.write = 0;
        self.history = 0;
    }

    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
    ) -> f32 {
        self.step_input(hz, sample_rate, trigger, params, Input::Oscillator)
    }

    #[inline]
    pub(super) fn step_input(
        &mut self,
        hz: f32,
        sample_rate: f32,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
        input: Input,
    ) -> f32 {
        let [algorithm, warp, equation, depth, lfo_rate, cutoff, resonance, decay] = *params;
        let algorithm = choice(algorithm, ALGORITHM_COUNT);
        if self.algorithm != algorithm {
            self.algorithm = algorithm;
            self.reset_shaper();
        }
        if let Some(velocity) = trigger {
            self.velocity = velocity;
            self.amp = velocity;
        }
        self.amp = approach(self.amp, 0.0, 0.015 + 1.8 * decay * decay, sample_rate);

        let lfo_hz = 0.07 + 25.0 * lfo_rate * lfo_rate;
        let next_lfo = self.lfo_phase + lfo_hz / sample_rate;
        self.lfo_phase = wrap_turns(next_lfo);
        let lfo = if lfo_rate == 0.0 {
            self.velocity
        } else {
            0.5 + 0.5 * sine(self.lfo_phase)
        };
        if trigger.is_some() || next_lfo >= 1.0 {
            self.held = (0.5 + 7.0 * lfo * self.amp) as u32 as f32 / 7.0;
        }
        let gain = match input {
            Input::Through(_) => 1.0,
            _ => self.amp,
        };
        let modulation = fun(choice(equation, EQUATION_COUNT), gain, lfo, self.held);
        let warp = (warp + depth * (modulation - 0.5)).clamp(0.0, 1.0);
        let step = phase_step(hz, sample_rate);
        self.phase = wrap_turns(self.phase + step);

        let source = |oscillator| match input {
            Input::Oscillator => oscillator,
            Input::Gated(sample) | Input::Through(sample) => sample.clamp(-1.0, 1.0),
        };
        let drive = 1.0 + 4.0 * warp;
        let shaped = match algorithm {
            0 => 2.0 * wrap_turns(0.5 + 0.5 * drive * source(sine(self.phase))) - 1.0,
            1 => fold(source(triangle(self.phase, step)) * drive),
            2 => {
                let steps = (32 - (29.0 * warp) as u32) as f32;
                let level = |sample| (0.5 + (0.5 + 0.5 * sample) * steps) as u32 as f32;
                let quantized = 2.0 * level(source(saw(self.phase, step))) / steps - 1.0;
                match input {
                    Input::Oscillator => quantized,
                    _ => quantized - (2.0 * level(0.0) / steps - 1.0),
                }
            }
            _ => {
                let osc = source(pulse(self.phase, step, 0.45)) * gain;
                let delay = 8 + (warp * (COMB_SAMPLES - 8) as f32) as usize;
                self.allpass_comb(osc, delay, 0.35 + 0.45 * warp)
            }
        };
        let cutoff = (cutoff + 0.25 * depth * (modulation - 0.5)).clamp(0.0, 1.0);
        let g = 0.01 + 0.84 * cutoff * cutoff;
        let damping = 1.60 - 1.42 * resonance;
        let input = if algorithm == 3 {
            shaped
        } else {
            shaped * gain
        };
        let filtered = self.filter.step(input, g, damping, 0.0);
        let output = match algorithm {
            1 => filtered.band * 1.5,
            2 => filtered.high,
            _ => filtered.low,
        };
        fast_tanh(output).clamp(-1.0, 1.0)
    }

    #[inline]
    fn allpass_comb(&mut self, input: f32, delay: usize, feedback: f32) -> f32 {
        let delayed = if self.history >= delay {
            self.comb[(self.write + COMB_SAMPLES - delay) % COMB_SAMPLES]
        } else {
            0.0
        };
        let state = input + feedback * delayed;
        self.comb[self.write] = if state.abs() < 1e-12 { 0.0 } else { state };
        self.write = (self.write + 1) % COMB_SAMPLES;
        self.history = (self.history + 1).min(COMB_SAMPLES);
        delayed - feedback * state
    }
}

#[inline]
fn fold(input: f32) -> f32 {
    1.0 - (4.0 * wrap_turns((input + 3.0) * 0.25) - 2.0).abs()
}

#[inline]
fn fun(equation: usize, a: f32, b: f32, held: f32) -> f32 {
    match equation {
        0 => (a + b) * 0.5,
        1 => a * b,
        2 => sine(0.5 * a) * b,
        3 => {
            let diode = (a - 0.25).max(0.0) * (4.0 / 3.0);
            4.0 * diode * (1.0 - diode) * b
        }
        _ => held,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MachineKind;

    #[test]
    fn sample_input_triggers_an_envelope_and_lane_input_runs_without_a_gate() {
        for algorithm in 0..ALGORITHM_COUNT {
            let mut params = MachineKind::VastFunSynth.spec().default_params();
            params[0] = (algorithm as f32 + 0.5) / ALGORITHM_COUNT as f32;
            let mut voice = Voice::default();
            for _ in 0..256 {
                assert_eq!(
                    voice.step_input(261.625_58, 48_000.0, None, &params, Input::Gated(0.25)),
                    0.0
                );
                assert_eq!(
                    voice.step_input(261.625_58, 48_000.0, None, &params, Input::Through(0.0)),
                    0.0
                );
            }
            let signal = |gated| {
                let mut voice = Voice::default();
                (0..256)
                    .map(|i| {
                        let sample = if i % 19 < 8 { 0.20 } else { -0.33 };
                        let input = if gated {
                            Input::Gated(sample)
                        } else {
                            Input::Through(sample)
                        };
                        let trigger = (gated && i == 0).then_some(0.8);
                        voice
                            .step_input(261.625_58, 48_000.0, trigger, &params, input)
                            .abs()
                    })
                    .sum::<f32>()
            };
            assert!(signal(true) > 0.1);
            assert!(signal(false) > 0.1);
        }
    }

    #[test]
    fn algorithms_and_equations_change_the_rendered_signal() {
        const FRAMES: usize = 2048;
        let render = |algorithm, equation| {
            let mut voice = Voice::default();
            let mut params = MachineKind::VastFunSynth.spec().default_params();
            params[0] = (algorithm as f32 + 0.5) / ALGORITHM_COUNT as f32;
            params[2] = (equation as f32 + 0.5) / EQUATION_COUNT as f32;
            params[3] = 0.8;
            params[4] = 0.8;
            let samples = core::array::from_fn::<_, FRAMES, _>(|i| {
                voice.step(261.625_58, 48_000.0, (i == 0).then_some(0.8), &params)
            });
            assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
            assert!(samples.iter().any(|sample| sample.abs() > 0.05));
            samples
        };
        let difference = |a: &[f32; FRAMES], b: &[f32; FRAMES]| {
            a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>()
        };
        let algorithms = core::array::from_fn::<_, ALGORITHM_COUNT, _>(|i| render(i, 0));
        for (i, a) in algorithms.iter().enumerate() {
            for b in &algorithms[..i] {
                assert!(difference(a, b) > 1.0);
            }
        }
        let equations = core::array::from_fn::<_, EQUATION_COUNT, _>(|i| render(1, i));
        for (i, a) in equations.iter().enumerate() {
            for b in &equations[..i] {
                assert!(difference(a, b) > 1.0);
            }
        }
    }

    #[test]
    fn maximum_drive_and_resonance_stay_finite_and_comb_reset_is_silent() {
        let mut voice = Voice::default();
        for algorithm in [0.3, 1.0] {
            let params = [algorithm, 1.0, 0.8, 1.0, 1.0, 1.0, 1.0, 1.0];
            for i in 0..48_000 {
                let hz = if i % 2400 < 1200 { 32.7 } else { 8372.0 };
                let sample = voice.step(hz, 48_000.0, (i % 512 == 0).then_some(1.0), &params);
                assert!(
                    (-1.0..=1.0).contains(&sample),
                    "algorithm {algorithm}, frame {i}: {sample}"
                );
            }
            assert!(voice.comb.iter().all(|sample| sample.abs() <= 5.001));
            voice.reset();
            for _ in 0..256 {
                assert_eq!(voice.step(220.0, 48_000.0, None, &params), 0.0);
            }
        }
    }
}
