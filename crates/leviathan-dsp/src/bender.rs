use crate::voice::{cosine, sine};
use crate::wrap_turns;

pub const BENDER_MAX_SHIFT_HZ: f32 = 200.0;
const DELAY_SECONDS: [f32; 4] = [0.019, 0.027, 0.036, 0.045];
const DIFFUSER_SAMPLES: usize = 192;
const MAX_TANK_RATE: f32 = 96_000.0;
const TANK_SAMPLES: usize = {
    let mut total = 0;
    let mut line = 0;
    while line < DELAY_SECONDS.len() {
        total += delay_frames(MAX_TANK_RATE, DELAY_SECONDS[line]);
        line += 1;
    }
    total
};

const fn delay_frames(rate: f32, seconds: f32) -> usize {
    ((rate * seconds + 0.5) as usize) | 1
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BenderParams {
    pub shift_norm: f32,
    pub feedback: f32,
    pub decay: f32,
    pub freeze: bool,
}

impl Default for BenderParams {
    fn default() -> Self {
        Self {
            shift_norm: 0.5,
            feedback: 0.32,
            decay: 0.40,
            freeze: false,
        }
    }
}

impl BenderParams {
    pub fn is_valid(self) -> bool {
        [self.shift_norm, self.feedback, self.decay]
            .iter()
            .all(|value| (0.0..=1.0).contains(value))
    }

    pub fn shift_hz(self) -> f32 {
        (2.0 * self.shift_norm - 1.0) * BENDER_MAX_SHIFT_HZ
    }
}

#[derive(Clone, Copy, Default)]
struct Quadrature {
    state: [[[f32; 2]; 3]; 2],
    parity: usize,
    delayed_q: f32,
}

impl Quadrature {
    #[inline]
    fn step(&mut self, input: f32) -> [f32; 2] {
        // Six-pole paths with three z^-2 allpasses apiece, plus a one-sample Q
        // delay. Coefficients were fitted to a +90 degree I/Q phase difference
        // over 0.0014..0.4986 cycles/sample using the allpass transfer function
        // H(z) = (z^-2 - a) / (1 - a*z^-2).
        const COEFFICIENTS: [[f32; 3]; 2] = [
            [0.189_643_43, 0.780_599_73, 0.963_998_9],
            [0.533_753_48, 0.906_971_55, 0.990_716_76],
        ];
        let mut out = [input; 2];
        for (branch, coefficients) in COEFFICIENTS.into_iter().enumerate() {
            for (stage, a) in coefficients.into_iter().enumerate() {
                let history = &mut self.state[branch][stage][self.parity];
                let y = *history - a * out[branch];
                *history = flush(out[branch] + a * y);
                out[branch] = y;
            }
        }
        self.parity ^= 1;
        [out[0], core::mem::replace(&mut self.delayed_q, out[1])]
    }
}

#[derive(Default)]
struct FrequencyShifter {
    phase: f32,
    channels: [Quadrature; 2],
}

impl FrequencyShifter {
    #[inline]
    fn step(&mut self, input: [f32; 2], shift_hz: f32, rate: f32) -> [f32; 2] {
        self.phase = wrap_turns(self.phase + shift_hz / rate);
        let cosine = cosine(self.phase);
        let sine = sine(self.phase);
        core::array::from_fn(|channel| {
            let [i, q] = self.channels[channel].step(input[channel]);
            i * cosine - q * sine
        })
    }
}

struct Diffuser {
    buffer: [f32; DIFFUSER_SAMPLES],
    length: usize,
    position: usize,
}

impl Diffuser {
    fn new(rate: f32, seconds: f32) -> Self {
        Self {
            buffer: [0.0; DIFFUSER_SAMPLES],
            length: ((rate * seconds + 0.5) as usize).clamp(1, DIFFUSER_SAMPLES),
            position: 0,
        }
    }

    fn step(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.position];
        let output = delayed - 0.55 * input;
        self.buffer[self.position] = flush(input + 0.55 * output);
        self.position += 1;
        if self.position == self.length {
            self.position = 0;
        }
        output
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.position = 0;
    }
}

struct Tank {
    memory: [f32; TANK_SAMPLES],
    offsets: [usize; 4],
    lengths: [usize; 4],
    cursors: [usize; 4],
    lows: [f32; 4],
}

// The shared tank plus its input diffusion delays stay within 64 KiB.
const _: () =
    assert!((TANK_SAMPLES + 2 * DIFFUSER_SAMPLES) * core::mem::size_of::<f32>() <= 64 * 1024);

impl Tank {
    fn new(rate: f32) -> Self {
        // Keep the geometry proportional through 96 kHz, then scale it down
        // to fit the fixed pool on devices with higher sample rates.
        let rate = rate.min(MAX_TANK_RATE);
        let lengths = DELAY_SECONDS.map(|seconds| delay_frames(rate, seconds));
        let offsets = [
            0,
            lengths[0],
            lengths[0] + lengths[1],
            lengths[0] + lengths[1] + lengths[2],
        ];
        Self {
            memory: [0.0; TANK_SAMPLES],
            offsets,
            lengths,
            cursors: [0; 4],
            lows: [0.0; 4],
        }
    }

    fn clear(&mut self) {
        self.memory.fill(0.0);
        self.cursors = [0; 4];
        self.lows = [0.0; 4];
    }

    #[inline]
    fn step(&mut self, input: [f32; 2], feedback: f32, damping: f32, frozen: f32) -> [f32; 2] {
        let read: [f32; 4] =
            core::array::from_fn(|i| self.memory[self.offsets[i] + self.cursors[i]]);
        let loss = 1.0 - frozen;
        let filtered = core::array::from_fn(|i| {
            self.lows[i] = flush(self.lows[i] + damping * (read[i] - self.lows[i]));
            read[i] + loss * (self.lows[i] - read[i])
        });
        let gain = feedback + frozen * (1.0 - feedback);
        let feedback = hadamard(filtered);
        let [l, r] = input.map(|v| 0.5 * loss * v);
        // Use two orthogonal Hadamard columns for the stereo input and return.
        let injection = [l + r, l - r, l + r, l - r];
        for i in 0..4 {
            self.memory[self.offsets[i] + self.cursors[i]] =
                flush(gain * feedback[i] + injection[i]);
            self.cursors[i] += 1;
            if self.cursors[i] == self.lengths[i] {
                self.cursors[i] = 0;
            }
        }
        let even = read[0] + read[2];
        let odd = read[1] + read[3];
        [0.5 * (even + odd), 0.5 * (even - odd)]
    }
}

#[inline]
fn hadamard([a, b, c, d]: [f32; 4]) -> [f32; 4] {
    let sum = a + b;
    let difference = a - b;
    let sum2 = c + d;
    let difference2 = c - d;
    [
        0.5 * (sum + sum2),
        0.5 * (difference + difference2),
        0.5 * (sum - sum2),
        0.5 * (difference - difference2),
    ]
}

#[inline]
fn flush(sample: f32) -> f32 {
    if sample.abs() < 1e-20 {
        0.0
    } else {
        sample
    }
}

pub struct SpoonBenderBus {
    rate: f32,
    shifter: FrequencyShifter,
    diffusers: [Diffuser; 2],
    tank: Tank,
    shift: f32,
    feedback: f32,
    damping: f32,
    freeze: f32,
    shift_target: f32,
    feedback_target: f32,
    damping_target: f32,
    freeze_target: bool,
    control_step: f32,
    freeze_step: f32,
    quiet_frames: u32,
}

impl SpoonBenderBus {
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() {
            sample_rate.max(8_000.0)
        } else {
            48_000.0
        };
        let target = BenderParams::default();
        let feedback = loop_gain(target.feedback, target.decay);
        let damping = loop_damping(target.decay, rate);
        Self {
            rate,
            shifter: FrequencyShifter::default(),
            diffusers: [Diffuser::new(rate, 0.0013), Diffuser::new(rate, 0.0017)],
            tank: Tank::new(rate),
            shift: target.shift_hz(),
            feedback,
            damping,
            freeze: 0.0,
            shift_target: target.shift_hz(),
            feedback_target: feedback,
            damping_target: damping,
            freeze_target: target.freeze,
            control_step: 1.0 / (1.0 + 0.012 * rate),
            freeze_step: 1.0 / (0.025 * rate),
            quiet_frames: u32::MAX,
        }
    }

    pub fn set_params(&mut self, params: BenderParams) {
        if params.is_valid() {
            self.shift_target = params.shift_hz();
            self.feedback_target = loop_gain(params.feedback, params.decay);
            self.damping_target = loop_damping(params.decay, self.rate);
            self.freeze_target = params.freeze;
            if self.quiet_frames == u32::MAX {
                self.snap_controls();
            }
        }
    }

    pub fn reset(&mut self) {
        self.shifter = FrequencyShifter::default();
        for diffuser in &mut self.diffusers {
            diffuser.clear();
        }
        self.tank.clear();
        self.quiet_frames = u32::MAX;
        self.snap_controls();
    }

    fn snap_controls(&mut self) {
        self.shift = self.shift_target;
        self.feedback = self.feedback_target;
        self.damping = self.damping_target;
        // An empty tank lets the first send start the freeze ramp.
        self.freeze = 0.0;
    }

    #[inline]
    pub(crate) fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
        let excitation = input.map(|v| if v.is_finite() { v } else { 0.0 });
        if self.quiet_frames == u32::MAX && excitation == [0.0; 2] {
            return [0.0; 2];
        }
        self.shift = crate::slew(self.shift, self.shift_target, self.control_step);
        self.feedback = crate::slew(self.feedback, self.feedback_target, self.control_step);
        self.damping = crate::slew(self.damping, self.damping_target, self.control_step);
        // Reach exactly unit gain and zero injection after a 25 ms fade.
        self.freeze = if self.freeze_target {
            (self.freeze + self.freeze_step).min(1.0)
        } else {
            (self.freeze - self.freeze_step).max(0.0)
        };
        let shifted = self.shifter.step(excitation, self.shift, self.rate);
        let diffused = core::array::from_fn(|i| self.diffusers[i].step(shifted[i]));
        let output = self
            .tank
            .step(diffused, self.feedback, self.damping, self.freeze);
        if excitation == [0.0; 2] && output.iter().all(|s| s.abs() < 1e-10) && self.freeze == 0.0 {
            // A sparse delay network can be silent between echoes.
            self.quiet_frames = self.quiet_frames.saturating_add(1);
            if self.quiet_frames > (self.rate * 0.25) as u32 {
                self.reset();
            }
        } else {
            self.quiet_frames = 0;
        }
        output.map(|sample| (0.31 * sample).clamp(-1.0, 1.0))
    }
}

#[inline]
fn loop_gain(feedback: f32, decay: f32) -> f32 {
    (0.10 + 0.67 * feedback + 0.25 * decay).min(0.975)
}

#[inline]
fn loop_damping(decay: f32, rate: f32) -> f32 {
    let scaled = core::f32::consts::TAU * (1900.0 + 11_000.0 * decay * decay) / rate;
    scaled / (1.0 + scaled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_dome_shifts_both_directions_with_image_rejection() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for shift in [-200.0, -80.0, 80.0, 200.0] {
                let mut shifter = FrequencyShifter::default();
                let mut bins = [[[0.0; 2]; 2]; 2];
                for i in 0..rate as u32 {
                    let time = i as f32 / rate;
                    let phase = |hz| wrap_turns((time * hz) % 1.0);
                    let y = shifter.step(
                        [0.5 * sine(phase(440.0)), 0.25 * cosine(phase(440.0))],
                        shift,
                        rate,
                    );
                    if time >= 0.5 {
                        for (bin, hz) in bins.iter_mut().zip([440.0 + shift, 440.0 - shift]) {
                            for (pair, sample) in bin.iter_mut().zip(y) {
                                pair[0] += sample * cosine(phase(hz));
                                pair[1] += sample * sine(phase(hz));
                            }
                        }
                    }
                }
                let power = |v: [f32; 2]| v[0] * v[0] + v[1] * v[1];
                for channel in 0..2 {
                    assert!(power(bins[0][channel]) > 1000.0);
                    assert!(
                        power(bins[0][channel]) > 1000.0 * power(bins[1][channel]),
                        "{rate} {shift} channel {channel}"
                    );
                }
            }
        }
    }

    #[test]
    fn freeze_keeps_energy_for_six_seconds_and_rejects_new_input() {
        let rate = 48_000.0;
        let mut bus = SpoonBenderBus::new(rate);
        bus.set_params(BenderParams {
            feedback: 1.0,
            decay: 1.0,
            ..BenderParams::default()
        });
        for i in 0..16_000 {
            let x = 0.22 * sine(i as f32 * 440.0 / rate % 1.0);
            bus.process([x, -0.4 * x]);
        }
        bus.set_params(BenderParams {
            feedback: 1.0,
            decay: 1.0,
            freeze: true,
            ..BenderParams::default()
        });
        for _ in 0..25_000 {
            bus.process([0.0; 2]);
        }
        assert_eq!(bus.freeze, 1.0);
        let energy = |bus: &SpoonBenderBus| {
            bus.tank
                .memory
                .iter()
                .map(|x| f64::from(*x) * f64::from(*x))
                .sum::<f64>()
        };
        let initial = energy(&bus);
        assert!(initial > 0.01);
        for _ in 0..288_000 {
            let wet = bus.process([0.8, 0.5]);
            assert!(wet.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        }
        let retained = energy(&bus);
        assert!(
            (0.998..1.002).contains(&(retained / initial)),
            "{initial} -> {retained}"
        );
        bus.set_params(BenderParams::default());
        for _ in 0..96_000 {
            bus.process([0.0; 2]);
        }
        assert!(energy(&bus) < initial * 0.001);
    }

    #[test]
    fn pre_armed_freeze_waits_for_input_and_sustains_the_capture() {
        let rate = 48_000.0;
        let mut bus = SpoonBenderBus::new(rate);
        bus.set_params(BenderParams {
            feedback: 0.85,
            decay: 0.9,
            freeze: true,
            ..BenderParams::default()
        });
        for _ in 0..2_400 {
            assert_eq!(bus.process([0.0; 2]), [0.0; 2]);
        }
        assert_eq!(bus.freeze, 0.0);

        for i in 0..2_400 {
            let env = 1.0 - i as f32 / 2_400.0;
            let hit = 0.35 * env * sine(i as f32 * 185.0 / rate);
            bus.process([hit; 2]);
        }
        assert_eq!(bus.freeze, 1.0);
        let energy = |bus: &SpoonBenderBus| {
            bus.tank
                .memory
                .iter()
                .map(|&sample| f64::from(sample).powi(2))
                .sum::<f64>()
        };
        let captured = energy(&bus);
        assert!(captured > 0.002, "{captured}");
        let mut peak = 0.0_f32;
        for _ in 0..96_000 {
            let [left, right] = bus.process([0.0; 2]);
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.005, "{peak}");
        assert!((0.998..1.002).contains(&(energy(&bus) / captured)));
    }
}
