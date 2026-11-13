#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(test, not(feature = "std")))]
extern crate std;

mod bbus;
mod bender;
mod byte_route;
mod captured;
mod cz;
pub mod dac;
pub mod fm;
mod fs77;
mod master;
mod otis;
mod otis_twin;
mod perc;
mod pvx;
pub mod sampler;
pub mod spec;
mod vast;
mod vast_k26;
mod voice;
mod west;

use core::f32::consts::{FRAC_1_PI, FRAC_1_SQRT_2, FRAC_2_PI, PI, TAU};
use fm::FourOpFmVoice;
use leviathan_core::rack_routing::{LaneInput, RackRouting};
use leviathan_core::{CvFrame, SoundLock, NUM_LANES};
use master::{MasterBus, MAX_SEND};
use sampler::{SampleBank, VarClockSamplerVoice};

pub use bbus::{ByteBusReceiver, ByteBusSource};
pub use bender::{BenderParams, BENDER_MAX_SHIFT_HZ};
pub use captured::{CaptureSample, CaptureSamples, CaptureTap};
pub use master::{DelayMode, MasterPeaks, StereoDelay};
pub use spec::{
    default_lane_patches, machine_spec, LanePatch, MachineSpec, ParamSpec, DEFAULT_LANE_MACHINES,
    DEFAULT_LANE_PATCHES, MACHINE_SPECS,
};

pub const MACHINE_COUNT: usize = 17;
pub const ALL_LANES_AUDIBLE: u8 = (1 << NUM_LANES) - 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneChannel {
    pub level: f32,
    pub pan: f32,
    pub delay_send: f32,
    pub fx_send: f32,
}

impl LaneChannel {
    const fn dry(level: f32, pan: f32) -> Self {
        Self {
            level,
            pan,
            delay_send: 0.0,
            fx_send: 0.0,
        }
    }

    pub fn is_valid(self) -> bool {
        // Range checks also reject NaN and infinities.
        (0.0..=1.25).contains(&self.level)
            && (-1.0..=1.0).contains(&self.pan)
            && (0.0..=1.0).contains(&self.delay_send)
            && (0.0..=1.0).contains(&self.fx_send)
    }

    fn slew(&mut self, target: Self, coefficient: f32) {
        self.level = slew(self.level, target.level, coefficient);
        self.pan = slew(self.pan, target.pan, coefficient);
        self.delay_send = slew(self.delay_send, target.delay_send, coefficient);
        self.fx_send = slew(self.fx_send, target.fx_send, coefficient);
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
    CzPhaseDist = 7,
    PolivoksSub = 8,
    VastFunSynth = 9,
    ElectroPerc = 10,
    WestCoastLpg = 11,
    OtisTranswave = 12,
    ByteBusR2r = 13,
    OtisTwin = 14,
    Fs77FormantFm = 15,
    VastTriple = 16,
}

impl MachineKind {
    pub const fn from_lane_and_lock(lane: usize, lock: &SoundLock) -> Self {
        let id = if lock.machine >= 0 {
            lock.machine as usize
        } else {
            lane
        };
        Self::from_id(id)
    }

    pub const fn short_name(self) -> &'static str {
        self.spec().badge
    }
    pub const ALL: [Self; MACHINE_COUNT] = [
        Self::KickDrum,
        Self::FourOpFm,
        Self::SubBass,
        Self::NoiseWash,
        Self::VarSampler,
        Self::AcidLead,
        Self::ChordStab,
        Self::CzPhaseDist,
        Self::PolivoksSub,
        Self::VastFunSynth,
        Self::ElectroPerc,
        Self::WestCoastLpg,
        Self::OtisTranswave,
        Self::ByteBusR2r,
        Self::OtisTwin,
        Self::Fs77FormantFm,
        Self::VastTriple,
    ];

    pub const fn from_id(id: usize) -> Self {
        MACHINE_SPECS[id % MACHINE_COUNT].kind
    }

    pub const fn from_patch_and_lock(patch: &LanePatch, lock: &SoundLock) -> Self {
        if lock.machine >= 0 && (lock.machine as usize) < MACHINE_COUNT {
            MACHINE_SPECS[lock.machine as usize].kind
        } else {
            patch.machine
        }
    }

    const fn is_expansion(self) -> bool {
        matches!(
            self,
            Self::CzPhaseDist
                | Self::PolivoksSub
                | Self::VastFunSynth
                | Self::ElectroPerc
                | Self::WestCoastLpg
                | Self::OtisTranswave
                | Self::ByteBusR2r
                | Self::OtisTwin
                | Self::Fs77FormantFm
                | Self::VastTriple
        )
    }

    pub const fn is_step_lock_valid(id: i16) -> bool {
        id == -1 || (id >= 0 && (id as usize) < MACHINE_COUNT)
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
    // Sixth-order series for 2^frac: within 0.02 cents over the octave. The
    // old cubic reached only 1.989 at the top, seven cents flat at the seventh.
    let poly = 1.0
        + frac
            * (0.693_147_2
                + frac
                    * (0.240_226_5
                        + frac
                            * (0.055_504_1
                                + frac * (0.009_618_13 + frac * (0.001_333_36 + frac * 0.000_154_04)))));
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
    capture: captured::Playback,
    capture_gain: f32,
    cz: cz::Voice,
    pvx: pvx::Voice,
    vast: vast::Voice,
    vast_k26: vast_k26::Voice,
    perc: perc::Voice,
    west: west::Voice,
    otis: otis::Voice,
    otis_twin: otis_twin::TwinVoice,
    bbus: bbus::Voice,
    fs77: fs77::Voice,
    active_machine: Option<MachineKind>,
    note_pitch: f32,
    note_lock: SoundLock,
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
            capture: captured::Playback::default(),
            capture_gain: 0.0,
            cz: cz::Voice::default(),
            pvx: pvx::Voice::default(),
            vast: vast::Voice::default(),
            vast_k26: vast_k26::Voice::default(),
            perc: perc::Voice::default(),
            west: west::Voice::default(),
            otis: otis::Voice::default(),
            otis_twin: otis_twin::TwinVoice::default(),
            bbus: bbus::Voice::default(),
            fs77: fs77::Voice::default(),
            active_machine: None,
            note_pitch: 0.0,
            note_lock: SoundLock::default(),
        }
    }
}
impl VoiceState {
    fn reset_synthesis(&mut self) {
        self.phase = 0.0;
        self.env = 0.0;
        self.pitch_env = 0.0;
        self.prev_gate = false;
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
        self.fm = FourOpFmVoice::default();
        self.sampler = VarClockSamplerVoice::default();
        self.capture = captured::Playback::default();
        self.capture_gain = 0.0;
        self.cz = cz::Voice::default();
        self.pvx = pvx::Voice::default();
        self.vast.reset();
        self.vast_k26.reset();
        self.perc = perc::Voice::default();
        self.west = west::Voice::default();
        self.otis = otis::Voice::default();
        self.otis_twin = otis_twin::TwinVoice::default();
        self.bbus = bbus::Voice::default();
        self.fs77 = fs77::Voice::default();
        self.active_machine = None;
    }

    #[inline]
    fn render(
        &mut self,
        cv: &CvFrame,
        lane: usize,
        patch: &LanePatch,
        sample_rate: f32,
        sample_bank: &SampleBank,
        bus_word: Option<u8>,
        calibrated: bool,
    ) -> f32 {
        let gate = cv.gate_v[lane] > 2.5;
        let velocity = cv.velocity_v[lane];
        let mut pitch = cv.pitch_v[lane];
        let mut raw_lock = &cv.locks[lane];
        let machine = MachineKind::from_patch_and_lock(patch, raw_lock);
        let mut attack = gate && (!self.prev_gate || cv.retrigger_mask & (1 << lane) != 0);
        // CV holds the last lock during rests. A new machine takes over on
        // a gate, allowing an expansion voice's own release to complete.
        let previous = self.active_machine;
        if gate
            && previous != Some(machine)
            && (machine.is_expansion() || previous.is_some_and(MachineKind::is_expansion))
        {
            self.reset_synthesis();
            attack = true;
        }
        self.prev_gate = gate;
        if gate {
            self.active_machine = Some(machine);
            self.note_pitch = pitch;
            self.note_lock = *raw_lock;
        }
        if let Some(active) = self.active_machine.filter(|machine| machine.is_expansion()) {
            let hz = voice::pitch_hz(self.note_pitch);
            let params = voice::controls(
                patch.resolve_lock(&self.note_lock).values,
                &active.spec().params,
            );
            let trigger = attack.then(|| {
                if velocity.is_finite() {
                    (velocity / 10.0).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            });
            return match active {
                MachineKind::CzPhaseDist => self.cz.step(
                    hz, sample_rate, gate, trigger, &params, &mut self.rng, calibrated,
                ),
                MachineKind::PolivoksSub => {
                    self.pvx
                        .step(hz, sample_rate, trigger, &params, &mut self.rng)
                }
                MachineKind::VastFunSynth => self.vast.step(hz, sample_rate, trigger, &params),
                MachineKind::VastTriple => self.vast_k26.step(hz, sample_rate, gate, trigger, &params),
                MachineKind::ElectroPerc => {
                    self.perc.step(hz, sample_rate, trigger, &params, &mut self.rng)
                }
                MachineKind::WestCoastLpg => self.west.step(hz, sample_rate, gate, trigger, &params),
                MachineKind::OtisTranswave => {
                    self.otis
                        .step(hz, sample_rate, trigger, &params, calibrated, None)
                }
                MachineKind::OtisTwin => {
                    self.otis_twin
                        .step(hz, sample_rate, gate, trigger, &params, calibrated, None)
                }
                MachineKind::ByteBusR2r => {
                    self.bbus.step(hz, sample_rate, gate, trigger, bus_word, &params)
                }
                MachineKind::Fs77FormantFm => {
                    self.fs77.step(hz, sample_rate, gate, trigger, &params, &mut self.rng)
                }
                _ => 0.0,
            };
        }

        // An expansion lock can arrive before the gate. Keep the previous
        // legacy note's pitch and locks until the new machine is triggered.
        let machine = if machine.is_expansion() {
            let Some(previous) = previous else {
                return 0.0;
            };
            pitch = self.note_pitch;
            raw_lock = &self.note_lock;
            previous
        } else {
            machine
        };
        let lock = patch.resolve_lock(raw_lock);
        if attack {
            self.env = (velocity / 10.0).clamp(0.1, 1.0);
            self.pitch_env = 1.0;
            self.fm.trigger();
            self.sampler.trigger(sample_bank, &lock);
        }
        let dt = 1.0 / sample_rate;
        let decay_lock = lock_param(&lock, 0, 0.35).clamp(0.03, 0.98);
        let decay_rate = 1.0 - (dt / (0.04 + 0.85 * decay_lock));
        self.env *= decay_rate;
        self.pitch_env *= 1.0 - (dt / 0.022);

        let base_hz = v_oct_to_hz(pitch);
        let freq = match machine {
            MachineKind::KickDrum => {
                48.0 + (175.0 + (lock.values[4] - 0.5) * 160.0) * self.pitch_env
            }
            MachineKind::SubBass => base_hz * 0.5,
            _ => base_hz,
        };
        self.phase = wrap_turns(self.phase + freq * dt);
        let noise = voice::noise(&mut self.rng);

        let raw = match machine {
            MachineKind::KickDrum => {
                let drive = 1.8 + 2.2 * lock_param(&lock, 1, 0.35).clamp(0.0, 1.0);
                fast_tanh(drive * fast_sin_turns(self.phase) * self.env)
            }
            MachineKind::FourOpFm => {
                if calibrated {
                    self.fm.step_calibrated(base_hz, sample_rate, self.env, &lock)
                } else {
                    self.fm.step_sample(base_hz, sample_rate, self.env, &lock)
                }
            }
            MachineKind::SubBass => {
                let saw = 2.0 * self.phase - 1.0;
                let drive = 1.6 + (lock.values[1] - 0.5) * 2.0;
                fast_tanh(drive * (0.65 * fast_sin_turns(self.phase) + 0.35 * saw)) * self.env
            }
            MachineKind::NoiseWash => noise * self.env * (0.55 + (lock.values[1] - 0.5) * 0.70),
            MachineKind::VarSampler => {
                self.sampler
                    .step_sample_gated(sample_bank, base_hz, sample_rate, &lock, gate)
                    * self.env
            }
            MachineKind::AcidLead => {
                let saw = 2.0 * self.phase - 1.0;
                let shaped = if (lock.values[1] - 0.5).abs() < 1e-6 {
                    saw
                } else {
                    fast_tanh((1.0 + lock.values[1] * 2.4) * saw)
                };
                shaped * self.env
            }
            MachineKind::ChordStab => {
                let fifth =
                    fast_sin_turns(self.phase * (1.498_307 + (lock.values[1] - 0.5) * 0.04));
                0.5 * (fast_sin_turns(self.phase) + fifth) * self.env
            }
            _ => 0.0,
        };

        let cutoff_norm = if raw_lock.has_param(2) {
            raw_lock.values[2]
        } else {
            (0.22 + (lock.values[2] - 0.5) * 0.8 + 0.52 * self.env).clamp(0.05, 0.88)
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
        let v3 = raw - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;
        if calibrated {
            v2 * match machine {
                MachineKind::FourOpFm => 1.58,
                MachineKind::SubBass => 1.26,
                MachineKind::NoiseWash => 1.42,
                MachineKind::VarSampler => 0.85,
                MachineKind::AcidLead => 0.56,
                MachineKind::ChordStab => 1.68,
                _ => 1.0,
            }
        } else {
            v2
        }
    }

    #[inline]
    fn render_capture(
        &mut self,
        cv: &CvFrame,
        lane: usize,
        patch: &LanePatch,
        rate: f32,
        sample_bank: &SampleBank,
        bus_word: Option<u8>,
        calibrated: bool,
        samples: &CaptureSamples<'_>,
        routing: &RackRouting,
        routed: &[[f32; 2]; NUM_LANES],
        frame_byte: Option<u8>,
    ) -> [f32; 2] {
        let input = routing.inputs[lane];
        if input == LaneInput::Instrument {
            return [self.render(cv, lane, patch, rate, sample_bank, bus_word, calibrated); 2];
        }
        let gate = cv.gate_v[lane] > 2.5;
        let requested = MachineKind::from_patch_and_lock(patch, &cv.locks[lane]);
        let machine = if gate {
            requested
        } else {
            self.active_machine.unwrap_or(patch.machine)
        };
        let sampled = matches!(
            machine,
            MachineKind::VarSampler
                | MachineKind::OtisTranswave
                | MachineKind::OtisTwin
                | MachineKind::VastFunSynth
        );
        let through = match input {
            LaneInput::Lane(source) if machine == MachineKind::VastFunSynth => {
                Some(usize::from(source))
            }
            _ => None,
        };
        let captured = input == LaneInput::Capture && sampled;
        if !captured && through.is_none() {
            self.capture.finish();
            return [self.render(cv, lane, patch, rate, sample_bank, bus_word, calibrated); 2];
        }

        let switching = self.active_machine != Some(machine);
        if switching {
            self.reset_synthesis();
        }
        let attack = gate && (!self.prev_gate || cv.retrigger_mask & (1 << lane) != 0);
        self.prev_gate = gate;
        self.active_machine = Some(machine);
        if gate {
            self.note_pitch = cv.pitch_v[lane];
            self.note_lock = cv.locks[lane];
        }
        let trigger = attack.then(|| {
            let velocity = cv.velocity_v[lane] / 10.0;
            if velocity.is_finite() {
                velocity.clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        let hz = voice::pitch_hz(self.note_pitch);
        if let Some(velocity) = trigger.filter(|_| captured) {
            self.capture.trigger(
                samples.current,
                routing.region,
                usize::from(routing.frame_size),
            );
            self.capture_gain = velocity;
        }
        let take = samples.get(self.capture.generation());
        if machine == MachineKind::VarSampler {
            return take.map_or([0.0; 2], |take| {
                let gain = self.capture_gain * if calibrated { 0.85 } else { 1.0 };
                self.capture
                    .step(take, hz / sampler::C4_HZ, rate, gate)
                    .map(|sample| sample * gain)
            });
        }
        let parameters = voice::controls(
            patch.resolve_lock(&self.note_lock).values,
            &machine.spec().params,
        );
        let mono = |pair: [f32; 2]| 0.5 * (pair[0] + pair[1]);
        match machine {
            MachineKind::VastFunSynth => {
                let input = if let Some(source) = through {
                    vast::Input::Through(mono(routed[source]))
                } else {
                    let pair = take.map_or([0.0; 2], |take| {
                        self.capture.step(take, hz / sampler::C4_HZ, rate, gate)
                    });
                    vast::Input::Gated(mono(pair))
                };
                [self.vast.step_input(hz, rate, trigger, &parameters, input); 2]
            }
            MachineKind::OtisTranswave | MachineKind::OtisTwin => {
                let source = take.map(|take| {
                    let bus = if routing.frame_bytebus {
                        frame_byte
                    } else {
                        None
                    };
                    self.capture.frames(take, bus)
                });
                if source.is_none() {
                    return [0.0; 2];
                }
                let signal = if machine == MachineKind::OtisTranswave {
                    let signal = self
                        .otis
                        .step(hz, rate, trigger, &parameters, calibrated, source);
                    if self.otis.is_silent() {
                        self.capture.finish();
                    }
                    signal
                } else {
                    let signal = self.otis_twin.step(
                        hz,
                        rate,
                        gate,
                        trigger,
                        &parameters,
                        calibrated,
                        source,
                    );
                    if self.otis_twin.is_silent() {
                        self.capture.finish();
                    }
                    signal
                };
                [signal; 2]
            }
            _ => [0.0; 2],
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
    voice_calibration: bool,
    routing: RackRouting,
    render_order: [usize; NUM_LANES],
    byte_register: byte_route::Register,
    routed_byte: Option<u8>,
}

// Leave stack headroom for worker initialization and Windows test threads.
const _: () = assert!(core::mem::size_of::<SynthRack>() < 262_144);

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
            voice_calibration: false,
            routing: RackRouting::default(),
            render_order: core::array::from_fn(|lane| lane),
            byte_register: byte_route::Register::default(),
            routed_byte: None,
        }
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
    }

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

    pub fn set_routing(&mut self, routing: RackRouting) -> bool {
        let Some(order) = routing.render_order() else {
            return false;
        };
        for (lane, voice) in self.voices.iter_mut().enumerate() {
            if self.routing.inputs[lane] != routing.inputs[lane] {
                voice.reset_synthesis();
            }
        }
        let was_clocked = self.routing.bytebus.enabled || self.routing.frame_bytebus;
        let clocked = routing.bytebus.enabled || routing.frame_bytebus;
        self.routing = routing;
        self.render_order = order;
        if was_clocked != clocked {
            self.reset_byte_route();
        }
        true
    }

    pub fn reset_byte_route(&mut self) {
        self.byte_register = byte_route::Register::default();
        self.byte_register.reset(self.routing.bytebus.reset_word);
        self.routed_byte = None;
    }

    pub fn routed_byte(&self) -> Option<u8> {
        self.routed_byte
    }

    pub fn capture_generations(&self) -> [Option<u64>; NUM_LANES] {
        self.voices
            .each_ref()
            .map(|voice| voice.capture.generation())
    }

    pub fn set_voice_calibration(&mut self, enabled: bool) {
        if self.voice_calibration != enabled {
            self.voice_calibration = enabled;
            self.reset_voices();
        }
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

    pub fn set_delay_mode(&mut self, mode: DelayMode) {
        self.master.set_delay_mode(mode);
    }

    pub fn set_bender_params(&mut self, params: BenderParams) {
        self.master.set_bender_params(params);
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
        self.render_mix(
            cv,
            None,
            None,
            bbus::tracker_word(cv),
            &CaptureSamples::default(),
            None,
        )
        .0
    }

    /// Renders with an already decoded external ByteBus word. Use
    /// `ByteBusReceiver` for raw clocked voltages and `None` for standalone.
    pub fn render_stereo_with_bytebus(&mut self, cv: &CvFrame, word: Option<u8>) -> (f32, f32) {
        self.render_mix(cv, None, None, word, &CaptureSamples::default(), None)
            .0
    }

    /// Renders the master and taps each lane after gain and pan, before the master bus.
    #[inline]
    pub fn render_stereo_with_stems(
        &mut self,
        cv: &CvFrame,
        stems: &mut [[f32; 2]; NUM_LANES],
    ) -> (f32, f32) {
        self.render_mix(
            cv,
            Some(stems),
            None,
            bbus::tracker_word(cv),
            &CaptureSamples::default(),
            None,
        )
        .0
    }

    /// Includes each lane's own contribution to the delay in its stem.
    pub fn render_stereo_with_wet_stems(
        &mut self,
        cv: &CvFrame,
        stems: &mut [[f32; 2]; NUM_LANES],
        delays: &mut [StereoDelay; NUM_LANES],
    ) -> (f32, f32) {
        self.render_stereo_with_fx_stems(cv, stems, delays).0
    }

    /// Renders the master, each lane's own delay contribution, and the shared
    /// BEND return as a separate stereo stem tapped before the master limiter.
    pub fn render_stereo_with_fx_stems(
        &mut self,
        cv: &CvFrame,
        stems: &mut [[f32; 2]; NUM_LANES],
        delays: &mut [StereoDelay; NUM_LANES],
    ) -> ((f32, f32), [f32; 2]) {
        let (master, wet, _) = self.render_mix(
            cv,
            Some(stems),
            Some(delays),
            bbus::tracker_word(cv),
            &CaptureSamples::default(),
            None,
        );
        (master, wet)
    }

    /// Render a live sample with an optional capture tap from the same mixer pass.
    pub fn render_session(
        &mut self,
        cv: &CvFrame,
        samples: &CaptureSamples<'_>,
        tap: Option<CaptureTap>,
    ) -> ((f32, f32), [f32; 2]) {
        let (master, _, sample) =
            self.render_mix(cv, None, None, bbus::tracker_word(cv), samples, tap);
        (master, sample)
    }

    pub fn render_session_with_stems(
        &mut self,
        cv: &CvFrame,
        samples: &CaptureSamples<'_>,
        stems: &mut [[f32; 2]; NUM_LANES],
        delays: &mut [StereoDelay; NUM_LANES],
    ) -> ((f32, f32), [f32; 2]) {
        let (master, wet, _) = self.render_mix(
            cv,
            Some(stems),
            Some(delays),
            bbus::tracker_word(cv),
            samples,
            None,
        );
        (master, wet)
    }

    #[inline]
    fn render_mix(
        &mut self,
        cv: &CvFrame,
        mut stems: Option<&mut [[f32; 2]; NUM_LANES]>,
        mut stem_delays: Option<&mut [StereoDelay; NUM_LANES]>,
        bus_word: Option<u8>,
        samples: &CaptureSamples<'_>,
        tap: Option<CaptureTap>,
    ) -> ((f32, f32), [f32; 2], [f32; 2]) {
        let _bus_step = self.counter.step(cv.bytebus_v[8] > 5.0, cv.reset_v > 5.0);
        let byte_routing = self.routing.bytebus.enabled || self.routing.frame_bytebus;
        let held_byte =
            byte_routing.then(|| self.byte_register.step(cv, bus_word, self.routing.bytebus));
        let routed_byte = if cv.run_v > 2.5 { held_byte } else { None };
        self.routed_byte = routed_byte;
        let bus_word = if self.routing.bytebus.enabled {
            routed_byte.or(bus_word)
        } else {
            bus_word
        };
        let mut dry_mix = [0.0_f32; 2];
        let mut delay_mix = [0.0_f32; 2];
        let mut fx_mix = [0.0_f32; 2];
        let master_send = self.master.next_send();
        let mut lane_dry = [[0.0; 2]; NUM_LANES];
        let mut lane_send = [[0.0; 2]; NUM_LANES];
        let mut lane_fx = [0.0; NUM_LANES];

        for lane in self.render_order {
            let v = &mut self.voices[lane];
            let voice = v.render_capture(
                cv,
                lane,
                &self.lane_patches[lane],
                self.sample_rate,
                &self.sample_bank,
                bus_word,
                self.voice_calibration,
                samples,
                &self.routing,
                &lane_dry,
                held_byte,
            );

            // Keep the voice running under the mute so it resumes at the same phase.
            v.output_gain = slew(
                v.output_gain,
                lane_target(self.audible_mask, lane),
                self.lane_slew,
            );
            v.channel.slew(self.channels[lane], self.lane_slew);
            let [pan_l, pan_r] = stereo_pan(v.channel.pan);
            let signal = voice.map(|sample| sample * v.output_gain * v.channel.level * 0.45);
            let dry = [signal[0] * pan_l, signal[1] * pan_r];
            // The master send raises the baseline for every channel. The lane
            // send uses the remaining range so the total stays bounded.
            let send_gain = master_send + v.channel.delay_send * (MAX_SEND - master_send);
            let send = dry.map(|sample| sample * send_gain);
            // Leave headroom for several lanes exciting the shared tank.
            let fx_gain = v.channel.fx_send * 0.5;
            lane_dry[lane] = dry;
            lane_send[lane] = send;
            lane_fx[lane] = fx_gain;
        }
        // Sum in lane order even when a source had to be rendered before its destination.
        for lane in 0..NUM_LANES {
            let dry = lane_dry[lane];
            let send = lane_send[lane];
            for channel in 0..2 {
                dry_mix[channel] += dry[channel];
                delay_mix[channel] += send[channel];
                fx_mix[channel] += dry[channel] * lane_fx[lane];
            }
            if let Some(stems) = &mut stems {
                stems[lane] = if let Some(delays) = &mut stem_delays {
                    delays[lane].mix(dry, send)
                } else {
                    dry
                };
            }
        }

        let ([left, right], wet) = self.master.process(dry_mix, delay_mix, fx_mix);
        let captured = match tap {
            Some(CaptureTap::Lane(lane)) => {
                lane_dry.get(usize::from(lane)).copied().unwrap_or_default()
            }
            Some(CaptureTap::Bend) => wet,
            None => [0.0; 2],
        };
        ((left, right), wet, captured)
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
    fn bend_capture_tap_is_the_wet_stem_before_master_processing() {
        let mut rack = SynthRack::new(48_000.0);
        let mut channels = DEFAULT_LANE_CHANNELS;
        channels[0].fx_send = 0.8;
        rack.set_lane_channels(channels);
        rack.reset_voices();
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.velocity_v[0] = 10.0;
        let inputs = CaptureSamples::default();
        let captured: std::vec::Vec<[f32; 2]> = (0..12_000)
            .map(|_| rack.render_session(&cv, &inputs, Some(CaptureTap::Bend)).1)
            .collect();
        assert!(captured.iter().any(|frame| frame[0].abs() > 1e-4));
        assert!(captured.iter().any(|frame| frame[1].abs() > 1e-4));
        rack.reset_voices();
        rack.reset_master(0.0);
        let mut stems = [[0.0; 2]; NUM_LANES];
        let mut delays: std::boxed::Box<[StereoDelay; NUM_LANES]> =
            core::iter::repeat_with(|| StereoDelay::new(48_000.0))
                .take(NUM_LANES)
                .collect::<std::vec::Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .ok()
                .unwrap();
        for expected in captured {
            let (_, wet) = rack.render_session_with_stems(&cv, &inputs, &mut stems, &mut delays);
            assert_eq!(wet, expected);
        }
    }

    #[test]
    fn byte_route_bypass_matches_direct_bbus_and_uses_the_latched_word_when_enabled() {
        let mut rack = SynthRack::new(48_000.0);
        rack.set_lane_patch(0, LanePatch::for_machine(MachineKind::ByteBusR2r));
        let mut cv = CvFrame {
            run_v: 10.0,
            ..CvFrame::default()
        };
        cv.gate_v[0] = 10.0;
        cv.velocity_v[0] = 10.0;
        cv.pitch_v[0] = 3.0;
        let inputs = CaptureSamples::default();
        let clock = |index: usize| index % 32 == 16;
        let word = |index: usize| (index as u8).wrapping_mul(31);
        let before = core::array::from_fn::<_, 512, _>(|index| {
            rack.render_stereo_with_bytebus(&cv, Some(word(index)))
        });
        rack.reset_voices();
        rack.reset_master(0.0);
        let after = core::array::from_fn::<_, 512, _>(|index| {
            cv.bytebus_v = leviathan_core::encode_bytebus_frame(word(index), clock(index));
            rack.render_session(&cv, &inputs, None).0
        });
        assert_eq!(before, after);
        assert_eq!(rack.routed_byte(), None);

        rack.reset_voices();
        rack.reset_master(0.0);
        let mut routing = RackRouting::default();
        routing.bytebus.enabled = true;
        routing.bytebus.reset_word = 0x77;
        assert!(rack.set_routing(routing));
        let latched = core::array::from_fn::<_, 512, _>(|index| {
            cv.bytebus_v = leviathan_core::encode_bytebus_frame(word(index), clock(index));
            rack.render_session(&cv, &inputs, None).0
        });
        assert_eq!(rack.routed_byte(), Some(word(496)));
        rack.reset_voices();
        rack.reset_master(0.0);
        assert!(rack.set_routing(RackRouting::default()));
        let mut held = 0x77;
        let direct = core::array::from_fn::<_, 512, _>(|index| {
            if clock(index) {
                held = word(index);
            }
            rack.render_stereo_with_bytebus(&cv, Some(held))
        });
        assert_eq!(latched, direct);
        assert_ne!(latched, before);
    }

    #[test]
    fn captured_sampler_keeps_the_current_note_across_slot_replacement() {
        let old_frames = [[8192, -8192]; 32];
        let new_frames = [[-8192, 8192]; 32];
        let old = CaptureSample {
            generation: 1,
            rate: 48_000,
            frames: &old_frames,
        };
        let new = CaptureSample {
            generation: 2,
            rate: 48_000,
            frames: &new_frames,
        };
        let mut rack = SynthRack::new(48_000.0);
        rack.set_lane_channels([LaneChannel::dry(1.0, 0.0); NUM_LANES]);
        rack.set_lane_patch(0, LanePatch::for_machine(MachineKind::VarSampler));
        let mut routing = RackRouting::default();
        routing.inputs[0] = LaneInput::Capture;
        assert!(rack.set_routing(routing));
        rack.reset_voices();
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.velocity_v[0] = 10.0;
        let tap = Some(CaptureTap::Lane(0));
        let mut samples = CaptureSamples {
            current: Some(old),
            ..CaptureSamples::default()
        };
        let (_, before) = rack.render_session(&cv, &samples, tap);
        samples.current = Some(new);
        samples.previous[0] = Some(old);
        let (_, held) = rack.render_session(&cv, &samples, tap);
        assert_eq!(rack.capture_generations()[0], Some(1));
        assert_eq!(before, held);
        cv.retrigger_mask = 1;
        let (_, after) = rack.render_session(&cv, &samples, tap);
        assert_eq!(rack.capture_generations()[0], Some(2));
        assert_eq!(after, [-before[0], -before[1]]);
    }

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
            assert_eq!(channel.fx_send, 0.0);
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
            fx_send: 1.0,
        };
        rack.set_lane_channels(target);
        rack.render_stereo(&CvFrame::default());
        let actual = rack.voices[0].channel;
        assert!((0.8..DEFAULT_LANE_CHANNELS[0].level).contains(&actual.level));
        assert!((DEFAULT_LANE_CHANNELS[0].pan..-0.65).contains(&actual.pan));
        assert!((0.0..0.01).contains(&actual.delay_send));
        assert!(actual.delay_send > 0.0);
        assert_eq!(actual.fx_send, actual.delay_send);
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
    fn calibration_narrows_factory_peak_spread() {
        let render_peaks = |enabled: bool| {
            let mut lows = f32::INFINITY;
            let mut highs = 0.0_f32;
            let mut rack = SynthRack::new(48_000.0);
            rack.set_lane_channels([LaneChannel::dry(0.8, 0.0); NUM_LANES]);
            rack.set_lane_audible_mask(1);
            rack.set_voice_calibration(enabled);
            for machine in MachineKind::ALL {
                let patch = if enabled {
                    LanePatch::for_machine_with_calibration(machine)
                } else {
                    LanePatch::for_machine(machine)
                };
                rack.set_lane_patch(0, patch);
                rack.reset_voices();
                rack.reset_master(0.0);
                let mut cv = CvFrame {
                    run_v: 10.0,
                    bytebus_v: leviathan_core::encode_bytebus_frame(1, false),
                    ..CvFrame::default()
                };
                cv.gate_v[0] = 10.0;
                cv.velocity_v[0] = 10.0;
                let mut peak = 0.0_f32;
                for i in 0..12_000 {
                    if i == 6_000 {
                        cv.gate_v[0] = 0.0;
                    }
                    let (left, right) = rack.render_stereo(&cv);
                    assert!(left.is_finite() && right.is_finite());
                    peak = peak.max(left.abs()).max(right.abs());
                }
                assert!(peak > 0.01, "{machine:?} {peak}");
                lows = lows.min(peak);
                highs = highs.max(peak);
            }
            highs / lows
        };
        let legacy = render_peaks(false);
        let calibrated = render_peaks(true);
        assert!(calibrated < 0.65 * legacy, "{legacy} -> {calibrated}");
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
        for (id, kind) in MachineKind::ALL.into_iter().enumerate() {
            assert_eq!(MachineKind::from_id(id), kind);
            assert_eq!(usize::from(kind.spec().id), id);
            assert_eq!(MachineKind::from_slug(kind.slug()), Some(kind));
            assert_eq!(kind.short_name(), kind.spec().badge);
            assert!(LanePatch::for_machine(kind).is_valid());
        }
        for (kind, id, slug, badge) in [
            (MachineKind::VastFunSynth, 9, "vast", "VAST"),
            (MachineKind::ElectroPerc, 10, "perc", "PERC"),
            (MachineKind::WestCoastLpg, 11, "west", "WEST"),
            (MachineKind::OtisTranswave, 12, "otis", "OTIS"),
            (MachineKind::ByteBusR2r, 13, "bbus", "BBUS"),
            (MachineKind::OtisTwin, 14, "otis2", "OT2"),
            (MachineKind::Fs77FormantFm, 15, "fs77", "FS77"),
            (MachineKind::VastTriple, 16, "k26", "K26"),
        ] {
            assert_eq!((kind.spec().id, kind.slug(), kind.short_name()), (id, slug, badge));
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
        assert!(MachineKind::is_step_lock_valid(-1));
        assert!(!MachineKind::is_step_lock_valid(-2));
        assert!(!MachineKind::is_step_lock_valid(MACHINE_COUNT as i16));
        assert_eq!(
            MachineKind::from_patch_and_lock(
                &patch,
                &SoundLock {
                    machine: i16::MAX,
                    ..SoundLock::default()
                },
            ),
            patch.machine
        );
    }

    #[test]
    fn rack_uses_clocked_zero_until_the_next_publication() {
        let mut rack = SynthRack::new(48_000.0);
        rack.set_lane_patch(0, LanePatch::for_machine(MachineKind::ByteBusR2r));
        let mut cv = CvFrame::default();
        cv.gate_v[0] = 10.0;
        cv.velocity_v[0] = 10.0;
        let render = |rack: &mut SynthRack, unpublished| {
            rack.reset_voices();
            rack.reset_master(0.0);
            let mut receiver = ByteBusReceiver::default();
            let mut samples = [0.0; 256];
            for (i, sample) in samples.iter_mut().enumerate() {
                let byte = if i < 128 { 0 } else { unpublished };
                let raw = leviathan_core::encode_bytebus_frame(byte, i == 0);
                let word = receiver.step(ByteBusSource::Clocked, &raw);
                *sample = rack.render_stereo_with_bytebus(&cv, word).0;
            }
            samples
        };
        let held = render(&mut rack, 0);
        assert!(held.iter().any(|sample| sample.abs() > 0.03));
        assert_eq!(render(&mut rack, 0xf7), held);
    }

    #[test]
    fn expansion_lanes_render_independently_and_keep_release_until_next_gate() {
        let mut rack = SynthRack::new(48_000.0);
        let engines = [
            MachineKind::CzPhaseDist,
            MachineKind::PolivoksSub,
            MachineKind::VastFunSynth,
            MachineKind::ElectroPerc,
            MachineKind::WestCoastLpg,
            MachineKind::OtisTranswave,
        ];
        for (lane, engine) in engines.into_iter().enumerate() {
            rack.set_lane_patch(lane, LanePatch::for_machine(engine));
        }
        let mut stems = [[0.0; 2]; NUM_LANES];
        rack.render_stereo_with_stems(&CvFrame::default(), &mut stems);
        assert_eq!(stems, [[0.0; 2]; NUM_LANES]);

        let mut cv = CvFrame::default();
        for lane in 0..engines.len() {
            cv.gate_v[lane] = 10.0;
            cv.velocity_v[lane] = 10.0;
        }
        let mut energy = [0.0_f32; NUM_LANES];
        for _ in 0..1024 {
            rack.render_stereo_with_stems(&cv, &mut stems);
            for (sum, stem) in energy.iter_mut().zip(stems) {
                assert!(stem.iter().all(|sample| sample.is_finite()));
                *sum += stem[0].abs() + stem[1].abs();
            }
        }
        assert!(energy.iter().all(|&sum| sum > 0.1));
        assert_eq!(
            rack.voices[0].active_machine,
            Some(MachineKind::CzPhaseDist)
        );
        assert_eq!(
            rack.voices[2].active_machine,
            Some(MachineKind::VastFunSynth)
        );

        cv.gate_v[0] = 0.0;
        cv.locks[0].machine = MachineKind::PolivoksSub as i16;
        rack.render_stereo_with_stems(&cv, &mut stems);
        assert_eq!(
            rack.voices[0].active_machine,
            Some(MachineKind::CzPhaseDist)
        );
        assert!(stems[0][0].abs() > 1e-5);
        cv.gate_v[0] = 10.0;
        rack.render_stereo_with_stems(&cv, &mut stems);
        assert_eq!(
            rack.voices[0].active_machine,
            Some(MachineKind::PolivoksSub)
        );

        rack.set_lane_patch(0, LanePatch::for_machine(MachineKind::VastFunSynth));
        cv.locks[0] = SoundLock::default();
        cv.gate_v[0] = 0.0;
        rack.render_stereo_with_stems(&cv, &mut stems);
        assert_eq!(stems[0], [0.0; 2]);
        assert_eq!(
            rack.voices[2].active_machine,
            Some(MachineKind::VastFunSynth)
        );
    }

    #[test]
    fn cz_and_electro_perc_render_every_tracker_ratchet() {
        let mut pattern = Pattern::default();
        pattern.lanes[0][0] = NoteCell {
            active: true,
            midi: 69,
            velocity: 127,
            gate_fraction: 0.8,
            retrig: 4,
            ..NoteCell::default()
        };
        let mut evaluator = TrackerEvaluator::new(EngineConfig {
            bpm: 120.0,
            ..EngineConfig::default()
        });
        evaluator.playing = true;
        let mut rack = SynthRack::new(48_000.0);
        let mut patch = LanePatch::for_machine(MachineKind::CzPhaseDist);
        patch.params[3] = 0.0;
        rack.set_lane_patch(0, patch);
        pattern.lanes[1][0] = pattern.lanes[0][0];
        rack.set_lane_patch(1, LanePatch::for_machine(MachineKind::ElectroPerc));
        let mut peaks = [[0.0_f32; 2]; 4];
        let mut stems = [[0.0; 2]; NUM_LANES];
        let mut attacks = 0;
        for _ in 0..6000 {
            let cv = evaluator.step_sample(&pattern);
            if cv.retrigger_mask & 1 != 0 {
                attacks += 1;
            }
            assert_eq!(cv.retrigger_mask & 1, (cv.retrigger_mask >> 1) & 1);
            rack.render_stereo_with_stems(&cv, &mut stems);
            if attacks > 0 {
                for (peak, [left, right]) in peaks[attacks - 1].iter_mut().zip(stems) {
                    *peak = peak.max(left.abs()).max(right.abs());
                }
            }
        }
        assert_eq!(attacks, 4);
        assert!(peaks.into_iter().flatten().all(|peak| peak > 0.04), "{peaks:?}");
    }

    #[test]
    fn pitch_conversion_stays_within_a_tenth_of_a_cent() {
        for semitone in -60..=60 {
            let volts = semitone as f32 / 12.0;
            let want = 261.625_58_f64 * 2f64.powf(f64::from(semitone) / 12.0);
            let cents = 1200.0 * (f64::from(v_oct_to_hz(volts)) / want).log2();
            assert!(cents.abs() < 0.1, "semitone {semitone}: {cents:.3} cents");
        }
    }
}
