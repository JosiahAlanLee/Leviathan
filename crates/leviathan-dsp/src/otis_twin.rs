use crate::voice::{approach, choice};
use crate::{fast_tanh, wrap_turns};
use leviathan_core::SOUND_PARAMS;

const TABLES: usize = 4;
const FRAMES: usize = 16;
const SAMPLES: usize = 128;
const FRACTION_BITS: u32 = 12;
const FRACTION: u32 = 1 << FRACTION_BITS;
const ADDRESS_SHIFT: u32 = u32::BITS - (SAMPLES.ilog2() + FRACTION_BITS);
const FRAME_SLEW_SECONDS: f32 = 0.009;
const TWIN_SLEW_SECONDS: f32 = 0.014;
const PITCH_SLEW_SECONDS: f32 = 0.004;
const PHASE_TURNS: f64 = 4_294_967_296.0;

const Q12_FRAC_MASK: u32 = FRACTION - 1;
const SAMPLE_INDEX_MASK: usize = SAMPLES - 1;

// Procedural tables in ROM. Every OTIS Twin lane reads the same bank.
static TWIN_ROM: [[[i16; SAMPLES]; FRAMES]; TABLES] = build_twin_rom();

#[inline]
const fn sin_q16(phase_q16: u32) -> i32 {
    // Bhaskara's sine approximation over each half-turn.
    let p = (phase_q16 & 0xFFFF) as i64;
    let neg = p >= 32_768;
    let half = if neg { p - 32_768 } else { p };
    let q = half * (32_768 - half);
    let num = 16 * q * 32_767;
    let den = 5 * 32_768_i64 * 32_768_i64 - 4 * q;
    let mag = (num / den) as i32;
    if neg {
        -mag
    } else {
        mag
    }
}

const fn normalize_twin_frame(raw: [i32; SAMPLES]) -> [i16; SAMPLES] {
    let mut sum = 0_i64;
    let mut i = 0;
    while i < SAMPLES {
        sum += raw[i] as i64;
        i += 1;
    }
    let mean = (sum / (SAMPLES as i64)) as i32;

    let mut peak = 1_i32;
    i = 0;
    while i < SAMPLES {
        let centered = (raw[i] - mean).abs();
        if centered > peak {
            peak = centered;
        }
        i += 1;
    }

    let mut out = [0_i16; SAMPLES];
    i = 0;
    while i < SAMPLES {
        let centered = (raw[i] - mean) as i64;
        out[i] = (centered * 28_672 / peak as i64) as i16;
        i += 1;
    }
    out
}

const fn build_twin_rom() -> [[[i16; SAMPLES]; FRAMES]; TABLES] {
    let mut tables = [[[0_i16; SAMPLES]; FRAMES]; TABLES];
    let mut frame = 0;
    while frame < FRAMES {
        // Bank 0: formant sweep with two moving peaks.
        let mut raw0 = [0_i32; SAMPLES];
        let f1_center = 200 + (frame as i32) * 42;
        let f2_center = 700 + ((FRAMES - 1 - frame) as i32) * 75;
        let mut s = 0;
        while s < SAMPLES {
            let base_p = table_phase(s);
            let mut acc = 0_i32;
            let mut h = 1_i32;
            while h <= 20 {
                let h100 = h * 100;
                let d1 = (h100 - f1_center).abs();
                let d2 = (h100 - f2_center).abs();
                let w1 = if d1 < 260 { 260 - d1 } else { 0 };
                let w2 = if d2 < 320 { (320 - d2) * 3 / 4 } else { 0 };
                let weight = w1 + w2 + (40 / h);
                acc += (sin_q16(base_p.wrapping_mul(h as u32)) * weight) >> 8;
                h += 1;
            }
            raw0[s] = acc;
            s += 1;
        }
        tables[0][frame] = normalize_twin_frame(raw0);

        // Bank 1: Metallic Bell & Chime Partials
        const BELL_HARMS: [u32; 10] = [1, 2, 3, 5, 7, 9, 11, 14, 17, 21];
        let mut raw1 = [0_i32; SAMPLES];
        s = 0;
        while s < SAMPLES {
            let base_p = table_phase(s);
            let mut acc = sin_q16(base_p) * 180;
            let mut idx = 1;
            while idx < 10 {
                let h = BELL_HARMS[idx];
                let comb_phase = ((frame * (idx + 2) * 4_096) + (idx * 9_102)) as u32;
                let weight = 96 + (sin_q16(comb_phase) >> 9);
                let phase_off = if (idx & 1) != 0 { 16_384_u32 } else { 0_u32 };
                acc += (sin_q16(base_p.wrapping_mul(h).wrapping_add(phase_off)) * weight)
                    / ((idx as i32) + 1);
                idx += 1;
            }
            raw1[s] = acc >> 6;
            s += 1;
        }
        tables[1][frame] = normalize_twin_frame(raw1);

        // Bank 2: windowed sync sweep with a varying-duty pulse.
        let mut raw2 = [0_i32; SAMPLES];
        let ratio_q8 = 256 + (frame as u32) * 64;
        s = 0;
        while s < SAMPLES {
            let master_p = table_phase(s);
            let slave_p = (master_p * ratio_q8) >> 8;
            let cos_p = master_p.wrapping_add(16_384);
            let window = 32_768 - sin_q16(cos_p);
            let slave = sin_q16(slave_p) + (sin_q16(slave_p.wrapping_mul(2)) >> 1);
            let sync = ((slave as i64 * window as i64) >> 15) as i32;
            let duty = 32_768 - frame as u32 * 1_408;
            let pulse = if master_p < duty { 20_000 } else { -20_000 };
            raw2[s] = (3 * sync + pulse) / 4;
            s += 1;
        }
        tables[2][frame] = normalize_twin_frame(raw2);

        // Bank 3: Sub-Harmonic Drawbar Organ -> Sine-Folded Cascade
        let mut raw3 = [0_i32; SAMPLES];
        let fold_gain = 256 + (frame as i64) * 92;
        s = 0;
        while s < SAMPLES {
            let base_p = table_phase(s);
            let organ = (sin_q16(base_p) * 180
                + sin_q16(base_p.wrapping_mul(2)) * 110
                + sin_q16(base_p.wrapping_mul(3)) * 85
                + sin_q16(base_p.wrapping_mul(4)) * 60
                + sin_q16(base_p.wrapping_mul(6)) * 42
                + sin_q16(base_p.wrapping_mul(8)) * 30)
                >> 8;
            let fold_phase = (((organ as i64 * fold_gain) >> 8) + 65_536 * 8) as u32;
            raw3[s] = sin_q16(fold_phase);
            s += 1;
        }
        tables[3][frame] = normalize_twin_frame(raw3);

        frame += 1;
    }
    tables
}

const fn table_phase(sample: usize) -> u32 {
    sample as u32 * 65_536 / SAMPLES as u32
}

#[derive(Clone, Copy)]
enum TwinLoopMode {
    OneShot,
    Bidir,
    FwdWrap,
}

impl TwinLoopMode {
    #[inline]
    fn decode(v: f32) -> (Self, f32) {
        let v = v.clamp(0.0, 1.0);
        if v < 0.34 {
            (Self::OneShot, (v * (1.0 / 0.34)).clamp(0.0, 1.0))
        } else if v < 0.67 {
            (Self::Bidir, ((v - 0.34) * (1.0 / 0.33)).clamp(0.0, 1.0))
        } else {
            (Self::FwdWrap, ((v - 0.67) * (1.0 / 0.33)).clamp(0.0, 1.0))
        }
    }
}

/// Converts a 32-bit full-scale phase accumulator word into a 19-bit `Q20.12` sample address.
#[inline]
const fn phase_to_q20_12(phase: u32) -> u32 {
    phase >> ADDRESS_SHIFT
}

/// Selects a detuned unison, fifth, or octave with a continuous blend target.
#[inline]
fn twin_osc_ratio_and_mix(ts: f32) -> (f32, f32) {
    if ts <= 0.01 {
        return (1.0, 0.0);
    }
    if ts < 0.55 {
        let u = (ts - 0.01) * (1.0 / 0.54);
        let ratio = 1.0 + 0.0012 + 0.0093 * u * u;
        (ratio, 0.85 * u)
    } else if ts < 0.80 {
        let u = (ts - 0.55) * (1.0 / 0.25);
        let ratio = 1.498_307 * (1.0 + 0.0022 * u);
        (ratio, 0.85 - 0.07 * u)
    } else {
        let u = (ts - 0.80) * (1.0 / 0.20);
        let ratio = 2.0 * (1.0 + 0.0018 + 0.0025 * u);
        (ratio, 0.78 - 0.04 * u)
    }
}

fn center_unison(ratio_b: f32, mix_b: f32) -> (f32, f32) {
    let power = mix_b * mix_b;
    let offset = (ratio_b - 1.0) * power / (1.0 + power);
    (1.0 - offset, ratio_b - offset)
}

#[inline]
fn sample_twin_rom_q20_12(table: usize, frame_pos: f32, addr_q12: u32) -> f32 {
    let bank = &TWIN_ROM[table.min(TABLES - 1)];
    let f_clamped = frame_pos.clamp(0.0, (FRAMES - 1) as f32);
    let f0 = (f_clamped as usize).min(FRAMES - 1);
    let f1 = (f0 + 1).min(FRAMES - 1);
    let f_frac = f_clamped - (f0 as f32);

    let idx0 = ((addr_q12 >> FRACTION_BITS) as usize) & SAMPLE_INDEX_MASK;
    let idx1 = (idx0 + 1) & SAMPLE_INDEX_MASK;
    let s_frac = ((addr_q12 & Q12_FRAC_MASK) as f32) * (1.0 / (FRACTION as f32));

    let a0 = bank[f0][idx0] as f32;
    let a1 = bank[f0][idx1] as f32;
    let b0 = bank[f1][idx0] as f32;
    let b1 = bank[f1][idx1] as f32;

    let wave_a = a0 + (a1 - a0) * s_frac;
    let wave_b = b0 + (b1 - b0) * s_frac;
    (wave_a + (wave_b - wave_a) * f_frac) * (1.0 / 32_768.0)
}

#[derive(Clone, Debug)]
pub(super) struct TwinVoice {
    phase_a: u32,
    phase_b: u32,
    amp_env: f32,
    scan_env: f32,
    scan_phase: f32,
    frame_a: f32,
    frame_b: f32,
    mix_b: f32,
    ratio_b: f32,
    ratio_a: f32,
    held_wave: f32,
    k1_slewed: f32,
    lp: [f32; 4],
    hp_lp: [f32; 2],
    decim_counter: u8,
}

const _: () = assert!(core::mem::size_of::<TwinVoice>() <= 96);
const _: () = assert!(core::mem::size_of_val(&TWIN_ROM) == 16 * 1024);

impl Default for TwinVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl TwinVoice {
    #[inline]
    const fn new() -> Self {
        Self {
            phase_a: 0,
            phase_b: 0x4000_0000,
            amp_env: 0.0,
            scan_env: 0.0,
            scan_phase: 0.0,
            frame_a: 0.0,
            frame_b: (FRAMES - 1) as f32,
            mix_b: 0.0,
            ratio_b: 1.0,
            ratio_a: 1.0,
            held_wave: 0.0,
            k1_slewed: 0.35,
            lp: [0.0; 4],
            hp_lp: [0.0; 2],
            decim_counter: 0,
        }
    }

    #[inline]
    fn compute_target_frames(
        frame_base: f32,
        scan_depth: f32,
        scan_env: f32,
        scan_phase: f32,
        loop_mode: TwinLoopMode,
    ) -> (f32, f32) {
        let max_frame = (FRAMES - 1) as f32;
        let span = scan_depth * 0.85;
        let lo = (frame_base * (1.0 - span)).clamp(0.0, 1.0);
        let hi = (lo + span).clamp(0.0, 1.0);

        match loop_mode {
            TwinLoopMode::OneShot => {
                let prog = (1.0 - scan_env).clamp(0.0, 1.0);
                let na = lo + (hi - lo) * prog;
                let nb = hi - (hi - lo) * prog;
                (na * max_frame, nb * max_frame)
            }
            TwinLoopMode::Bidir => {
                let tri = 1.0 - f32::abs(2.0 * scan_phase - 1.0);
                let na = lo + (hi - lo) * tri;
                let nb = hi - (hi - lo) * tri;
                (na * max_frame, nb * max_frame)
            }
            TwinLoopMode::FwdWrap => {
                let wrap_a = wrap_turns(frame_base + scan_depth * scan_phase);
                let wrap_b = 1.0 - wrap_a;
                (wrap_a * max_frame, wrap_b * max_frame)
            }
        }
    }

    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        gate: bool,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
        calibrated: bool,
    ) -> f32 {
        let sr = sample_rate.max(8_000.0);
        let root = hz.clamp(8.0, sr * 0.45);

        // The rack supplies normalized controls through the machine registry.
        let [decay, table, frame_base, scan_depth, f1, f2, twin_spread, mode_grit] = *params;
        let table = choice(table, TABLES);
        let (loop_mode, grit) = TwinLoopMode::decode(mode_grit);
        let (target_ratio_b, target_mix_b) = twin_osc_ratio_and_mix(twin_spread);
        let (target_ratio_a, target_ratio_b) = if calibrated && twin_spread < 0.55 {
            center_unison(target_ratio_b, target_mix_b)
        } else {
            (1.0, target_ratio_b)
        };

        if let Some(velocity) = trigger {
            let cold_start = self.amp_env == 0.0;
            self.amp_env = velocity;
            self.scan_env = 1.0;
            self.scan_phase = 0.0;
            self.decim_counter = 0;
            if cold_start {
                self.phase_a = 0;
                self.phase_b = 0x4000_0000;
                self.mix_b = target_mix_b;
                self.ratio_b = target_ratio_b;
                self.ratio_a = target_ratio_a;
                (self.frame_a, self.frame_b) = Self::compute_target_frames(
                    frame_base,
                    scan_depth,
                    self.scan_env,
                    self.scan_phase,
                    loop_mode,
                );
            }
        }

        let amp_tau = if gate {
            0.08 + 3.20 * decay * decay
        } else {
            0.015 + 0.95 * decay * decay
        };
        let scan_tau = 0.06 + 2.40 * decay;
        self.amp_env *= 1.0 - 1.0 / (sr * amp_tau);
        self.scan_env *= 1.0 - 1.0 / (sr * scan_tau);
        if self.amp_env < 1e-6 {
            self.amp_env = 0.0;
            self.lp = [0.0; 4];
            self.hp_lp = [0.0; 2];
            return 0.0;
        }

        self.mix_b = approach(self.mix_b, target_mix_b, TWIN_SLEW_SECONDS, sr);
        self.ratio_b = approach(self.ratio_b, target_ratio_b, PITCH_SLEW_SECONDS, sr);
        self.ratio_a = approach(self.ratio_a, target_ratio_a, PITCH_SLEW_SECONDS, sr);
        let hz_a = (root * self.ratio_a).clamp(8.0, sr * 0.45);
        let hz_b = (root * self.ratio_b).min(sr * 0.45);
        let phase_scale = PHASE_TURNS / f64::from(sr);
        self.phase_a = self
            .phase_a
            .wrapping_add((f64::from(hz_a) * phase_scale) as u32);
        self.phase_b = self
            .phase_b
            .wrapping_add((f64::from(hz_b) * phase_scale) as u32);

        let scan_rate_hz = 0.22 + 5.0 * scan_depth * scan_depth;
        self.scan_phase = wrap_turns(self.scan_phase + scan_rate_hz / sr);

        let (target_fa, target_fb) = Self::compute_target_frames(
            frame_base,
            scan_depth,
            self.scan_env,
            self.scan_phase,
            loop_mode,
        );
        self.frame_a = approach(self.frame_a, target_fa, FRAME_SLEW_SECONDS, sr);
        self.frame_b = approach(self.frame_b, target_fb, FRAME_SLEW_SECONDS, sr);

        if self.decim_counter == 0 {
            self.decim_counter = (grit * grit * 6.0) as u8;
            let discard = (grit * FRACTION_BITS as f32) as u32;
            let frac_mask = u32::MAX << discard;

            let addr_a = phase_to_q20_12(self.phase_a) & frac_mask;
            let osc_a = sample_twin_rom_q20_12(table, self.frame_a, addr_a);

            let mix_b = self.mix_b;
            let mut s = if mix_b > 1e-5 {
                let addr_b = phase_to_q20_12(self.phase_b) & frac_mask;
                let osc_b = sample_twin_rom_q20_12(table, self.frame_b, addr_b);
                let norm_gain = 1.0 / (1.0 + 0.55 * mix_b);
                (osc_a + mix_b * osc_b) * norm_gain
            } else {
                osc_a
            };

            if grit > 0.45 {
                let levels = 2048.0 - 1880.0 * ((grit - 0.45) / 0.55);
                let q = (s * levels) as i32;
                s = (q as f32) / levels;
            }
            self.held_wave = s;
        } else {
            self.decim_counter -= 1;
        }

        self.filter(self.held_wave, f1, f2, sr, calibrated) * self.amp_env
    }

    fn filter(&mut self, input: f32, f1: f32, f2: f32, rate: f32, calibrated: bool) -> f32 {
        // F1 is the low-pass ceiling. Positive envelope changes open it faster.
        let target_cut = (0.06 + 0.78 * f1 + 0.14 * self.amp_env).clamp(0.04, 0.94);
        let target_k1 = (target_cut * target_cut * 0.86).clamp(0.005, 0.86);
        let time = if target_k1 > self.k1_slewed {
            0.0015
        } else {
            0.006
        };
        self.k1_slewed = approach(self.k1_slewed, target_k1, time, rate);
        let g1 = self.k1_slewed;

        let resonance = (1.0 - 2.0 * f2).max(0.0);
        let fb_gain = resonance * 2.85 * (1.0 - 0.22 * g1);
        let fb_sig = fast_tanh(self.lp[3]);
        let driven_in = fast_tanh(input * (1.0 + 0.85 * fb_gain) - fb_gain * fb_sig);

        self.lp[0] += g1 * (driven_in - self.lp[0]);
        self.lp[1] += g1 * (self.lp[0] - self.lp[1]);
        self.lp[2] += g1 * (self.lp[1] - self.lp[2]);
        self.lp[3] += g1 * (self.lp[2] - self.lp[3]);
        let lp_out = self.lp[2] + resonance * (self.lp[3] - self.lp[2]);

        // Above center, F2 raises the two-pole high-pass floor.
        let highpass = (2.0 * f2 - 1.0).max(0.0);
        let min_hp = (60.0 / rate).clamp(0.0005, 0.01);
        let max_hp = (g1 * 0.42).clamp(min_hp, 0.22);
        let g_hp = min_hp + (max_hp - min_hp) * highpass * highpass;

        self.hp_lp[0] += g_hp * (lp_out - self.hp_lp[0]);
        let hp1 = lp_out - self.hp_lp[0];
        self.hp_lp[1] += g_hp * (hp1 - self.hp_lp[1]);
        let hp2 = hp1 - self.hp_lp[1];

        let base_makeup = if calibrated { 1.38 } else { 1.15 };
        let makeup = base_makeup + 1.05 * highpass + 0.15 * resonance;
        fast_tanh(hp2 * makeup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: [f32; SOUND_PARAMS] = crate::MachineKind::OtisTwin.spec().default_params();

    #[test]
    fn unison_centers_the_power_weighted_ratio() {
        for spread in [0.12, 0.28, 0.42, 0.54] {
            let (ratio, mix) = twin_osc_ratio_and_mix(spread);
            let (a, b) = center_unison(ratio, mix);
            let power = mix * mix;
            assert!(a < 1.0 && b > 1.0);
            assert!(((a + power * b) / (1.0 + power) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn twin_voice_and_rom_fit_t24_memory_budget() {
        assert!(core::mem::size_of::<TwinVoice>() <= 96);
        assert_eq!(core::mem::size_of_val(&TWIN_ROM), 16_384);
        for table in 0..TABLES {
            for frame in 0..FRAMES {
                let mut sum = 0_i64;
                let mut peak = 0_i32;
                for &s in &TWIN_ROM[table][frame] {
                    sum += s as i64;
                    let a = i32::from(s).abs();
                    if a > peak {
                        peak = a;
                    }
                }
                let mean = (sum / SAMPLES as i64).abs();
                assert!(peak >= 20_000);
                assert!(mean <= 64);
            }
        }
    }

    #[test]
    fn twin_bidir_readers_travel_in_opposite_directions() {
        let mut v = TwinVoice::new();
        let mut p = DEFAULTS;
        p[2] = 0.25; // FramePos
        p[3] = 0.80; // Fast Bidir scan (~3.42 Hz => ~14,035 samples per full cycle at 48 kHz)
        p[6] = 0.45; // Twin unison active
        p[7] = 0.50; // Bidir zone

        let _ = v.step(220.0, 48_000.0, true, Some(0.95), &p, false);

        // Advance into the rising half of `Bidir` (`scan_phase` ~ 0.15 .. 0.35)
        for _ in 0..2_200 {
            let _ = v.step(220.0, 48_000.0, true, None, &p, false);
        }
        let fa_0 = v.frame_a;
        let fb_0 = v.frame_b;
        for _ in 0..2_000 {
            let _ = v.step(220.0, 48_000.0, true, None, &p, false);
        }
        let d_rise_a = v.frame_a - fa_0;
        let d_rise_b = v.frame_b - fb_0;
        assert!(d_rise_a > 0.5, "Osc A must scan upward in first Bidir half");
        assert!(
            d_rise_b < -0.5,
            "Osc B must counter-scan downward in first Bidir half"
        );

        // Advance into the falling half of `Bidir` (`scan_phase` ~ 0.65 .. 0.85)
        for _ in 0..5_000 {
            let _ = v.step(220.0, 48_000.0, true, None, &p, false);
        }
        let fa_1 = v.frame_a;
        let fb_1 = v.frame_b;
        for _ in 0..2_000 {
            let _ = v.step(220.0, 48_000.0, true, None, &p, false);
        }
        let d_fall_a = v.frame_a - fa_1;
        let d_fall_b = v.frame_b - fb_1;
        assert!(
            d_fall_a < -0.5,
            "Osc A must scan downward in second Bidir half"
        );
        assert!(
            d_fall_b > 0.5,
            "Osc B must counter-scan upward in second Bidir half"
        );
    }

    #[test]
    fn twin_q20_12_phase_accuracy_over_128k_samples() {
        let mut voice = TwinVoice::new();
        let mut params = DEFAULTS;
        params[3] = 0.0;
        params[6] = 0.62;
        params[7] = 0.0;
        let rate = 48_000.0;
        let hz = 317.125;
        let hz_b = hz * twin_osc_ratio_and_mix(params[6]).0;
        let increment = |frequency| (f64::from(frequency) / f64::from(rate) * PHASE_TURNS) as u64;
        let (step_a, step_b) = (increment(hz), increment(hz_b));
        for sample in 1..=128_000 {
            let out = voice.step(hz, rate, true, (sample == 1).then_some(0.95), &params, false);
            assert!(out.is_finite());
            assert!(out.abs() <= 1.0);
            let phase_a = (sample * step_a) as u32;
            let phase_b = (sample * step_b + 0x4000_0000) as u32;
            assert_eq!(voice.phase_a, phase_a);
            assert_eq!(voice.phase_b, phase_b);
            assert_eq!(phase_to_q20_12(phase_a), phase_a / 8192);
            assert_eq!(phase_to_q20_12(phase_b), phase_b / 8192);
        }
    }

    #[test]
    fn twin_series_filters_reshape_the_frequency_response() {
        let response = |hz, f1, f2| {
            let mut voice = TwinVoice::new();
            let mut energy = 0.0;
            for sample in 0..24_000 {
                let input = 0.02 * crate::voice::sine((sample as f32 * hz / 48_000.0) % 1.0);
                let output = voice.filter(input, f1, f2, 48_000.0, false);
                if sample >= 12_000 {
                    energy += output * output;
                }
            }
            energy
        };
        let bass = response(120.0, 0.42, 0.05) / response(120.0, 0.42, 0.5);
        let peak = response(960.0, 0.42, 0.05) / response(960.0, 0.42, 0.5);
        assert!(
            peak > 1.5 * bass && peak > 1.5,
            "resonant gain {peak}, bass gain {bass}"
        );

        let sub = response(60.0, 0.82, 0.92) / response(60.0, 0.82, 0.5);
        let mid = response(2400.0, 0.82, 0.92) / response(2400.0, 0.82, 0.5);
        assert!(
            sub < 0.05 && mid > 0.1,
            "high-pass sub gain {sub}, mid gain {mid}"
        );
    }

    #[test]
    fn twin_envelopes_keep_their_time_at_high_sample_rates() {
        let mut params = DEFAULTS;
        params[0] = 1.0;
        let after_one_second = |rate| {
            let mut voice = TwinVoice::new();
            for sample in 0..rate as usize {
                voice.step(220.0, rate, true, (sample == 0).then_some(1.0), &params, false);
            }
            (voice.amp_env, voice.scan_env)
        };
        let (amp_48, scan_48) = after_one_second(48_000.0);
        let (amp_96, scan_96) = after_one_second(96_000.0);
        assert!((amp_48 - amp_96).abs() < 0.01);
        assert!((scan_48 - scan_96).abs() < 0.01);
    }

    #[test]
    fn twin_wrap_and_mode_transitions_stay_discontinuity_free() {
        let mut v = TwinVoice::new();
        let mut p = DEFAULTS;
        p[0] = 0.95;
        p[2] = 0.15;
        p[3] = 1.00; // Fastest FwdWrap rate (~5.22 Hz -> wraps every ~9,195 samples)
        p[4] = 0.72;
        p[5] = 0.50;
        p[6] = 0.45;
        p[7] = 0.70; // Clean FwdWrap

        let mut prev = v.step(165.0, 48_000.0, true, Some(0.90), &p, false);
        let mut max_step_delta = 0.0_f32;

        for i in 1..24_000 {
            // Switch modes and TwinSpread live mid-note without retriggering!
            if i == 11_000 {
                p[7] = 0.05; // OneShot
                p[6] = 0.48; // Smoothly open TwinSpread unison
            } else if i == 15_000 {
                p[7] = 0.45; // Bidir
                p[6] = 0.70; // Fifth stack
            } else if i == 19_000 {
                p[7] = 0.70; // Back to FwdWrap across wraparound boundary
                p[6] = 0.00; // Smoothly close back to single-reader
            }
            let cur = v.step(165.0, 48_000.0, true, None, &p, false);
            let d = f32::abs(cur - prev);
            if d > max_step_delta {
                max_step_delta = d;
            }
            prev = cur;
        }
        assert!(
            max_step_delta < 0.35,
            "Wraparound and mode/spread transitions must remain click-free (max delta = {})",
            max_step_delta
        );
    }

    #[test]
    fn twin_blend_and_pitch_slew_to_the_new_interval() {
        let mut voice = TwinVoice::new();
        let mut p = DEFAULTS;
        p[2] = 0.0;
        p[6] = 0.0;
        p[7] = 0.67;
        assert_eq!(voice.step(220.0, 48_000.0, true, Some(0.0), &p, false), 0.0);
        voice.step(220.0, 48_000.0, true, Some(0.95), &p, false);
        assert_eq!(voice.mix_b, 0.0);
        assert!(voice.frame_b > 14.99);
        p[6] = 0.85;
        voice.step(220.0, 48_000.0, true, None, &p, false);
        assert!((0.0..0.01).contains(&voice.mix_b));
        assert!((1.0..1.01).contains(&voice.ratio_b));
        for _ in 0..4_800 {
            voice.step(220.0, 48_000.0, true, None, &p, false);
        }
        assert!(voice.mix_b > 0.75);
        assert!(voice.ratio_b > 2.0);
    }
}
