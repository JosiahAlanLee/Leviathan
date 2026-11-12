#![cfg_attr(not(feature = "std"), no_std)]

pub mod fm;
mod master;
pub mod sampler;
pub mod spec;

use core::f32::consts::{FRAC_1_PI, FRAC_1_SQRT_2, FRAC_2_PI, PI, TAU};
use fm::FourOpFmVoice;
use leviathan_core::{CvFrame, SoundLock, NUM_LANES};
use master::{MasterBus, MAX_SEND};
use sampler::{SampleBank, VarClockSamplerVoice};

pub use master::{MasterPeaks, StereoDelay};
pub use spec::{default_lane_patches, machine_spec, LanePatch, MachineSpec, ParamSpec, DEFAULT_LANE_MACHINES, DEFAULT_LANE_PATCHES, MACHINE_SPECS};

pub const MACHINE_COUNT: usize = 7;
pub const ALL_LANES_AUDIBLE: u8 = (1 << NUM_LANES) - 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneChannel {
    pub level: f32,
    pub pan: f32,
    pub delay_send: f32,
}

impl LaneChannel {
    const fn dry(level: f32, pan: f32) -> Self {
        Self {
            level,
            pan,
            delay_send: 0.0,
        }
    }

    pub fn is_valid(self) -> bool {
        // Range checks also reject NaN and infinities.
        (0.0..=1.25).contains(&self.level)
            && (-1.0..=1.0).contains(&self.pan)
            && (0.0..=1.0).contains(&self.delay_send)
    }

    fn slew(&mut self, target: Self, coefficient: f32) {
        self.level = slew(self.level, target.level, coefficient);
        self.pan = slew(self.pan, target.pan, coefficient);
        self.delay_send = slew(self.delay_send, target.delay_send, coefficient);
    }
}

// Match the stereo gains of the original fixed lane spread in old projects.
pub const DEFAULT_LANE_CHANNELS: [LaneChannel; NUM_LANES] = [
    LaneChannel::dry(0.824_621_14, -0.668_698_43),
    LaneChannel::dry(0.751_531_8, -0.421_323_33),
    LaneChannel::dry(0.712_179_8, -0.144_335_06),
    LaneChannel::dry(0.712_179_8, 0.144_335_06),
    LaneChannel::dry(0.751_531_8, 0.421_323_33),
    LaneChannel::dry(0.824_621_14, 0.668_698_43),
];

#[inline]
fn stereo_pan(pan: f32) -> [f32; 2] {
    // Rotate the unit vector at center using the tangent half-angle formula.
    // Factoring the numerators gives exact zeros at the hard-pan endpoints.
    const TAN_PI_8_SQUARED: f32 = 0.171_572_88;
    let scale = FRAC_1_SQRT_2 / (1.0 + TAN_PI_8_SQUARED * pan * pan);
    let offset = TAN_PI_8_SQUARED * pan;
    [
        (1.0 - pan) * (1.0 + offset) * scale,
        (1.0 + pan) * (1.0 - offset) * scale,
    ]
}

#[inline]
fn slew(value: f32, target: f32, coefficient: f32) -> f32 {
    let difference = target - value;
    if difference.abs() < 1e-5 {
        target
    } else {
        value + coefficient * difference
    }
}

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
    pub const ALL: [Self; MACHINE_COUNT] = [
        Self::KickDrum,
        Self::FourOpFm,
        Self::SubBass,
        Self::NoiseWash,
        Self::VarSampler,
        Self::AcidLead,
        Self::ChordStab,
    ];

    pub const fn from_id(id: usize) -> Self {
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

    pub const fn from_patch_and_lock(patch: &LanePatch, lock: &SoundLock) -> Self {
        if lock.machine >= 0 {
            Self::from_id(lock.machine as usize)
        } else {
            patch.machine
        }
    }

    pub const fn spec(self) -> &'static MachineSpec {
        spec::machine_spec(self)
    }

    pub const fn slug(self) -> &'static str {
        self.spec().slug
    }

    pub const fn display_name(self) -> &'static str {
        self.spec().name
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        MACHINE_SPECS
            .iter()
            .find(|s| s.slug == slug)
            .map(|s| s.kind)
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
    output_gain: f32,
    channel: LaneChannel,
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
            output_gain: 1.0,
            channel: LaneChannel::dry(1.0, 0.0),
            ic1eq: 0.0,
            ic2eq: 0.0,
            rng: 0x9e37_79b9,
            fm: FourOpFmVoice::default(),
            sampler: VarClockSamplerVoice::default(),
        }
    }

impl VoiceState {
    pub fn reset_synthesis(&mut self) {
        self.phase = 0.0;
        self.env = 0.0;
        self.pitch_env = 0.0;
        self.prev_gate = false;
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
        self.fm = FourOpFmVoice::default();
        self.sampler = VarClockSamplerVoice::default();
    }
}
}

pub struct SynthRack {
    sample_rate: f32,
    voices: [VoiceState; NUM_LANES],
    audible_mask: u8,
    channels: [LaneChannel; NUM_LANES],
    lane_slew: f32,
    master: MasterBus,
    pub sample_bank: SampleBank,
    pub counter: ByteBusCounter,
    lane_patches: [LanePatch; NUM_LANES],
}

impl SynthRack {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8_000.0);
        Self {
            sample_rate,
            voices: Self::fresh_voices(ALL_LANES_AUDIBLE, DEFAULT_LANE_CHANNELS),
            audible_mask: ALL_LANES_AUDIBLE,
            channels: DEFAULT_LANE_CHANNELS,
            lane_slew: 1.0 / (1.0 + sample_rate * 0.004),
            master: MasterBus::new(sample_rate),
            sample_bank: SampleBank::new(),
            counter: ByteBusCounter::default(),
            lane_patches: DEFAULT_LANE_PATCHES,
        }

    pub fn lane_patches(&self) -> &[LanePatch; NUM_LANES] {
        &self.lane_patches
    }

    pub fn lane_patch(&self, lane: usize) -> LanePatch {
        self.lane_patches[lane.min(NUM_LANES - 1)]
    }

    pub fn set_lane_patch(&mut self, lane: usize, patch: LanePatch) {
        if lane < NUM_LANES {
            let clean = patch.sanitized();
            if self.lane_patches[lane].machine != clean.machine {
                self.voices[lane].reset_synthesis();
            }
            self.lane_patches[lane] = clean;
        }
    }

    pub fn set_lane_patches(&mut self, patches: [LanePatch; NUM_LANES]) {
        for (lane, patch) in patches.into_iter().enumerate() {
            self.set_lane_patch(lane, patch);
        }
    }    }

    fn fresh_voices(
        audible_mask: u8,
        channels: [LaneChannel; NUM_LANES],
    ) -> [VoiceState; NUM_LANES] {
        let mut voices = core::array::from_fn(|_| VoiceState::default());
        for (i, v) in voices.iter_mut().enumerate() {
            v.rng ^= (i as u32 + 1).wrapping_mul(0x85eb_ca6b);
            v.output_gain = lane_target(audible_mask, i);
            v.channel = channels[i];
        }
        voices
    }

    pub fn reset_voices(&mut self) {
        self.voices = Self::fresh_voices(self.audible_mask, self.channels);
        self.counter = ByteBusCounter::default();
    }

    pub fn set_lane_audible_mask(&mut self, mask: u8) {
        self.audible_mask = mask & ALL_LANES_AUDIBLE;
    }

    pub fn set_lane_channels(&mut self, channels: [LaneChannel; NUM_LANES]) {
        if channels.iter().all(|channel| channel.is_valid()) {
            self.channels = channels;
        }
    }

    pub fn set_master_send(&mut self, send: f32) {
        self.master.set_send(send);
    }

    pub fn reset_master(&mut self, send: f32) {
        self.master.reset(send);
    }

    pub fn master_peaks(&self) -> MasterPeaks {
        self.master.peaks
    }

    pub fn clear_master_peaks(&mut self) {
        self.master.peaks = MasterPeaks::default();
    }

    pub fn retrigger_gates(&mut self) {
        self.retrigger_lanes(u8::MAX);
    }

    pub fn retrigger_lanes(&mut self, lanes: u8) {
        for (lane, voice) in self.voices.iter_mut().enumerate() {
            if lanes & (1 << lane) != 0 {
                voice.prev_gate = false;
            }
        }
    }

    #[inline]
    pub fn render_stereo(&mut self, cv: &CvFrame) -> (f32, f32) {
        self.render_mix(cv, None, None)
    }

    /// Renders the master and taps each lane after gain and pan, before the master bus.
    #[inline]
    pub fn render_stereo_with_stems(
        &mut self,
        cv: &CvFrame,
        stems: &mut [[f32; 2]; NUM_LANES],
    ) -> (f32, f32) {
        self.render_mix(cv, Some(stems), None)
    }

    /// Includes each lane's own contribution to the delay in its stem.
    pub fn render_stereo_with_wet_stems(
        &mut self,
        cv: &CvFrame,
        stems: &mut [[f32; 2]; NUM_LANES],
        delays: &mut [StereoDelay; NUM_LANES],
    ) -> (f32, f32) {
        self.render_mix(cv, Some(stems), Some(delays))
    }

    #[inline]
    fn render_mix(
        &mut self,
        cv: &CvFrame,
        mut stems: Option<&mut [[f32; 2]; NUM_LANES]>,
        mut stem_delays: Option<&mut [StereoDelay; NUM_LANES]>,
    ) -> (f32, f32) {
        let _bus_step = self.counter.step(cv.bytebus_v[8] > 5.0, cv.reset_v > 5.0);
        let dt = 1.0 / self.sample_rate;
        let mut dry_mix = [0.0_f32; 2];
        let mut delay_mix = [0.0_f32; 2];
        let master_send = self.master.next_send();

        for (lane, v) in self.voices.iter_mut().enumerate() {
            let gate_high = cv.gate_v[lane] > 2.5;
            let raw_lock = &cv.locks[lane];
let patch = &self.lane_patches[lane];
let resolved_lock = patch.resolve_lock(raw_lock);
let lock = &resolved_lock;
let machine = MachineKind::from_patch_and_lock(patch, raw_lock);
            
            if gate_high && (!v.prev_gate || cv.retrigger_mask & (1 << lane) != 0) {
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
            MachineKind::KickDrum => 48.0 + (175.0 + (lock.values[4] - 0.5) * 160.0) * v.pitch_env,
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
                let drive = 1.6 + (lock.values[1] - 0.5) * 2.0;
                fast_tanh(drive * (0.65 * fast_sin_turns(v.phase) + 0.35 * saw)) * v.env
            }
            MachineKind::NoiseWash => noise * v.env * (0.55 + (lock.values[1] - 0.5) * 0.70),
            MachineKind::VarSampler => {
                v.sampler.step_sample_gated(
                    &self.sample_bank,
                    base_hz,
                    self.sample_rate,
                    lock,
                    gate_high,
                ) * v.env
            }
            MachineKind::AcidLead => {
                let saw = 2.0 * v.phase - 1.0;
                let shaped = if (lock.values[1] - 0.5).abs() < 1e-6 {
                    saw
                } else {
                    fast_tanh((1.0 + lock.values[1] * 2.4) * saw)
                };
                shaped * v.env
            }
            MachineKind::ChordStab => {
                let fifth =
                    fast_sin_turns(v.phase * (1.498_307 + (lock.values[1] - 0.5) * 0.04));
                0.5 * (fast_sin_turns(v.phase) + fifth) * v.env
            }
        };

        let cutoff_norm = if raw_lock.has_param(2) {
            raw_lock.values[2]
        } else {
            (0.22 + (lock.values[2] - 0.5) * 0.8 + 0.52 * v.env).clamp(0.05, 0.88)
        }
        .clamp(0.04, 0.88);
        let g = (cutoff_norm * 0.72).clamp(0.01, 0.78);
        let base_k = if matches!(machine, MachineKind::AcidLead) {
            0.55_f32
        } else {
            0.85_f32
        };
        let k = if matches!(machine, MachineKind::FourOpFm | MachineKind::VarSampler) {
            base_k
        } else {
            (base_k - (lock.values[3] - 0.5) * 0.70).clamp(0.12, 1.40)
        };
            let a1 = 1.0 / (1.0 + g * (g + k));
            let a2 = g * a1;
            let a3 = g * a2;
            let v3 = raw - v.ic2eq;
            let v1 = a1 * v.ic1eq + a2 * v3;
            let v2 = v.ic2eq + a2 * v.ic1eq + a3 * v3;
            v.ic1eq = 2.0 * v1 - v.ic1eq;
            v.ic2eq = 2.0 * v2 - v.ic2eq;

            // Keep the voice running under the mute so it resumes at the same phase.
            v.output_gain = slew(
                v.output_gain,
                lane_target(self.audible_mask, lane),
                self.lane_slew,
            );
            v.channel.slew(self.channels[lane], self.lane_slew);
            let signal = v2 * v.output_gain * v.channel.level * 0.45;
            let dry = stereo_pan(v.channel.pan).map(|gain| signal * gain);
            // The master send raises the baseline for every channel. The lane
            // send uses the remaining range so the total stays bounded.
            let send_gain = master_send + v.channel.delay_send * (MAX_SEND - master_send);
            let send = dry.map(|sample| sample * send_gain);
            for channel in 0..2 {
                dry_mix[channel] += dry[channel];
                delay_mix[channel] += send[channel];
            }
            if let Some(stems) = &mut stems {
                stems[lane] = if let Some(delays) = &mut stem_delays {
                    delays[lane].mix(dry, send)
                } else {
                    dry
                };
            }
        }

        let [left, right] = self.master.process(dry_mix, delay_mix);
        (left, right)
    }
}

fn lane_target(audible_mask: u8, lane: usize) -> f32 {
    if audible_mask & (1 << lane) != 0 {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leviathan_core::{EngineConfig, NoteCell, Pattern, TrackerEvaluator};

    #[test]
    fn pan_is_constant_power_and_defaults_reproduce_the_old_spread() {
        assert_eq!(stereo_pan(0.0), [FRAC_1_SQRT_2; 2]);
        assert_eq!(stereo_pan(-1.0)[1], 0.0);
        assert_eq!(stereo_pan(1.0)[0], 0.0);
        for i in -100..=100 {
            let [l, r] = stereo_pan(i as f32 * 0.01);
            assert!((l * l + r * r - 1.0).abs() < 5e-7);
        }
        for (lane, channel) in DEFAULT_LANE_CHANNELS.iter().enumerate() {
            let actual = stereo_pan(channel.pan).map(|gain| gain * channel.level);
            let old = 0.2 + 0.6 * lane as f32 / 5.0;
            assert!((actual[0] - (1.0 - old)).abs() < 2e-7);
            assert!((actual[1] - old).abs() < 2e-7);
            assert_eq!(channel.delay_send, 0.0);
        }
    }

    #[test]
    fn live_mixer_changes_slew_then_reach_the_snapshot_values() {
        let mut rack = SynthRack::new(48_000.0);
        let mut target = DEFAULT_LANE_CHANNELS;
        target[0] = LaneChannel {
            level: 0.0,
            pan: 1.0,
            delay_send: 1.0,
        };
        rack.set_lane_channels(target);
        rack.render_stereo(&CvFrame::default());
        let actual = rack.voices[0].channel;
        assert!((0.8..DEFAULT_LANE_CHANNELS[0].level).contains(&actual.level));
        assert!((DEFAULT_LANE_CHANNELS[0].pan..-0.65).contains(&actual.pan));
        assert!((0.0..0.01).contains(&actual.delay_send));
        assert!(actual.delay_send > 0.0);
        for _ in 0..4096 {
            rack.render_stereo(&CvFrame::default());
        }
        assert_eq!(rack.voices[0].channel, target[0]);
        assert_eq!(rack.voices[1].channel, target[1]);
    }

    #[test]
    fn lane_mute_slews_for_a_sustained_voice_and_keeps_other_lanes_audible() {
        let cv = CvFrame {
            gate_v: [10.0; NUM_LANES],
            velocity_v: [10.0; NUM_LANES],
            ..CvFrame::default()
        };
        let mut rack = SynthRack::new(48_000.0);
        let mut stems = [[0.0; 2]; NUM_LANES];
        for _ in 0..160 {
            rack.render_stereo_with_stems(&cv, &mut stems);
        }
        rack.set_lane_audible_mask(ALL_LANES_AUDIBLE & !(1 << 1));
        let mut previous = 1.0;
        for _ in 0..192 {
            rack.render_stereo_with_stems(&cv, &mut stems);
            let gain = rack.voices[1].output_gain;
            assert!(gain <= previous && previous - gain < 0.006);
            previous = gain;
        }
        assert!((0.36..0.38).contains(&previous));
        assert_eq!(rack.voices[0].output_gain, 1.0);
        for _ in 0..3000 {
            rack.render_stereo_with_stems(&cv, &mut stems);
        }
        assert_eq!(stems[1], [0.0; 2]);
        assert!(stems[0].iter().any(|value| value.abs() > 1e-5));

        rack.set_lane_audible_mask(ALL_LANES_AUDIBLE);
        rack.render_stereo_with_stems(&cv, &mut stems);
        assert!((0.0..0.006).contains(&rack.voices[1].output_gain));
        assert!(rack.voices[1].prev_gate);
    }

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
    fn master_bounds_hot_six_lane_chords_and_reports_the_overload() {
        let lock = SoundLock {
            machine: MachineKind::AcidLead as i16,
            mask: 0b101,
            values: [0.98, 0.0, 0.88, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        let cv = CvFrame {
            pitch_v: [-12, -8, -5, 0, 4, 7].map(|semitones| semitones as f32 / 12.0),
            gate_v: [10.0; NUM_LANES],
            velocity_v: [10.0; NUM_LANES],
            locks: [lock; NUM_LANES],
            ..CvFrame::default()
        };
        let mut rack = SynthRack::new(48_000.0);
        rack.reset_master(1.0);
        for _ in 0..8192 {
            let (left, right) = rack.render_stereo(&cv);
            assert!((-1.0..=1.0).contains(&left));
            assert!((-1.0..=1.0).contains(&right));
        }
        let peaks = rack.master_peaks();
        assert_eq!(peaks.clipped, [true, true], "{peaks:?}");
        assert!(peaks.output.iter().all(|peak| *peak > 0.9));
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
    #[test]
    fn dual_lanes_same_machine_render_independent_base_patches() {
        let mut rack = SynthRack::new(48_000.0);
        let mut patch_a = LanePatch::for_machine(MachineKind::FourOpFm);
        patch_a.params[1] = 0.05;
        let mut patch_b = LanePatch::for_machine(MachineKind::FourOpFm);
        patch_b.params[1] = 0.95;
        rack.set_lane_patch(0, patch_a);
        rack.set_lane_patch(1, patch_b);

        let mut cv = CvFrame::default();
        cv.gate_v[0] = 5.0;
        cv.gate_v[1] = 5.0;
        cv.velocity_v[0] = 8.0;
        cv.velocity_v[1] = 8.0;

        let mut energy_a = 0.0_f32;
        let mut energy_b = 0.0_f32;
        let mut diff_energy = 0.0_f32;
        let mut stems = [[0.0_f32; 2]; NUM_LANES];
        for _ in 0..256 {
            let (l, r) = rack.render_stereo_with_stems(&cv, &mut stems);
            assert!(l.is_finite() && r.is_finite());
            let a = stems[0][0] + stems[0][1];
            let b = stems[1][0] + stems[1][1];
            energy_a += a * a;
            energy_b += b * b;
            diff_energy += (a - b) * (a - b);
        }
        assert!(energy_a > 1e-4);
        assert!(energy_b > 1e-4);
        assert!(diff_energy > 1e-3);
    }

    #[test]
    fn machine_spec_slugs_and_masked_step_locks_preserve_base_patch() {
        for kind in MachineKind::ALL {
            assert_eq!(MachineKind::from_slug(kind.slug()), Some(kind));
            assert_eq!(kind.short_name(), kind.spec().badge);
        }
        let mut patch = LanePatch::for_machine(MachineKind::ChordStab);
        patch.params[0] = 0.90;
        patch.params[2] = 0.80;
        let mut step_lock = SoundLock::default();
        step_lock.mask = 1 << 2;
        step_lock.values[2] = 0.12;
        let resolved = patch.resolve_lock(&step_lock);
        assert_eq!(resolved.machine, MachineKind::ChordStab as i16);
        assert!((resolved.values[0] - 0.90).abs() < 1e-6);
        assert!((resolved.values[2] - 0.12).abs() < 1e-6);
    }
}
