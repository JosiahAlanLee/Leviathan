#![cfg_attr(not(feature = "std"), no_std)]

use core::f32::consts::TAU;
use leviathan_core::{CvFrame, SoundLock, NUM_LANES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineKind {
    KickDrum = 0,
    FmGlass = 1,
    SubBass = 2,
    NoiseWash = 3,
    ChordStab = 4,
    AcidLead = 5,
}

impl MachineKind {
    pub const fn from_lane_and_lock(lane: usize, lock: &SoundLock) -> Self {
        let id = if lock.machine >= 0 { lock.machine as usize } else { lane };
        match id % 6 {
            0 => Self::KickDrum,
            1 => Self::FmGlass,
            2 => Self::SubBass,
            3 => Self::NoiseWash,
            4 => Self::ChordStab,
            _ => Self::AcidLead,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CounterMode {
    Loop,
    OneShot,
    Pendulum,
}

#[derive(Clone, Debug)]
pub struct ByteBusCounter {
    pub mode: CounterMode,
    pub modulus: u8,
    pub value: u8,
    ascending: bool,
    prev_clock: bool,
}

impl Default for ByteBusCounter {
    fn default() -> Self {
        Self {
            mode: CounterMode::Loop,
            modulus: 16,
            value: 0,
            ascending: true,
            prev_clock: false,
        }
    }
}

impl ByteBusCounter {
    pub fn step(&mut self, clock_high: bool, reset_high: bool) -> u8 {
        let m = self.modulus.max(2);
        if reset_high {
            self.value = 0;
            self.ascending = true;
        } else if clock_high && !self.prev_clock {
            match self.mode {
                CounterMode::Loop => {
                    self.value = (self.value + 1) % m;
                }
                CounterMode::OneShot => {
                    if self.value + 1 < m {
                        self.value += 1;
                    }
                }
                CounterMode::Pendulum => {
                    if self.ascending {
                        if self.value + 1 < m {
                            self.value += 1;
                        } else {
                            self.ascending = false;
                            self.value = self.value.saturating_sub(1);
                        }
                    } else if self.value > 0 {
                        self.value -= 1;
                    } else {
                        self.ascending = true;
                        self.value = 1;
                    }
                }
            }
        }
        self.prev_clock = clock_high;
        self.value
    }
}

#[inline]
fn fast_sin(phase_01: f32) -> f32 {
    let x = (phase_01 - (phase_01 as i32 as f32)) * TAU - core::f32::consts::PI;
    let y = x * (core::f32::consts::FRAC_2_PI) * (1.0 - x.abs() * core::f32::consts::FRAC_1_PI);
    0.225 * (y * y.abs() - y) + y
}

#[inline]
fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

#[inline]
fn v_oct_to_hz(v_oct: f32) -> f32 {
    // Padé/octave power-of-2 approximation centered at C4 = 261.62558 Hz (0V)
    let clamped = v_oct.clamp(-5.0, 5.0);
    let whole = if clamped >= 0.0 {
        clamped as i32
    } else {
        (clamped - 0.999_999) as i32
    };
    let frac = clamped - whole as f32;
    let poly = 1.0 + frac * (0.693_147_2 + frac * (0.240_226_5 + frac * 0.055_504_1));
    let scale = if whole >= 0 {
        (1u32 << (whole as u32)) as f32
    } else {
        1.0 / ((1u32 << ((-whole) as u32)) as f32)
    };
    261.625_58 * scale * poly
}

#[derive(Clone, Debug)]
struct VoiceState {
    phase: f32,
    mod_phase: f32,
    env: f32,
    pitch_env: f32,
    prev_gate: bool,
    ic1eq: f32,
    ic2eq: f32,
    rng: u32,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            mod_phase: 0.0,
            env: 0.0,
            pitch_env: 0.0,
            prev_gate: false,
            ic1eq: 0.0,
            ic2eq: 0.0,
            rng: 0x9e37_79b9,
        }
    }
}

pub struct SynthRack {
    sample_rate: f32,
    voices: [VoiceState; NUM_LANES],
    pub counter: ByteBusCounter,
}

impl SynthRack {
    pub fn new(sample_rate: f32) -> Self {
        let mut voices = core::array::from_fn(|_| VoiceState::default());
        for (i, v) in voices.iter_mut().enumerate() {
            v.rng ^= (i as u32 + 1).wrapping_mul(0x85eb_ca6b);
        }
        Self {
            sample_rate: sample_rate.max(8_000.0),
            voices,
            counter: ByteBusCounter::default(),
        }
    }

    pub fn render_stereo(&mut self, cv: &CvFrame) -> (f32, f32) {
        let _bus_step = self.counter.step(cv.bytebus_v[8] > 5.0, cv.reset_v > 5.0);
        let dt = 1.0 / self.sample_rate;
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;

        for lane in 0..NUM_LANES {
            let v = &mut self.voices[lane];
            let gate_high = cv.gate_v[lane] > 2.5;
            let lock = &cv.locks[lane];
            let machine = MachineKind::from_lane_and_lock(lane, lock);

            if gate_high && !v.prev_gate {
                v.env = (cv.velocity_v[lane] / 10.0).clamp(0.1, 1.0);
                v.pitch_env = 1.0;
            }
            v.prev_gate = gate_high;

            let decay_Lock = if (lock.mask & 1) != 0 {
                lock.values[0].clamp(0.05, 0.98)
            } else {
                0.35
            };
            let decay_rate = 1.0 - (dt / (0.04 + 0.65 * decay_Lock));
            v.env *= decay_rate;
            v.pitch_env *= 1.0 - (dt / 0.022);

            let base_hz = v_oct_to_hz(cv.pitch_v[lane]);
            let freq = match machine {
                MachineKind::KickDrum => 48.0 + 170.0 * v.pitch_env,
                MachineKind::SubBass => base_hz * 0.5,
                _ => base_hz,
            };

            v.phase += freq * dt;
            if v.phase >= 1.0 {
                v.phase -= 1.0;
            }
            v.mod_phase += freq * 2.732 * dt;
            if v.mod_phase >= 1.0 {
                v.mod_phase -= 1.0;
            }

            v.rng ^= v.rng << 13;
            v.rng ^= v.rng >> 17;
            v.rng ^= v.rng << 5;
            let noise = (v.rng as i32 as f32) * (1.0 / 2_147_483_648.0);

            let raw = match machine {
                MachineKind::KickDrum => fast_tanh(2.2 * fast_sin(v.phase) * v.env),
                MachineKind::FmGlass => {
                    let idx = if (lock.mask & 2) != 0 { lock.values[1] * 1.5 } else { 0.45 };
                    fast_sin(v.phase + idx * v.env * fast_sin(v.mod_phase)) * v.env
                }
                MachineKind::SubBass => {
                    let saw = 2.0 * v.phase - 1.0;
                    fast_tanh(1.6 * (0.65 * fast_sin(v.phase) + 0.35 * saw)) * v.env
                }
                MachineKind::NoiseWash => noise * v.env * 0.55,
                MachineKind::ChordStab => {
                    let fifth = fast_sin((v.phase * 1.498_307) % 1.0);
                    0.5 * (fast_sin(v.phase) + fifth) * v.env
                }
                MachineKind::AcidLead => (2.0 * v.phase - 1.0) * v.env,
            };

            // Lightweight 2-pole SVF low-pass modulated by envelope & lock slot 2
            let cutoff_norm = if (lock.mask & 4) != 0 {
                lock.values[2].clamp(0.05, 0.85)
            } else {
                (0.18 + 0.45 * v.env).clamp(0.05, 0.85)
            };
            let g = (cutoff_norm * 0.65).clamp(0.01, 0.75);
            let k = 0.85_f32;
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            let v3 = raw - v.ic2eq;
            let v1 = a1 * v.ic1eq + a2 * v3;
            let v2 = v.ic2eq + a2 * v.ic1eq + a3 * v3;
            v.ic1eq = 2.0 * v1 - v.ic1eq;
            v.ic2eq = 2.0 * v2 - v.ic2eq;

            let pan = 0.2 + 0.6 * (lane as f32 / (NUM_LANES.saturating_sub(1)) as f32);
            left += v2 * (1.0 - pan) * 0.45;
            right += v2 * pan * 0.45;
        }

        (fast_tanh(left), fast_tanh(right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leviathan_core::{EngineConfig, NoteCell, Pattern, TrackerEvaluator};

    #[test]
    fn synth_rack_renders_finite_stereo_signal() {
        let mut pat = Pattern::default();
        for lane in 0..NUM_LANES {
            pat.lanes[lane][0] = NoteCell {
                active: true,
                midi: 60 + lane as u8 * 3,
                velocity: 110,
                ..NoteCell::default()
            };
        }
        let mut eval = TrackerEvaluator::new(EngineConfig::default());
        eval.playing = true;
        let mut rack = SynthRack::new(48_000.0);

        let mut energy = 0.0_f32;
        for _ in 0..256 {
            let cv = eval.step_sample(&pat);
            let (l, r) = rack.render_stereo(&cv);
            assert!(l.is_finite() && r.is_finite());
            energy += l.abs() + r.abs();
        }
        assert!(energy > 0.1);
    }

    #[test]
    fn pendulum_counter_bounces_cleanly() {
        let mut ctr = ByteBusCounter {
            mode: CounterMode::Pendulum,
            modulus: 3,
            ..ByteBusCounter::default()
        };
        let mut seq = [0u8; 5];
        for slot in &mut seq {
            ctr.step(false, false);
            *slot = ctr.step(true, false);
        }
        assert_eq!(seq, [1, 2, 1, 0, 1]);
    }
}
