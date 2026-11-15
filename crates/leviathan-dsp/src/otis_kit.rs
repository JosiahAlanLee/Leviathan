//! Eight-slot one-shot player. Sample storage and generations belong to the host.

use leviathan_core::SOUND_PARAMS;

pub const SLOTS: usize = 8;
pub const MAX_FRAMES: usize = 1_048_320;
pub const CHIP_RATES: [f32; 3] = [
    10_000_000.0 / 336.0,
    10_000_000.0 / 224.0,
    10_000_000.0 / 128.0,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lane {
    pub bytebus: bool,
    pub clock: u8,
    pub drop_samples: bool,
    pub velocity: VelocityCurve,
    /// Hold each chip sample to the host rate (stair-step, with images above
    /// the chip's Nyquist) instead of band-limiting it. Kits saved before
    /// reconstruction existed load with this on.
    pub raw_output: bool,
    /// The slot filter knob's taper. Kits saved before load as Squared.
    pub filter_taper: FilterTaper,
}

/// Slot filter knob to the two chip-rate lowpass coefficients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterTaper {
    /// Linear in the knob, as the ES5505 K1 register (OtisLab Otis Kit).
    #[default]
    Linear,
    /// The original squared knob: about half the cutoff at the same setting.
    Squared,
}

impl FilterTaper {
    fn coefficient(self, filter: f32) -> f32 {
        match self {
            Self::Linear => 0.015 + 0.985 * filter,
            Self::Squared => 0.015 + 0.985 * filter * filter,
        }
    }
}

const TAPS: usize = 32;
const PHASES: usize = 64;
type Kernel = [[f32; TAPS]; PHASES + 1];

// These cutoffs are fractions of the chip's Nyquist. The downsampler
// selects a band at or below 0.85 of the host's Nyquist for transition room.
const DOWN_CUTOFFS: [f32; 16] = [
    0.075, 0.125, 0.1875, 0.25, 0.3125, 0.375, 0.4375, 0.475, 0.5, 0.55, 0.60, 0.65,
    0.70, 0.75, 0.80, 0.825,
];
static UPSAMPLE: Kernel = make_kernel(0.9);
static DOWNSAMPLE: [Kernel; DOWN_CUTOFFS.len()] = downsample_kernels();

const fn downsample_kernels() -> [Kernel; DOWN_CUTOFFS.len()] {
    let mut banks = [[[0.0; TAPS]; PHASES + 1]; DOWN_CUTOFFS.len()];
    let mut i = 0;
    while i < DOWN_CUTOFFS.len() {
        banks[i] = make_kernel(DOWN_CUTOFFS[i]);
        i += 1;
    }
    banks
}

fn output_kernel(chip: f32, host: f32) -> &'static Kernel {
    if chip < host {
        &UPSAMPLE
    } else {
        let cutoff = 0.85 * (host / chip);
        let band = DOWN_CUTOFFS.iter().rposition(|&f| f <= cutoff).unwrap_or(0);
        &DOWNSAMPLE[band]
    }
}

const fn sine_turns(turns: f32) -> f32 {
    let wrapped = turns - (turns as i32) as f32;
    let wrapped = if wrapped < 0.0 { wrapped + 1.0 } else { wrapped };
    crate::voice::sine_cycle(wrapped)
}

// Blackman-windowed sinc with fractional phases normalized to unity at DC.
const fn make_kernel(cutoff: f32) -> Kernel {
    let mut table = [[0.0; TAPS]; PHASES + 1];
    let mut p = 0;
    while p <= PHASES {
        let fraction = p as f32 / PHASES as f32;
        let mut sum = 0.0;
        let mut k = 0;
        while k < TAPS {
            // Distance from the output instant (half the kernel behind the
            // newest, plus the fraction) to the tap k samples back.
            let d = k as f32 - (TAPS / 2) as f32 + fraction;
            let x = cutoff * d;
            let sinc = if x == 0.0 || (x > -1.0e-6 && x < 1.0e-6) {
                1.0
            } else {
                sine_turns(0.5 * x) / (core::f32::consts::PI * x)
            };
            let w = d / (TAPS / 2) as f32;
            let window = if w <= -1.0 || w >= 1.0 {
                0.0
            } else {
                0.42 + 0.5 * sine_turns(0.5 * w + 0.25) + 0.08 * sine_turns(w + 0.25)
            };
            table[p][k] = sinc * window;
            sum += sinc * window;
            k += 1;
        }
        k = 0;
        while k < TAPS {
            table[p][k] /= sum;
            k += 1;
        }
        p += 1;
    }
    table
}

/// How MIDI velocity maps to hit gain. Both curves keep 192 quantized steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VelocityCurve {
    /// 0.375 dB per step, 72 dB of range: velocity 94 is -18.75 dB.
    #[default]
    Chip,
    /// 0.125 dB per step, about 24 dB of range: velocity 94 is -6.25 dB.
    Soft,
}

impl VelocityCurve {
    pub const ALL: [Self; 2] = [Self::Chip, Self::Soft];

    pub const fn step_db(self) -> f32 {
        match self {
            Self::Chip => 0.375,
            Self::Soft => 0.125,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Chip => "Chip (72 dB)",
            Self::Soft => "Soft (24 dB)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub tune: f32,
    pub decay: f32,
    pub level: f32,
    pub filter: f32,
    pub start: f32,
    pub reverse: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            tune: 0.0,
            decay: 1.0,
            level: 1.0,
            filter: 1.0,
            start: 0.0,
            reverse: false,
        }
    }
}

impl Slot {
    pub fn is_valid(self) -> bool {
        (-24.0..=24.0).contains(&self.tune)
            && [self.decay, self.level, self.filter, self.start]
                .iter()
                .all(|v| (0.0..=1.0).contains(v))
    }
}

#[derive(Clone, Copy)]
pub struct Sample<'a> {
    pub generation: u64,
    pub rate: u32,
    pub pcm: &'a [i16],
}

pub trait Memory {
    fn current(&self, slot: usize) -> Option<Sample<'_>>;
    fn previous(&self, lane: usize, slot: usize, generation: u64) -> Option<Sample<'_>>;
}

#[derive(Clone, Copy, Debug, Default)]
struct Hit {
    generation: Option<u64>,
    position: u64,
    delta: u64,
    start: u64,
    end: u64,
    reverse: bool,
    drop_samples: bool,
    phase: f32,
    ratio: f32,
    filtered: [f32; 2],
    cutoff: f32,
    amp: f32,
    decay: f32,
    held: f32,
    kernel: Option<&'static Kernel>,
    history: [f32; TAPS],
    head: u8,
    /// Chip samples of silence still to pass through the reconstruction.
    tail: u8,
}

const ONE: u64 = 1 << 32;
const PCM_GAIN: f32 = 1.0 / 32768.0;

fn exp2(octaves: f32) -> f32 {
    let part = crate::v_oct_to_hz(octaves / 3.0) / 261.625_58;
    part * part * part
}

/// Relative gain in dB for a normalized hit velocity on the chip curve.
/// Silence has no finite gain.
#[inline]
pub fn velocity_gain_db(velocity: f32) -> Option<f32> {
    velocity_gain_db_with(velocity, VelocityCurve::Chip)
}

/// Relative gain in dB for a normalized hit velocity on `curve`.
#[inline]
pub fn velocity_gain_db_with(velocity: f32, curve: VelocityCurve) -> Option<f32> {
    if !velocity.is_finite() || velocity <= 0.0 {
        None
    } else {
        let steps = (192.0 * (1.0 - velocity.clamp(0.0, 1.0)) + 0.5) as u32;
        Some(-curve.step_db() * steps as f32)
    }
}

#[cfg(test)]
fn log_velocity(velocity: f32) -> f32 {
    log_velocity_with(velocity, VelocityCurve::Chip)
}

fn log_velocity_with(velocity: f32, curve: VelocityCurve) -> f32 {
    velocity_gain_db_with(velocity, curve).map_or(0.0, |db| exp2(db / 6.020_6))
}

impl Hit {
    fn new(
        sample: Sample<'_>,
        params: Slot,
        pitch_oct: f32,
        velocity: f32,
        clock: u8,
        settings: Lane,
        rate: f32,
    ) -> Self {
        let Lane { drop_samples, velocity: curve, .. } = settings;
        let chip = CHIP_RATES[usize::from(clock).min(CHIP_RATES.len() - 1)];
        let end_frame = sample.pcm.len() - 1;
        let start_frame = ((end_frame as f32 * params.start) as usize).min(end_frame - 1);
        let start = start_frame as u64 * ONE;
        let end = sample.pcm.len() as u64 * ONE;
        let amp = log_velocity_with(velocity, curve) * params.level * params.level;
        let seconds = 0.02 * exp2(8.965_784 * params.decay);
        let decay = if params.decay >= 1.0 {
            1.0
        } else {
            exp2(-9.965_784 / (chip * seconds))
        };
        let delta = f64::from(exp2((pitch_oct + params.tune / 12.0).clamp(-12.0, 12.0)))
            * f64::from(sample.rate)
            / f64::from(chip);
        Self {
            generation: (amp > 0.0).then_some(sample.generation),
            position: if params.reverse { end - ONE } else { start },
            delta: ((delta * ONE as f64) as u64).max(1),
            start,
            end,
            reverse: params.reverse,
            drop_samples,
            phase: 1.0,
            ratio: chip / rate,
            filtered: [0.0; 2],
            cutoff: settings.filter_taper.coefficient(params.filter),
            amp,
            decay,
            held: 0.0,
            kernel: (!settings.raw_output && chip != rate).then(|| output_kernel(chip, rate)),
            history: [0.0; TAPS],
            head: 0,
            tail: TAPS as u8,
        }
    }

    fn render(&mut self, samples: &dyn Memory, lane: usize, slot: usize) -> f32 {
        let Some(generation) = self.generation else {
            return 0.0;
        };
        let Some(sample) = samples
            .current(slot)
            .filter(|s| s.generation == generation)
            .or_else(|| samples.previous(lane, slot, generation))
        else {
            self.generation = None;
            return 0.0;
        };
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            if self.amp == 0.0 {
                if self.kernel.is_none() || self.tail == 0 {
                    self.generation = None;
                    return 0.0;
                }
                // Clock the filter's tail, including extra ticks in the
                // host frame where a fast chip played its last sample.
                self.push(0.0);
                self.tail -= 1;
                continue;
            }
            let index = (self.position >> 32) as usize;
            let a = i32::from(sample.pcm[index]);
            let value = if self.drop_samples {
                a
            } else {
                let b = i32::from(sample.pcm[(index + 1).min((self.end >> 32) as usize - 1)]);
                let fraction = ((self.position as u32) >> (32 - 9)) as i32;
                a + ((b - a) * fraction >> 9)
            };
            self.filtered[0] += self.cutoff * (value as f32 * PCM_GAIN - self.filtered[0]);
            self.filtered[1] += self.cutoff * (self.filtered[0] - self.filtered[1]);
            self.held = self.filtered[1] * self.amp;
            if self.kernel.is_some() {
                self.push(self.held);
            }
            self.amp *= self.decay;
            let next = if self.reverse {
                self.position.checked_sub(self.delta)
            } else {
                self.position.checked_add(self.delta)
            };
            if let Some(next) =
                next.filter(|p| (*p >= self.start && *p < self.end) && self.amp >= 0.0001)
            {
                self.position = next;
            } else {
                self.amp = 0.0;
                if self.kernel.is_none() {
                    // Raw output keeps the last held sample for this frame.
                    break;
                }
            }
        }
        let out = self.kernel.map_or(self.held, |kernel| self.reconstruct(kernel));
        self.phase += self.ratio;
        out
    }

    #[inline]
    fn push(&mut self, value: f32) {
        self.head = (self.head + 1) % TAPS as u8;
        self.history[usize::from(self.head)] = value;
    }

    /// The output `phase` chip samples after the newest, delayed by TAPS / 2.
    #[inline]
    fn reconstruct(&self, kernel: &Kernel) -> f32 {
        let position = self.phase.clamp(0.0, 1.0) * PHASES as f32;
        let row = (position as usize).min(PHASES - 1);
        let mix = position - row as f32;
        let mut out = 0.0;
        for k in 0..TAPS {
            let tap = kernel[row][k] + mix * (kernel[row + 1][k] - kernel[row][k]);
            let index = (usize::from(self.head) + TAPS - k) % TAPS;
            out += tap * self.history[index];
        }
        out
    }
}

#[derive(Clone, Debug, Default)]
struct Player {
    hit: Hit,
    old: Hit,
    fade_left: u16,
    fade_len: u16,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    slots: [Player; SLOTS],
}

impl Voice {
    pub(super) fn sounding(&self) -> u8 {
        self.slots
            .iter()
            .enumerate()
            .fold(0, |mask, (slot, state)| {
                mask | (u8::from(state.hit.generation.is_some() || state.old.generation.is_some())
                    << slot)
            })
    }

    pub(super) fn generations(&self) -> [[Option<u64>; 2]; SLOTS] {
        self.slots
            .each_ref()
            .map(|s| [s.hit.generation, s.old.generation])
    }

    pub(super) fn step(
        &mut self,
        lane: usize,
        sample_rate: f32,
        memory: &dyn Memory,
        slots: &[Slot; SLOTS],
        settings: Lane,
        pitch: f32,
        trigger: Option<f32>,
        offset: &[f32; SOUND_PARAMS],
        bus: Option<u8>,
    ) -> f32 {
        if let Some(velocity) = trigger {
            let (slot, pitch) = addressing(
                pitch,
                offset[0],
                settings.bytebus.then_some(bus.unwrap_or(0)),
            );
            if let Some(sample) = memory
                .current(slot)
                .filter(|s| s.pcm.len() >= 2 && s.rate != 0)
            {
                let mut params = slots[slot];
                params.tune = (params.tune + 48.0 * (offset[1] - 0.5)).clamp(-24.0, 24.0);
                for (value, shift) in [
                    &mut params.decay,
                    &mut params.filter,
                    &mut params.start,
                    &mut params.level,
                ]
                .into_iter()
                .zip([offset[2], offset[3], offset[4], offset[7]])
                {
                    *value = (*value + 2.0 * (shift - 0.5)).clamp(0.0, 1.0);
                }
                params.reverse ^= offset[5] >= 0.75;
                let clock = (i16::from(settings.clock) + i16::from(clock_offset(offset[6])))
                    .clamp(0, 2) as u8;
                let player = &mut self.slots[slot];
                let next = Hit::new(
                    sample,
                    params,
                    pitch,
                    velocity,
                    clock,
                    settings,
                    sample_rate,
                );
                if next.generation.is_some() {
                    if player.hit.generation.is_some() {
                        player.old = player.hit;
                        player.fade_len = (0.0015 * sample_rate + 0.5) as u16;
                        player.fade_left = player.fade_len;
                    }
                    player.hit = next;
                }
            }
        }
        self.slots
            .iter_mut()
            .enumerate()
            .map(|(slot, voice)| {
                let current = voice.hit.render(memory, lane, slot);
                if voice.fade_left == 0 {
                    return current;
                }
                let old = voice.old.render(memory, lane, slot);
                let blend = f32::from(voice.fade_left) / f32::from(voice.fade_len);
                voice.fade_left -= 1;
                if voice.fade_left == 0 {
                    voice.old = Hit::default();
                }
                current + blend * (old - current)
            })
            .sum()
    }
}

fn clock_offset(value: f32) -> i8 {
    if value < 0.25 {
        -1
    } else if value >= 0.75 {
        1
    } else {
        0
    }
}

fn addressing(pitch: f32, offset: f32, byte: Option<u8>) -> (usize, f32) {
    let pitch = if pitch.is_finite() {
        pitch.clamp(-10.0, 10.0)
    } else {
        0.0
    };
    let semitones = (pitch * 12.0 + 0.5 * pitch.signum()) as i32;
    let shift = (8.0 * (offset - 0.5)).clamp(-4.0, 4.0);
    let shift = (shift + 0.5 * shift.signum()) as i32;
    match byte {
        Some(word) => (
            ((i32::from(word & 7) + shift).rem_euclid(8)) as usize,
            pitch,
        ),
        None => (
            ((semitones + shift).rem_euclid(8)) as usize,
            semitones.div_euclid(8) as f32,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_readout_matches_the_existing_hit_gain() {
        for midi in 0..=127 {
            let velocity = f32::from(midi as u8) / 127.0;
            let steps = (192.0 * (1.0 - velocity) + 0.5) as u32;
            let before = if midi == 0 {
                0.0
            } else {
                exp2(-0.375 * steps as f32 / 6.020_6)
            };
            assert_eq!(log_velocity(velocity).to_bits(), before.to_bits());
        }
        assert_eq!(velocity_gain_db(0.0), None);
        assert_eq!(velocity_gain_db(1.0), Some(0.0));
        assert_eq!(velocity_gain_db(110.0 / 127.0), Some(-9.75));
        assert_eq!(velocity_gain_db(94.0 / 127.0), Some(-18.75));
        let soft = |midi: f32| velocity_gain_db_with(midi / 127.0, VelocityCurve::Soft);
        assert_eq!(soft(0.0), None);
        assert_eq!(soft(127.0), Some(0.0));
        assert_eq!(soft(110.0), Some(-3.25));
        assert_eq!(soft(94.0), Some(-6.25));
        assert_eq!(soft(1.0), Some(-23.75));
        for midi in 1..=127 {
            let v = f32::from(midi as u8) / 127.0;
            assert_eq!(
                velocity_gain_db_with(v, VelocityCurve::Soft).map(|db| db * 3.0),
                velocity_gain_db(v)
            );
        }
        let gain_delta = log_velocity(94.0 / 127.0) / log_velocity(110.0 / 127.0);
        assert!((20.0 * f64::from(gain_delta).log10() + 9.0).abs() < 0.01);
    }

    struct Bank<'a> {
        current: Sample<'a>,
        old: Option<Sample<'a>>,
    }
    impl Memory for Bank<'_> {
        fn current(&self, _slot: usize) -> Option<Sample<'_>> {
            Some(self.current)
        }
        fn previous(&self, _lane: usize, _slot: usize, id: u64) -> Option<Sample<'_>> {
            self.old.filter(|old| old.generation == id)
        }
    }

    #[test]
    fn captured_source_rate_is_compensated_and_two_frame_reverse_is_bounded() {
        for rate in [29_762, 44_100, 48_000, 96_000] {
            let bank = Bank {
                current: Sample {
                    generation: 1,
                    rate,
                    pcm: &[12_000, -12_000],
                },
                old: None,
            };
            let output_rate = CHIP_RATES[0];
            let mut hit = Hit::new(
                bank.current,
                Slot {
                    start: 1.,
                    reverse: true,
                    ..Slot::default()
                },
                0.0,
                1.0,
                0,
                Lane { raw_output: true, filter_taper: FilterTaper::Squared, ..Lane::default() },
                output_rate,
            );
            assert!(
                (hit.delta as f64 / ONE as f64 - rate as f64 / output_rate as f64).abs() < 1e-6
            );
            assert!(hit.render(&bank, 0, 0) < -0.3);
            for _ in 0..4 {
                assert!(hit.render(&bank, 0, 0).abs() < 0.4);
            }
            assert_eq!(hit.generation, None);
        }
    }

    #[test]
    fn eight_slots_overlap_and_retrigger_fades_keep_the_old_generation() {
        let mut voice = Voice::default();
        let mut bank = Bank {
            current: Sample {
                generation: 1,
                rate: 48_000,
                pcm: &[3200; 512],
            },
            old: None,
        };
        let config = Lane {
            bytebus: true,
            ..Lane::default()
        };
        let slots = [Slot::default(); SLOTS];
        for word in 0..8 {
            voice.step(
                0,
                48_000.,
                &bank,
                &slots,
                config,
                0.0,
                Some(1.0),
                &[0.5; 8],
                Some(word),
            );
        }
        assert!(voice.generations().iter().all(|pair| pair[0] == Some(1)));
        bank.old = Some(bank.current);
        bank.current = Sample {
            generation: 2,
            rate: 48_000,
            pcm: &[-3200; 512],
        };
        voice.step(
            0,
            48_000.,
            &bank,
            &slots,
            config,
            0.0,
            Some(1.0),
            &[0.5; 8],
            Some(0),
        );
        assert_eq!(voice.generations()[0], [Some(2), Some(1)]);
        for _ in 0..72 {
            voice.step(
                0,
                48_000.,
                &bank,
                &slots,
                config,
                0.0,
                None,
                &[0.5; 8],
                Some(0),
            );
        }
        assert_eq!(voice.generations()[0], [Some(2), None]);
        assert_eq!(voice.generations()[7], [Some(1), None]);
    }

    #[test]
    fn reconstruction_removes_chip_images_and_raw_output_keeps_them() {
        // A 10 kHz tone at 48 kHz played on the 29.8 kHz clock: its first
        // image sits at about 19.76 kHz.
        let pcm: std::vec::Vec<i16> = (0..9_600)
            .map(|i| (16_000.0 * crate::voice::sine_cycle(((i * 10_000) % 48_000) as f32 / 48_000.0)) as i16)
            .collect();
        let bank = Bank { current: Sample { generation: 1, rate: 48_000, pcm: &pcm }, old: None };
        let level = |raw: bool, hz: f64| {
            let lane = Lane { raw_output: raw, ..Lane::default() };
            let mut hit = Hit::new(bank.current, Slot::default(), 0.0, 1.0, 0, lane, 48_000.0);
            let out: std::vec::Vec<f32> = (0..6_000).map(|_| hit.render(&bank, 0, 0)).collect();
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &x) in out[1_000..5_800].iter().enumerate() {
                let w = 2.0 * core::f64::consts::PI * hz * n as f64 / 48_000.0;
                re += f64::from(x) * w.cos();
                im += f64::from(x) * w.sin();
            }
            (re * re + im * im).sqrt()
        };
        let image = CHIP_RATES[0] as f64 - 10_000.0;
        let raw = 20.0 * (level(true, image) / level(true, 10_000.0)).log10();
        let smooth = 20.0 * (level(false, image) / level(false, 10_000.0)).log10();
        assert!(raw > -20.0, "raw image {raw:.1} dB");
        assert!(smooth < -60.0, "reconstructed image {smooth:.1} dB");
        for row in UPSAMPLE.iter() {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn faster_chip_clocks_filter_before_downsampling_to_the_host() {
        for (rate, clock, tone) in [(48_000, 2, 30_000.0), (44_100, 1, 22_200.0)] {
            let measure = |hz: f64, raw_output| {
                let pcm = (0..28_800)
                    .map(|n| {
                        let phase = core::f64::consts::TAU * hz * n as f64 / 96_000.0;
                        (24_000.0 * phase.sin()) as i16
                    })
                    .collect::<std::vec::Vec<_>>();
                let bank = Bank {
                    current: Sample { generation: 1, rate: 96_000, pcm: &pcm },
                    old: None,
                };
                let lane = Lane { raw_output, ..Lane::default() };
                let mut hit = Hit::new(bank.current, Slot::default(), 0.0, 1.0, clock, lane, rate as f32);
                let observed_hz = hz.min(f64::from(rate) - hz);
                let count = rate / 10;
                let mut re = 0.0;
                let mut im = 0.0;
                for n in 0..count * 2 {
                    let sample = f64::from(hit.render(&bank, 0, 0));
                    if n >= count {
                        let phase = core::f64::consts::TAU * observed_hz * f64::from(n) / f64::from(rate);
                        re += sample * phase.cos();
                        im += sample * phase.sin();
                    }
                }
                2.0 * re.hypot(im) / f64::from(count)
            };
            let raw_alias = measure(tone, true);
            let clean_alias = measure(tone, false);
            assert!(raw_alias > 0.1);
            assert!(clean_alias < 0.01 * raw_alias, "{rate} Hz alias {clean_alias}");
            let passband_gain = measure(2_000.0, false) / measure(2_000.0, true);
            assert!((0.95..=1.05).contains(&passband_gain), "{rate} Hz gain {passband_gain}");
        }
    }

    #[test]
    fn reconstruction_drains_after_the_last_tick_of_a_short_sample() {
        let bank = Bank {
            current: Sample { generation: 1, rate: 96_000, pcm: &[10_000, -20_000, 10_000] },
            old: None,
        };
        let mut hit = Hit::new(bank.current, Slot::default(), 0.0, 1.0, 2, Lane::default(), 44_100.0);
        let mut peak = 0.0_f32;
        for _ in 0..48 {
            peak = peak.max(hit.render(&bank, 0, 0).abs());
        }
        assert!(peak > 0.01);
        assert!(hit.generation.is_none());
        assert_eq!(hit.render(&bank, 0, 0), 0.0);
    }
}
