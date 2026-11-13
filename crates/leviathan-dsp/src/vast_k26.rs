//! K26's three-stage voice uses a source and prefilter, a nonlinear block, and a postfilter.

use crate::otis::sample_rom;
use crate::voice::{approach, choice, cosine, phase_step, saw, sine, StateVariable};
use crate::{fast_tanh, wrap_turns};
use core::f32::consts::PI;
use leviathan_core::SOUND_PARAMS;

const SOURCE_COUNT: usize = 4;
const BLOCK_COUNT: usize = 4;
const FUN_COUNT: usize = 6;
const FUN_RATE_HZ: f32 = 500.0;
const BLOCK_SLEW_SECONDS: f32 = 0.005;
const PAD_GAIN: f32 = 0.75;

#[derive(Clone, Debug, Default)]
struct FunPair {
    output: [f32; 2],
    a_high: [bool; 2],
}

impl FunPair {
    fn step(&mut self, index: usize, equation: usize, a: f32, b: f32) -> f32 {
        let a = a.clamp(0.0, 1.0);
        let b = b.clamp(0.0, 1.0);
        let a_high = a > 0.5;
        let rising = a_high && !self.a_high[index];
        self.a_high[index] = a_high;
        let y = self.output[index];
        let next = match equation {
            0 => {
                let u = (0.5 * (y + 1.0)).clamp(0.02, 0.98);
                let rate = 2.65 + 1.30 * (0.55 * a + 0.45 * b);
                2.0 * rate * u * (1.0 - u) - 1.0
            }
            1 => sine(0.55 * a + 0.35 * b + 0.38 * y),
            2 => {
                let steps = (2.0 + 14.0 * a) as u32 as f32;
                let quantized = (b * steps) as u32 as f32 / steps;
                2.0 * quantized - 1.0
            }
            3 => {
                let rectified = (a - 0.65 * b - 0.08).max(0.0);
                0.72 * y + 1.0725 * rectified - 0.25
            }
            4 if rising => 2.0 * b - 1.0,
            4 => y,
            _ => {
                let target = 2.0 * b - 1.0;
                let limit = (0.35 + 18.0 * a * a) / FUN_RATE_HZ;
                y + (target - y).clamp(-limit, limit)
            }
        }
        .clamp(-1.0, 1.0);
        self.output[index] = next;
        next
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    phase: f32,
    aux_phase: f32,
    lfo_phase: f32,
    amp_env: f32,
    env2: f32,
    fun: FunPair,
    fun_clock: f32,
    fun2_slewed: f32,
    block_mix: [f32; BLOCK_COUNT],
    block_drive: [f32; BLOCK_COUNT],
    shaped_slewed: f32,
    pre: StateVariable,
    ring_allpass: StateVariable,
    post: [StateVariable; 2],
    phaser: [StateVariable; 2],
    dc_low: f32,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 128);

impl Voice {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        gate: bool,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
    ) -> f32 {
        let rate = sample_rate.max(8_000.0);
        let hz = hz.clamp(16.0, 0.44 * rate);
        let [decay, source_key, pre_tilt, triple_alg, fun_eq, fun_depth, post_cut, post_reso] =
            *params;
        let block = choice(triple_alg, BLOCK_COUNT);
        let cold_start = self.amp_env == 0.0 && trigger.is_some();
        if let Some(velocity) = trigger {
            self.amp_env = velocity;
            self.env2 = 1.0;
            if cold_start {
                self.aux_phase = 0.25;
                self.fun.output = [0.23, -0.17];
                self.fun_clock = 1.0;
            }
        }
        let amp_seconds = if gate {
            0.06 + 3.10 * decay * decay
        } else {
            0.014 + 0.85 * decay * decay
        };
        self.amp_env = approach(self.amp_env, 0.0, amp_seconds, rate);
        self.env2 = approach(self.env2, 0.0, 0.022 + 1.15 * decay, rate);
        if self.amp_env < 1e-6 {
            if self.amp_env != 0.0 || trigger.is_some() {
                self.reset();
            }
            return 0.0;
        }

        self.update_fun(fun_eq, fun_depth, rate);
        let fun1 = self.fun.output[0] * fun_depth;
        let fun2 = self.fun2_slewed * fun_depth;
        let source = choice(source_key, SOURCE_COUNT);
        let source_position = source_key * SOURCE_COUNT as f32 - source as f32;
        let timbre = (source_position + 0.35 * fun1).clamp(0.0, 1.0);
        let (exciter, auxiliary) = self.exciter(source, source_position, timbre, hz, rate);
        let (padded, low) = self.pre_filter(exciter, pre_tilt, rate);

        let block_position = triple_alg * BLOCK_COUNT as f32 - block as f32;
        let drive = (0.22 + 0.78 * block_position + 0.42 * fun1).clamp(0.05, 1.0);
        self.block_drive[block] = if self.block_mix[block] == 0.0 {
            drive
        } else {
            approach(self.block_drive[block], drive, BLOCK_SLEW_SECONDS, rate)
        };
        if cold_start {
            self.block_mix[block] = 1.0;
        }
        let modulation = 0.5 + 0.5 * fun2;
        let shaped = self.shape(padded, low, auxiliary, block, modulation, rate);
        self.shaped_slewed = approach(self.shaped_slewed, shaped, 0.000_045, rate);
        self.dc_low += (65.0 / rate) * (self.shaped_slewed - self.dc_low);
        let dc_clean = self.shaped_slewed - self.dc_low;

        let cutoff = (post_cut + 0.28 * fun2 + 0.16 * self.env2).clamp(0.04, 1.0);
        let output = self.post_filter(dc_clean, cutoff, post_reso, modulation, rate);
        fast_tanh(output * 1.18) * self.amp_env
    }

    fn update_fun(&mut self, equation: f32, depth: f32, rate: f32) {
        let hz = 0.08 + 16.0 * depth * depth;
        self.lfo_phase = wrap_turns(self.lfo_phase + hz / rate);
        self.fun_clock += FUN_RATE_HZ / rate;
        if self.fun_clock >= 1.0 {
            self.fun_clock -= 1.0;
            let equation = choice(equation, FUN_COUNT);
            let a = self
                .fun
                .step(0, equation, self.env2, 0.5 + 0.5 * sine(self.lfo_phase));
            self.fun.step(
                1,
                (equation + 1) % FUN_COUNT,
                0.5 + 0.5 * a,
                0.5 + 0.5 * sine(self.lfo_phase + 0.375),
            );
        }
        self.fun2_slewed = approach(self.fun2_slewed, self.fun.output[1], 0.002, rate);
    }

    fn exciter(
        &mut self,
        source: usize,
        position: f32,
        timbre: f32,
        hz: f32,
        rate: f32,
    ) -> (f32, f32) {
        let previous = self.phase;
        let step = phase_step(hz, rate);
        self.phase = wrap_turns(self.phase + step);
        match source {
            0 => {
                let detuned = phase_step(hz * (1.003 + 0.015 * timbre), rate);
                self.aux_phase = wrap_turns(self.aux_phase + detuned);
                let auxiliary = saw(self.aux_phase, detuned);
                ((saw(self.phase, step) + 0.82 * auxiliary) * 0.58, auxiliary)
            }
            1 => {
                let slave_step = phase_step(hz * (1.25 + 5.25 * timbre), rate);
                self.aux_phase = if self.phase < previous {
                    self.phase / step * slave_step
                } else {
                    wrap_turns(self.aux_phase + slave_step)
                };
                let window = 0.5 * (1.0 - cosine(self.phase));
                let synced = saw(self.aux_phase, slave_step) * window;
                (synced, sine(self.phase))
            }
            2 => {
                self.aux_phase = wrap_turns(self.aux_phase + 0.5 * step);
                drawbars(self.aux_phase, step, timbre)
            }
            _ => {
                self.aux_phase = wrap_turns(self.aux_phase + 2.002 * step);
                let bank = choice(position, 4);
                let a = sample_rom(bank, timbre, self.phase);
                let b = sample_rom((bank + 1) % 4, 1.0 - timbre, self.aux_phase);
                (0.85 * a + 0.35 * b, b)
            }
        }
    }

    fn pre_filter(&mut self, input: f32, tilt: f32, rate: f32) -> (f32, f32) {
        let low_pass = ((0.45 - tilt) / 0.45).max(0.0);
        let band_pass = ((tilt - 0.55) / 0.45).max(0.0);
        let cutoff = 110.0 + 9_500.0 * cutoff_curve(1.0 - low_pass) - 5_150.0 * band_pass;
        let g = filter_coefficient(cutoff, rate, 0.70);
        let filtered = self.pre.step(input, g, 0.80 - 0.48 * band_pass, 0.0);
        let low_mix = (4.0 * low_pass).min(1.0);
        let output =
            input + low_mix * (filtered.low - input) + band_pass * (1.45 * filtered.band - input);
        (PAD_GAIN * output, PAD_GAIN * filtered.low)
    }

    fn shape(
        &mut self,
        input: f32,
        low: f32,
        aux: f32,
        selected: usize,
        modulation: f32,
        rate: f32,
    ) -> f32 {
        let mut output = 0.0;
        for (block, mix) in self.block_mix.iter_mut().enumerate() {
            let target = if block == selected { 1.0 } else { 0.0 };
            *mix = approach(*mix, target, BLOCK_SLEW_SECONDS, rate);
            if *mix == 0.0 {
                if block == 3 {
                    self.ring_allpass = StateVariable::default();
                }
                continue;
            }
            let drive = self.block_drive[block];
            let signal = match block {
                0 => sine(input * (0.35 + 2.40 * drive) * 0.5),
                1 => {
                    let wrapped = wrap_word(input * (1.05 + 4.20 * drive));
                    input * (1.0 - drive) * 0.35 + wrapped * (0.65 + 0.35 * drive)
                }
                2 => fast_tanh(input * (1.0 + 2.4 * drive) + (input - low) * (1.8 * drive)),
                _ => {
                    let g = filter_coefficient(240.0 + 3_600.0 * modulation * drive, rate, 0.65);
                    let shifted = allpass(&mut self.ring_allpass, input, g, 0.35);
                    fast_tanh(shifted * (1.0 - 0.55 * drive) + input * aux * (1.25 * drive))
                }
            };
            output += *mix * signal;
        }
        output
    }

    fn post_filter(
        &mut self,
        input: f32,
        cutoff: f32,
        resonance: f32,
        modulation: f32,
        rate: f32,
    ) -> f32 {
        let g = filter_coefficient(85.0 + 15_500.0 * cutoff_curve(cutoff), rate, 0.74);
        let emphasis = (1.0 - 2.0 * resonance).max(0.0);
        let phase_mix = (2.0 * resonance - 1.0).max(0.0);
        let damping = (0.96 - 0.80 * emphasis - 0.30 * phase_mix).max(0.14);
        let low = self.post[0].step(input, g, damping, 0.0).low;
        let low = self.post[1].step(low, g, damping, 0.0).low;
        if phase_mix == 0.0 {
            return low;
        }
        let center = (280.0 + 2_800.0 * modulation).min(rate * 0.10);
        let first = filter_coefficient(center, rate, 0.65);
        let second = filter_coefficient(1.8 * center, rate, 0.65);
        let shifted = allpass(&mut self.phaser[0], low, first, 0.42);
        let shifted = allpass(&mut self.phaser[1], shifted, second, 0.42);
        low + 0.5 * phase_mix * (shifted - low)
    }
}

fn drawbars(half_phase: f32, note_step: f32, timbre: f32) -> (f32, f32) {
    // Ratios relative to the sub-octave phase keep the fifth continuous.
    let mut sum = 0.0;
    let mut octave = 0.0;
    for (partial, gain) in [
        (1.0, 0.32),
        (2.0, 0.40),
        (3.0, 0.18 + 0.16 * timbre),
        (4.0, 0.24),
        (6.0, 0.18),
        (8.0, 0.15),
        (10.0, 0.13),
        (12.0, 0.12),
        (16.0, 0.10),
    ] {
        let taper = ((0.49 - 0.5 * partial * note_step) * 20.0).clamp(0.0, 1.0);
        let brightness = if partial >= 6.0 {
            0.3 + 0.7 * timbre
        } else {
            1.0
        };
        let component = sine(half_phase * partial) * gain * taper * brightness;
        sum += component;
        if partial == 4.0 {
            octave = component;
        }
    }
    (sum * 0.78, octave)
}

#[inline]
fn cutoff_curve(value: f32) -> f32 {
    value * value * (0.42 + 0.58 * value)
}

#[inline]
fn filter_coefficient(hz: f32, rate: f32, ceiling: f32) -> f32 {
    (PI * hz / rate).clamp(0.008, ceiling)
}

#[inline]
fn allpass(state: &mut StateVariable, input: f32, g: f32, damping: f32) -> f32 {
    let band = state.step(input, g, damping, 0.0).band;
    input - 2.0 * damping * band
}

#[inline]
fn wrap_word(input: f32) -> f32 {
    let word = (input * 32_768.0) as i32 as i16;
    f32::from(word) * (1.0 / 32_768.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: [f32; SOUND_PARAMS] = crate::MachineKind::VastTriple.spec().default_params();

    #[test]
    fn every_exciter_and_block_combination_is_distinct_and_audible() {
        let mut by_source = [[0.0; 1_024]; SOURCE_COUNT];
        let mut by_block = [[0.0; 1_024]; BLOCK_COUNT];
        for (source, source_render) in by_source.iter_mut().enumerate() {
            for (block, block_render) in by_block.iter_mut().enumerate() {
                let mut voice = Voice::default();
                let mut params = DEFAULTS;
                params[1] = (source as f32 + 0.60) / SOURCE_COUNT as f32;
                params[3] = (block as f32 + 0.60) / BLOCK_COUNT as f32;
                let mut peak = 0.0_f32;
                for i in 0..1_024 {
                    let sample =
                        voice.step(146.83, 48_000.0, true, (i == 0).then_some(0.9), &params);
                    assert!((-1.0..=1.0).contains(&sample));
                    peak = peak.max(sample.abs());
                    if block == 1 {
                        source_render[i] = sample;
                    }
                    if source == 0 {
                        block_render[i] = sample;
                    }
                }
                assert!(peak > 0.01, "source {source} block {block}: {peak}");
            }
        }
        for renders in [by_source, by_block] {
            for (i, a) in renders.iter().enumerate() {
                for b in &renders[..i] {
                    let difference: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
                    assert!(difference > 5.0, "{i} {difference}");
                }
            }
        }
    }

    #[test]
    fn prefilter_changes_which_partials_enter_the_shaper() {
        let mut dark = Voice::default();
        let mut open = Voice::default();
        let mut band = Voice::default();
        let (mut dark_energy, mut open_energy, mut band_energy) = (0.0, 0.0, 0.0);
        let (mut dark_fold, mut open_fold) = (0.0, 0.0);
        for i in 0..4_800 {
            let input = sine((i as f32 / 16.0) % 1.0);
            let (a, _) = dark.pre_filter(input, 0.0, 48_000.0);
            let (b, _) = open.pre_filter(input, 0.5, 48_000.0);
            let (c, _) = band.pre_filter(input, 1.0, 48_000.0);
            if i >= 1_600 {
                dark_energy += a * a;
                open_energy += b * b;
                band_energy += c * c;
                let harmonic = sine((i as f32 * 3.0 / 16.0) % 1.0);
                dark_fold += sine(1.35 * a) * harmonic;
                open_fold += sine(1.35 * b) * harmonic;
            }
        }
        assert!(dark_energy < 0.01 * open_energy);
        assert!(band_energy > 0.2 * open_energy);
        assert!(dark_fold.abs() < 0.01 * open_fold.abs());
    }

    #[test]
    fn each_block_boundary_crossfades_on_a_held_note() {
        for boundary in [0.25, 0.50, 0.75] {
            let mut params = DEFAULTS;
            params[1] = 0.16;
            params[3] = boundary - 0.000_01;
            params[5] = 0.0;
            params[6] = 1.0;
            params[7] = 0.50;
            let mut switched_params = params;
            switched_params[3] = boundary + 0.000_01;
            let mut voice = Voice::default();
            let mut worst = 0.0_f32;
            for n in 0..4_800 {
                let trigger = (n == 0).then_some(0.95);
                let mut switched = voice.clone();
                let a = voice.step(165.0, 48_000.0, true, trigger, &params);
                let b = switched.step(165.0, 48_000.0, true, trigger, &switched_params);
                if n > 0 {
                    worst = worst.max((b - a).abs());
                }
            }
            assert!(worst < 0.005, "boundary {boundary}: {worst}");
            for n in 0..3_000 {
                // Rapid edits can interrupt a crossfade without clearing filter history.
                let controls = if n < 600 && n % 29 < 12 {
                    &params
                } else {
                    &switched_params
                };
                let sample = voice.step(165.0, 48_000.0, true, None, controls);
                assert!((-1.0..=1.0).contains(&sample));
            }
            let selected = choice(switched_params[3], BLOCK_COUNT);
            assert!(voice.block_mix[selected] > 0.999);
            assert!((voice.block_mix.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn fun_sample_and_hold_captures_b_on_each_a_rising_edge() {
        let mut fun = FunPair::default();
        assert_eq!(fun.step(0, 4, 0.0, 1.0), 0.0);
        assert_eq!(fun.step(0, 4, 0.75, 0.125), -0.75);
        assert_eq!(fun.step(0, 4, 0.90, 1.0), -0.75);
        assert_eq!(fun.step(0, 4, 0.25, 0.0), -0.75);
        assert_eq!(fun.step(0, 4, 1.0, 0.75), 0.50);
    }

    #[test]
    fn recursive_fun_pairs_stay_bounded_at_extreme_modulation() {
        for eq in 0..FUN_COUNT {
            let mut pair = FunPair::default();
            for i in 0..20_000 {
                let a = f32::from(i % 97 < 47);
                let b = f32::from(i % 61 < 37);
                let output1 = pair.step(0, eq, a, b);
                let output2 = pair.step(1, (eq + 1) % FUN_COUNT, 0.5 + 0.5 * output1, 1.0 - b);
                assert!((-1.0..=1.0).contains(&output1));
                assert!((-1.0..=1.0).contains(&output2));
            }
        }
    }

    #[test]
    fn drawbar_fifth_survives_the_master_cycle() {
        let note_step = 200.0 / 48_000.0;
        let fifth: f32 = (0..4_800)
            .map(|n| {
                let phase = (n as f32 / 480.0) % 1.0;
                drawbars(phase, note_step, 1.0).0 * sine(3.0 * phase)
            })
            .sum();
        assert!((2.0 * fifth / 4_800.0 - 0.34 * 0.78).abs() < 0.01);
    }

    #[test]
    fn digital_wrap_uses_signed_sixteen_bit_words() {
        assert_eq!(wrap_word(1.0), -1.0);
        assert_eq!(wrap_word(-1.25), 0.75);
        assert_eq!(wrap_word(2.5), 0.5);
    }

    #[test]
    fn phaser_creates_a_notch_with_nearby_passbands() {
        let rate = 48_000.0;
        let coefficients = [
            filter_coefficient(1_000.0, rate, 0.65),
            filter_coefficient(1_800.0, rate, 0.65),
        ];
        let response = |frequency| {
            let [mut a, mut b] = [StateVariable::default(), StateVariable::default()];
            let (mut energy, mut dry_energy, mut wet_energy) = (0.0, 0.0, 0.0);
            for sample in 0..4_800 {
                let input = 0.1 * sine((sample as f32 * frequency / rate) % 1.0);
                let wet = allpass(&mut a, input, coefficients[0], 0.42);
                let wet = allpass(&mut b, wet, coefficients[1], 0.42);
                if sample >= 2_400 {
                    let mixed = 0.5 * (input + wet);
                    energy += mixed * mixed;
                    dry_energy += input * input;
                    wet_energy += wet * wet;
                }
            }
            assert!((wet_energy / dry_energy - 1.0).abs() < 0.025);
            energy / dry_energy
        };
        assert!(response(950.0) < 0.20);
        assert!(response(440.0) > 0.7);
    }

    #[test]
    fn zero_velocity_is_silent_and_release_resets_inline_state() {
        assert!(core::mem::size_of::<Voice>() <= 128);
        let mut voice = Voice::default();
        let mut params = DEFAULTS;
        params[0] = 0.0;
        assert_eq!(voice.step(220.0, 48_000.0, true, Some(0.0), &params), 0.0);
        let mut peak = 0.0_f32;
        for i in 0..16_000 {
            let sample = voice.step(220.0, 48_000.0, false, (i == 0).then_some(0.95), &params);
            assert!((-1.0..=1.0).contains(&sample));
            peak = peak.max(sample.abs());
        }
        assert!(peak > 0.10);
        assert_eq!(voice.amp_env, 0.0);
        assert_eq!(voice.block_mix, [0.0; BLOCK_COUNT]);
        assert_eq!(voice.step(220.0, 48_000.0, false, None, &params), 0.0);
    }
}
