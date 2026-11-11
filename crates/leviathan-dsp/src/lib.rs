#![cfg_attr(not(feature = "std"), no_std)]

pub mod fm;
pub mod sampler;

use core::f32::consts::{FRAC_1_PI, FRAC_2_PI, PI, TAU};
use fm::FourOpFmVoice;
use leviathan_core::{CvFrame, SoundLock, NUM_LANES};
use sampler::{SampleBank, VarClockSamplerVoice};

pub const MACHINE_COUNT: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineKind {
    KickDrum = 0,
    FourOpFm = 1,
    SubBass = 2,
    NoiseWash = 3,
    VarSampler = 4,
    AcidLead = 5,
    ChordStab = 6,
}

impl MachineKind {
    pub const fn from_lane_and_lock(lane: usize, lock: &SoundLock) -> Self {
        let id = if lock.machine >= 0 {
            lock.machine as usize
        } else {
            lane
        };
        match id % MACHINE_COUNT {
            0 => Self::KickDrum,
            1 => Self::FourOpFm,
            2 => Self::SubBass,
            3 => Self::NoiseWash,
            4 => Self::VarSampler,
            5 => Self::AcidLead,
            _ => Self::ChordStab,
        }
    }

    pub const fn short_name(self) -> &'static str {
        match self {
            Self::KickDrum => "KICK",
            Self::FourOpFm => "FM4",
            Self::SubBass => "SUB",
            Self::NoiseWash => "AIR",
            Self::VarSampler => "SMPL",
            Self::AcidLead => "ACID",
            Self::ChordStab => "CHRD",
        }
    }
}

#[inline]
pub fn lock_param(lock: &SoundLock, slot: usize, default: f32) -> f32 {
    if slot < 8 && ((lock.mask >> slot) & 1) != 0 {
        lock.values[slot]
    } else {
        default
    }
}

#[inline]
pub fn wrap_turns(turns: f32) -> f32 {
    let clamped = turns.clamp(-64.0, 64.0);
    let i = clamped as i32;
    let mut frac = clamped - (i as f32);
    if frac < 0.0 {
        frac += 1.0;
    }
    frac
}

#[inline]
pub fn fast_sin_turns(turns: f32) -> f32 {
    let x = wrap_turns(turns) * TAU - PI;
    let y = x * FRAC_2_PI * (1.0 - x.abs() * FRAC_1_PI);
    -(0.225 * (y * y.abs() - y) + y)
}

#[inline]
fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

#[inline]
pub fn v_oct_to_hz(v_oct: f32) -> f32 {
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

#[derive(Clone, Debug)]
struct VoiceState {
    phase: f32,
    env: f32,
    pitch_env: f32,
    prev_gate: bool,
    ic1eq: f32,
    ic2eq: f32,
    rng: u32,
    fm: FourOpFmVoice,
    sampler: VarClockSamplerVoice,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            phase: 0.0,
            env: 0.0,
            pitch_env: 0.0,
            prev_gate: false,
            ic1eq: 0.0,
            ic2eq: 0.0,
            rng: 0x9e37_79b9,
            fm: FourOpFmVoice::default(),
            sampler: VarClockSamplerVoice::default(),
        }
    }
}

pub struct SynthRack {
    sample_rate: f32,
    voices: [VoiceState; NUM_LANES],
    pub sample_bank: SampleBank,
    pub counter: ByteBusCounter,
}

impl SynthRack {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(8_000.0),
            voices: Self::fresh_voices(),
            sample_bank: SampleBank::new(),
            counter: ByteBusCounter::default(),
        }
    }

    fn fresh_voices() -> [VoiceState; NUM_LANES] {
        let mut voices = core::array::from_fn(|_| VoiceState::default());
        for (i, v) in voices.iter_mut().enumerate() {
            v.rng ^= (i as u32 + 1).wrapping_mul(0x85eb_ca6b);
        }
        voices
    }

    pub fn reset_voices(&mut self) {
        self.voices = Self::fresh_voices();
        self.counter = ByteBusCounter::default();
    }

    pub fn retrigger_gates(&mut self) {
        for voice in &mut self.voices {
            voice.prev_gate = false;
        }
    }

    pub fn render_stereo(&mut self, cv: &CvFrame) -> (f32, f32) {
        let _bus_step = self.counter.step(cv.bytebus_v[8] > 5.0, cv.reset_v > 5.0);
        let dt = 1.0 / self.sample_rate;
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;

        for (lane, v) in self.voices.iter_mut().enumerate() {
            let gate_high = cv.gate_v[lane] > 2.5;
            let lock = &cv.locks[lane];
            let machine = MachineKind::from_lane_and_lock(lane, lock);

            if gate_high && !v.prev_gate {
                v.env = (cv.velocity_v[lane] / 10.0).clamp(0.1, 1.0);
                v.pitch_env = 1.0;
                v.fm.trigger();
                v.sampler.trigger(&self.sample_bank, lock);
            }
            v.prev_gate = gate_high;

            let decay_lock = lock_param(lock, 0, 0.35).clamp(0.03, 0.98);
            let decay_rate = 1.0 - (dt / (0.04 + 0.85 * decay_lock));
            v.env *= decay_rate;
            v.pitch_env *= 1.0 - (dt / 0.022);

            let base_hz = v_oct_to_hz(cv.pitch_v[lane]);
            let freq = match machine {
                MachineKind::KickDrum => 48.0 + 175.0 * v.pitch_env,
                MachineKind::SubBass => base_hz * 0.5,
                _ => base_hz,
            };

            v.phase = wrap_turns(v.phase + freq * dt);

            v.rng ^= v.rng << 13;
            v.rng ^= v.rng >> 17;
            v.rng ^= v.rng << 5;
            let noise = (v.rng as i32 as f32) * (1.0 / 2_147_483_648.0);

            let raw = match machine {
                MachineKind::KickDrum => {
                    let drive = 1.8 + 2.2 * lock_param(lock, 1, 0.35).clamp(0.0, 1.0);
                    fast_tanh(drive * fast_sin_turns(v.phase) * v.env)
                }
                MachineKind::FourOpFm => v.fm.step_sample(base_hz, self.sample_rate, v.env, lock),
                MachineKind::SubBass => {
                    let saw = 2.0 * v.phase - 1.0;
                    fast_tanh(1.6 * (0.65 * fast_sin_turns(v.phase) + 0.35 * saw)) * v.env
                }
                MachineKind::NoiseWash => noise * v.env * 0.55,
                MachineKind::VarSampler => {
                    v.sampler.step_sample(&self.sample_bank, base_hz, self.sample_rate, lock) * v.env
                }
                MachineKind::AcidLead => (2.0 * v.phase - 1.0) * v.env,
                MachineKind::ChordStab => {
                    let fifth = fast_sin_turns(v.phase * 1.498_307);
                    0.5 * (fast_sin_turns(v.phase) + fifth) * v.env
                }
            };

            let cutoff_norm = lock_param(lock, 2, (0.22 + 0.52 * v.env).clamp(0.05, 0.88))
                .clamp(0.04, 0.88);
            let g = (cutoff_norm * 0.72).clamp(0.01, 0.78);
            let k = if matches!(machine, MachineKind::AcidLead) {
                0.55_f32
            } else {
                0.85_f32
            };
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
    fn synth_rack_renders_fm_and_sampler_sound_locks() {
        let mut pat = Pattern::default();
        pat.lanes[1][0] = NoteCell {
            active: true,
            midi: 64,
            velocity: 118,
            lock: SoundLock {
                machine: MachineKind::FourOpFm as i16,
                mask: 0b0111_1011,
                values: [0.55, 0.80, 0.70, 0.0, 0.55, 0.30, 0.50, 0.15],
            },
            ..NoteCell::default()
        };
        pat.lanes[4][0] = NoteCell {
            active: true,
            midi: 57,
            velocity: 120,
            lock: SoundLock {
                machine: MachineKind::VarSampler as i16,
                mask: 0b1111_1011,
                values: [0.70, 0.18, 0.65, 0.60, 0.10, 0.45, 0.35, 0.80],
            },
            ..NoteCell::default()
        };

        let mut eval = TrackerEvaluator::new(EngineConfig::default());
        eval.playing = true;
        let mut rack = SynthRack::new(48_000.0);

        let mut energy = 0.0_f32;
        for _ in 0..512 {
            let cv = eval.step_sample(&pat);
            let (l, r) = rack.render_stereo(&cv);
            assert!(l.is_finite() && r.is_finite());
            energy += l.abs() + r.abs();
        }
        assert!(energy > 0.25);
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
