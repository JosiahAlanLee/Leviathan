//! Four-operator phase modulation with generated OTIS waveforms and voiced and unvoiced formants.

use crate::dac::{ConverterProfile, CzRange, DacStage};
use crate::otis::sample_rom;
use crate::voice::{approach, choice, cosine, noise, phase_step, sine, StateVariable};
use crate::{fast_tanh, wrap_turns};
use leviathan_core::SOUND_PARAMS;

const MOD_RATIOS: [f32; 8] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 7.0];
const DAC_SLEW_SECONDS: f32 = 0.005;

/// Vowel formant centers `(F1_Hz, F2_Hz)` across `/u/ -> /o/ -> /a/ -> /e/ -> /i/`.
const VOWEL_FORMANTS_HZ: [(f32, f32); 5] = [
    (310.0, 870.0),   // /u/ ("oo")
    (450.0, 1_050.0), // /o/ ("oh")
    (730.0, 1_150.0), // /a/ ("ah")
    (530.0, 1_840.0), // /e/ ("eh")
    (290.0, 2_290.0), // /i/ ("ee")
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fs77Algorithm {
    Cascade,
    DualFormant,
    RcmStack,
    Vowel,
}

/// Interpolates fixed-Hz `(F1, F2)` vowel formant frequencies across `0.0 ..= 1.0`.
#[inline]
fn vowel_formant_pair_hz(norm: f32) -> (f32, f32) {
    let scaled = norm.clamp(0.0, 1.0) * 4.0;
    let idx = (scaled as usize).min(3);
    let frac = scaled - (idx as f32);
    let (a1, a2) = VOWEL_FORMANTS_HZ[idx];
    let (b1, b2) = VOWEL_FORMANTS_HZ[idx + 1];
    (a1 + (b1 - a1) * frac, a2 + (b2 - a2) * frac)
}

/// Seven FS1R-inspired operator shapes, indexed from the sine to the narrow resonant shape.
#[inline]
fn fs1r_operator_wave(shape: usize, p: f32) -> f32 {
    match shape {
        0 => sine(p),
        1 => {
            // All 1: warm sawtooth-like harmonic series
            (0.72 * sine(p) + 0.34 * sine(2.0 * p) + 0.18 * sine(3.0 * p)) * 0.88
        }
        2 => {
            // All 2: half-rectified zero-mean bright harmonic wave
            let s = sine(p);
            s.max(0.0) * 1.55 - 0.49
        }
        3 => {
            // Odd 1: hollow square/woodwind odd harmonics
            (0.76 * sine(p) + 0.28 * sine(3.0 * p) + 0.14 * sine(5.0 * p)) * 0.90
        }
        4 => {
            // Odd 2: pinched odd-harmonic reed pulse
            sine(p) * cosine(2.0 * p) * 1.32
        }
        5 => {
            // Res 1: raised-cosine windowed resonant sync (3.5x)
            0.5 * (1.0 - cosine(p)) * sine(3.5 * p) * 1.42
        }
        _ => {
            // Res 2: higher formant-peaked sync (5.5x)
            let window = 0.5 * (1.0 - cosine(p));
            window * window * sine(5.5 * p) * 1.42
        }
    }
}

/// Continuous morph across the 7 FS1R spectral shapes (`wave_morph` in `0.0 ..= 1.0`).
#[inline]
fn morphed_fs1r_wave(wave_morph: f32, phase: f32) -> f32 {
    let phase = wrap_turns(phase);
    let scaled = wave_morph.clamp(0.0, 1.0) * 6.0;
    let i0 = (scaled as usize).min(5);
    let frac = scaled - (i0 as f32);
    let w0 = fs1r_operator_wave(i0, phase);
    if frac < 1e-4 {
        w0
    } else {
        let w1 = fs1r_operator_wave(i0 + 1, phase);
        w0 + (w1 - w0) * frac
    }
}

/// Evaluates an FS1R-inspired Phase-Aligned Formant (`PAF`) grain locked to fundamental `fund_phase`
/// with fixed-Hz formant carriers `(f1_phase, f2_phase)` and `skirt` bandwidth (`0.0 ..= 1.0`).
#[inline]
fn fs1r_formant_grain(
    fund_phase: f32,
    f1_phase: f32,
    f2_phase: f32,
    skirt: f32,
    pm_mod: f32,
) -> f32 {
    let base_win = (0.5 * (1.0 - cosine(fund_phase))).clamp(0.0, 1.0);
    let win2 = base_win * base_win;
    let win4 = win2 * win2;
    let win8 = win4 * win4;

    let sk = skirt.clamp(0.0, 1.0);
    let env_grain = if sk < 0.5 {
        let u = sk * 2.0;
        base_win + (win2 - base_win) * u
    } else {
        let u = (sk - 0.5) * 2.0;
        win2 + (win8 - win2) * u
    };
    let grain_gain = 1.0 + 0.95 * sk;

    let c1 = sine(f1_phase + pm_mod);
    let c2 = sine(f2_phase + 0.65 * pm_mod);
    env_grain * grain_gain * (0.68 * c1 + 0.44 * c2)
}

#[inline]
fn cubic_curve(x: f32) -> f32 {
    x * x * (0.45 + 0.55 * x)
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    phases: [f32; 4],
    formant_f1_phase: f32,
    formant_f2_phase: f32,
    amp_env: f32,
    mod_env: f32,
    breath_env: f32,
    fb_z1: f32,
    fb_z2: f32,
    rcm_scan: f32,
    unvoiced_svf: StateVariable,
    filt1: StateVariable,
    filt2: StateVariable,
    cz_dac: DacStage,
    ym_dac: DacStage,
    cz_mix: f32,
    ym_mix: f32,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 128);

impl Voice {
    #[inline]
    fn decode_alg_and_ratio(value: f32) -> (Fs77Algorithm, f32) {
        let zone = choice(value, 4);
        let intra = value * 4.0 - zone as f32;
        let alg = match zone {
            0 => Fs77Algorithm::Cascade,
            1 => Fs77Algorithm::DualFormant,
            2 => Fs77Algorithm::RcmStack,
            _ => Fs77Algorithm::Vowel,
        };
        (alg, MOD_RATIOS[choice(intra, MOD_RATIOS.len())])
    }

    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        gate: bool,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
        rng: &mut u32,
    ) -> f32 {
        let sr = sample_rate.max(8_000.0);
        let base_hz = hz.clamp(16.0, sr * 0.44);

        let [decay, fm_index, wave_rcm, formant_norm, skirt_unv, alg_ratio, filter_cut, reso_dac] =
            *params;
        let (alg, mod_ratio) = Self::decode_alg_and_ratio(alg_ratio);

        if let Some(vel) = trigger {
            self.amp_env = vel;
            self.mod_env = 1.0;
            self.breath_env = 1.0;
        }

        let amp_tau = if gate {
            0.05 + 3.10 * decay * decay
        } else {
            0.012 + 0.85 * decay * decay
        };
        let mod_tau = 0.018 + 0.90 * decay;
        let breath_tau = 0.010 + 0.22 * decay;
        self.amp_env = approach(self.amp_env, 0.0, amp_tau, sr);
        self.mod_env = approach(self.mod_env, 0.0, mod_tau, sr);
        self.breath_env = approach(self.breath_env, 0.0, breath_tau, sr);

        if self.amp_env < 1e-6 {
            if self.amp_env != 0.0 || trigger.is_some() {
                *self = Self::default();
            }
            return 0.0;
        }

        let prev_p0 = self.phases[0];
        let op_ratios = [
            1.0,
            mod_ratio,
            (mod_ratio * 2.0).min(12.0),
            (mod_ratio * 1.5).min(12.0),
        ];
        for (ph, &r) in self.phases.iter_mut().zip(op_ratios.iter()) {
            let f = (base_hz * r).clamp(16.0, sr * 0.44);
            *ph = wrap_turns(*ph + phase_step(f, sr));
        }

        let (f1_hz, f2_hz) = vowel_formant_pair_hz(formant_norm);
        let f1_step = phase_step(f1_hz, sr);
        let f2_step = phase_step(f2_hz, sr);
        if self.phases[0] < prev_p0 {
            let elapsed = self.phases[0] / phase_step(base_hz, sr);
            self.formant_f1_phase = elapsed * f1_step;
            self.formant_f2_phase = elapsed * f2_step;
        } else {
            self.formant_f1_phase = wrap_turns(self.formant_f1_phase + f1_step);
            self.formant_f2_phase = wrap_turns(self.formant_f2_phase + f2_step);
        }

        // SY77 RCM wavetable modulator (`otis::ROM` bank selected by `FormantHz` & `WaveRcm`)
        self.rcm_scan = wrap_turns(self.rcm_scan + (0.35 + 2.4 * fm_index) / sr);
        let rcm_mix = ((wave_rcm - 0.40) * (1.0 / 0.60)).clamp(0.0, 1.0);
        let rcm_sig = if rcm_mix > 1e-4 {
            let rcm_bank = choice(formant_norm, 4);
            let rcm_frame = 1.0 - (2.0 * self.rcm_scan - 1.0).abs();
            sample_rom(rcm_bank, rcm_frame, self.phases[3]) * rcm_mix
        } else {
            0.0
        };

        // Averaging two feedback samples attenuates rapid alternation in the loop.
        let fb_amt = 0.08 + 0.34 * fm_index;
        let fb_avg = 0.5 * (self.fb_z1 + self.fb_z2);
        let shape_morph = (wave_rcm * 1.35).clamp(0.0, 1.0);
        let op4 = morphed_fs1r_wave(
            shape_morph,
            self.phases[3] + fb_amt * fb_avg + 0.42 * rcm_sig,
        );
        self.fb_z2 = self.fb_z1;
        self.fb_z1 = op4;

        let idx_dyn = fm_index * (0.28 + 0.72 * self.mod_env) * 1.55;
        let skirt = (skirt_unv * 1.25).clamp(0.0, 1.0);

        let voiced = match alg {
            Fs77Algorithm::Cascade => {
                let op3 =
                    morphed_fs1r_wave(shape_morph * 0.7, self.phases[2] + idx_dyn * 0.75 * op4);
                let op2 = morphed_fs1r_wave(shape_morph * 0.4, self.phases[1] + idx_dyn * op3);
                let fm_car = sine(self.phases[0] + idx_dyn * op2);
                let sub_form = fs1r_formant_grain(
                    self.phases[0],
                    self.formant_f1_phase,
                    self.formant_f2_phase,
                    skirt,
                    0.18 * op2,
                );
                0.78 * fm_car + 0.28 * sub_form
            }
            Fs77Algorithm::DualFormant => {
                let op3 = morphed_fs1r_wave(shape_morph, self.phases[2] + idx_dyn * 0.85 * op4);
                let fm_bell = sine(self.phases[0] + idx_dyn * 0.65 * op3) * self.mod_env;
                let op2 = sine(self.phases[1]);
                let vocal = fs1r_formant_grain(
                    self.phases[0],
                    self.formant_f1_phase,
                    self.formant_f2_phase,
                    skirt,
                    idx_dyn * 0.32 * op2 + 0.25 * rcm_sig,
                );
                0.42 * fm_bell + 0.72 * vocal
            }
            Fs77Algorithm::RcmStack => {
                let op3 = morphed_fs1r_wave(shape_morph * 0.8, self.phases[2]);
                let mod_sum = (0.55 * op4 + 0.45 * op3 + 0.65 * rcm_sig) * idx_dyn * 0.65;
                let c1 = sine(self.phases[0] + mod_sum);
                let op2 = morphed_fs1r_wave(shape_morph * 0.5, self.phases[1]);
                let c2 = fs1r_formant_grain(
                    self.phases[0],
                    self.formant_f1_phase,
                    self.formant_f2_phase,
                    skirt,
                    0.25 * op2,
                );
                0.62 * c1 + 0.46 * c2
            }
            Fs77Algorithm::Vowel => {
                let op2 =
                    morphed_fs1r_wave(shape_morph * 0.6, self.phases[1] + 0.35 * idx_dyn * op4);
                fs1r_formant_grain(
                    self.phases[0],
                    self.formant_f1_phase,
                    self.formant_f2_phase,
                    skirt,
                    idx_dyn * 0.38 * op2 + 0.30 * rcm_sig,
                )
            }
        };

        // FS1R Unvoiced consonant / breath operator tuned to vowel `F2` (`skirt_unv > 0.35`)
        let unv_amt = ((skirt_unv - 0.35) * (1.0 / 0.65)).clamp(0.0, 1.0);
        let unvoiced = if unv_amt > 1e-4 {
            let white = noise(rng);
            let g_unv = ((core::f32::consts::PI * f2_hz) / sr).clamp(0.02, 0.65);
            let bp = self.unvoiced_svf.step(white, g_unv, 0.22, 1.0).band;
            bp * unv_amt * (0.30 + 0.70 * self.breath_env) * 0.85
        } else {
            0.0
        };

        let raw_mix = fast_tanh((voiced + unvoiced) * 1.12);

        // SY77 Dual Coupled 12 dB Digital Filters (`filt1` + `filt2`, single step per filter)
        let cut_norm = (filter_cut * 0.82 + 0.18 * self.mod_env).clamp(0.0, 1.0);
        let cut_hz = 95.0 + cubic_curve(cut_norm) * 14_500.0;
        let g_f = ((core::f32::consts::PI * cut_hz.min(sr * 0.42)) / sr).clamp(0.008, 0.72);

        let lp_reso = if reso_dac < 0.50 {
            (0.50 - reso_dac) * 2.0
        } else {
            0.0
        };
        let bp_ym_amt = if reso_dac > 0.50 {
            (reso_dac - 0.50) * 2.0
        } else {
            0.0
        };

        let damping = (0.95 - 0.78 * lp_reso - 0.45 * bp_ym_amt).clamp(0.14, 1.10);
        let g_f1 = if bp_ym_amt > 0.0 {
            (g_f * (1.0 - 0.82 * bp_ym_amt + 0.36 * bp_ym_amt * bp_ym_amt)).clamp(0.005, 0.72)
        } else {
            g_f
        };
        let stage1 = self
            .filt1
            .step(raw_mix, g_f1, damping, 1.0 + 0.35 * lp_reso);
        let in2 = stage1.low * (1.0 - bp_ym_amt) + stage1.high * bp_ym_amt;
        let filtered = self.filt2.step(in2, g_f, damping, 1.0).low;

        let converted = self.converter_mix(filtered, sr, reso_dac);
        let makeup = 1.12 + 0.45 * bp_ym_amt;
        let out = fast_tanh(converted * makeup) * self.amp_env;
        if out.abs() < 1e-12 {
            0.0
        } else {
            out
        }
    }

    fn converter_mix(&mut self, dry: f32, rate: f32, control: f32) -> f32 {
        let cz_target = (0.30 - control).max(0.0) / 0.30;
        let ym_target = (control - 0.55).max(0.0) / 0.45;
        self.cz_mix = approach(self.cz_mix, cz_target, DAC_SLEW_SECONDS, rate);
        self.ym_mix = approach(self.ym_mix, ym_target, DAC_SLEW_SECONDS, rate);

        // Separate histories let the outgoing profile finish its crossfade.
        let mut output = dry;
        if self.cz_mix > 0.0 {
            let profile = ConverterProfile::CzProvisional {
                range: CzRange::Syllabic,
                track: 0.40,
            };
            let wet = self.cz_dac.step(dry, rate, profile, 0.0);
            output += self.cz_mix * (wet - dry);
        } else {
            self.cz_dac = DacStage::default();
        }
        if self.ym_mix > 0.0 {
            let wet = self.ym_dac.step(dry, rate, ConverterProfile::Ym3012Fp, 0.0);
            output += self.ym_mix * (wet - dry);
        } else {
            self.ym_dac = DacStage::default();
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS77_DEFAULTS: [f32; SOUND_PARAMS] =
        crate::MachineKind::Fs77FormantFm.spec().default_params();

    #[test]
    fn fs77_voice_fits_memory_budget_and_decays_cleanly() {
        assert!(core::mem::size_of::<Voice>() <= 128);
        let mut voice = Voice::default();
        let mut rng = 0x1234_5678_u32;
        let mut p = FS77_DEFAULTS;
        p[0] = 0.18;
        let first = voice.step(220.0, 48_000.0, false, Some(0.95), &p, &mut rng);
        assert!(first.is_finite());
        let mut peak = first.abs();
        for _ in 0..16_000 {
            let s = voice.step(220.0, 48_000.0, false, None, &p, &mut rng);
            assert!(s.is_finite());
            peak = peak.max(s.abs());
        }
        assert!(peak > 0.10);
        assert!(voice.amp_env < 0.05);
    }

    #[test]
    fn all_four_algorithms_produce_distinct_bounded_audio() {
        let mut energies = [0.0_f32; 4];
        for (alg_idx, slot) in energies.iter_mut().enumerate() {
            let mut v = Voice::default();
            let mut rng = 0xCAFE_BABE_u32;
            let mut p = FS77_DEFAULTS;
            p[2] = 0.85;
            p[5] = (alg_idx as f32 + 0.12) * 0.25;
            for i in 0..1_024 {
                let s = v.step(
                    110.0,
                    48_000.0,
                    true,
                    if i == 0 { Some(0.9) } else { None },
                    &p,
                    &mut rng,
                );
                assert!(s.is_finite() && (-1.1..=1.1).contains(&s));
                *slot += s.abs();
            }
            assert!(*slot > 5.0);
        }
        for (a, energy) in energies.iter().enumerate() {
            for other in &energies[a + 1..] {
                assert!((energy - other).abs() > 0.25, "{energies:?}");
            }
        }
    }

    #[test]
    fn vowel_morph_shifts_energy_into_the_upper_formant() {
        let mut p_u = FS77_DEFAULTS;
        p_u[1] = 0.05;
        p_u[3] = 0.00;
        p_u[4] = 0.20;
        p_u[5] = 0.85;
        p_u[6] = 0.95;
        p_u[7] = 0.50;

        let mut p_i = p_u;
        p_i[3] = 1.00;

        let mut vu = Voice::default();
        let mut vi = Voice::default();
        let mut rng_u = 1_u32;
        let mut rng_i = 1_u32;

        let mut prev_u = 0.0_f32;
        let mut prev_i = 0.0_f32;
        let mut hf_u = 0.0_f32;
        let mut tot_u = 1e-9_f32;
        let mut hf_i = 0.0_f32;
        let mut tot_i = 1e-9_f32;

        for k in 0..4_096 {
            let trig = if k == 0 { Some(0.95) } else { None };
            let su = vu.step(110.0, 48_000.0, true, trig, &p_u, &mut rng_u);
            let si = vi.step(110.0, 48_000.0, true, trig, &p_i, &mut rng_i);
            if k >= 256 {
                let du = su - prev_u;
                let di = si - prev_i;
                hf_u += du * du;
                tot_u += su * su;
                hf_i += di * di;
                tot_i += si * si;
            }
            prev_u = su;
            prev_i = si;
        }
        assert!((hf_i / tot_i) > (hf_u / tot_u) * 1.6);
    }

    #[test]
    fn unvoiced_consonant_pair_adds_high_frequency_breath_transient() {
        let mut p_voiced = FS77_DEFAULTS;
        p_voiced[3] = 0.90;
        p_voiced[4] = 0.80;
        p_voiced[5] = 0.85;
        p_voiced[6] = 0.92;
        p_voiced[7] = 0.50;

        let mut p_unv = p_voiced;
        p_unv[4] = 1.0;

        let mut vv = Voice::default();
        let mut vu = Voice::default();
        let mut r1 = 0xABCD_1234_u32;
        let mut r2 = 0xABCD_1234_u32;
        let mut diff_e = 0.0_f32;

        for k in 0..2_048 {
            let trig = if k == 0 { Some(0.95) } else { None };
            let a = vv.step(146.83, 48_000.0, true, trig, &p_voiced, &mut r1);
            let b = vu.step(146.83, 48_000.0, true, trig, &p_unv, &mut r2);
            let d = b - a;
            diff_e += d * d;
        }
        assert!(diff_e > 0.5);
    }

    #[test]
    fn filter_modes_and_converter_profiles_are_bounded_under_max_feedback() {
        for control in [0.0, 0.5, 1.0] {
            let mut voice = Voice::default();
            let mut rng = 0x7777_7777_u32;
            let params = [0.65, 1.0, 1.0, 0.75, 0.80, 0.10, 0.55, control];
            for sample in 0..16_000 {
                let output = voice.step(
                    330.0,
                    48_000.0,
                    sample < 8_000,
                    (sample == 0).then_some(1.0),
                    &params,
                    &mut rng,
                );
                assert!(output.is_finite() && (-1.0..=1.0).contains(&output));
            }
        }
    }

    #[test]
    fn rcm_modulation_changes_audio_with_operator_shape_held_fixed() {
        let mut params = FS77_DEFAULTS;
        params[2] = 0.80;
        params[5] = 0.60;
        params[7] = 0.50;
        let mut a = Voice::default();
        let mut b = Voice::default();
        let (mut rng_a, mut rng_b) = (1, 1);
        let mut difference = 0.0;
        for sample in 0..4_096 {
            let trigger = (sample == 0).then_some(0.95);
            let x = a.step(165.0, 48_000.0, true, trigger, &params, &mut rng_a);
            let mut stronger = params;
            stronger[2] = 1.0;
            let y = b.step(165.0, 48_000.0, true, trigger, &stronger, &mut rng_b);
            difference += (x - y) * (x - y);
        }
        assert!(difference > 0.05, "{difference}");
    }

    #[test]
    fn vowel_carrier_is_fixed_and_the_skirt_widens_with_grain_shaping() {
        let mut params = FS77_DEFAULTS;
        params[1] = 0.0;
        params[2] = 0.0;
        params[3] = 1.0;
        params[4] = 0.20;
        params[5] = 0.85;
        params[6] = 0.95;
        params[7] = 0.50;
        // The /i/ upper formant is 2,290 Hz. Pitch determines the harmonic spacing.
        for pitch in [55.0, 110.0, 220.0, 440.0] {
            let mut voice = Voice::default();
            let mut rng = 1;
            let (mut center, mut above, mut below) = ([0.0; 2], [0.0; 2], [0.0; 2]);
            let harmonic = (2_290.0_f32 / pitch + 0.5) as u32;
            for n in 0..9_600 {
                let output = voice.step(
                    pitch,
                    48_000.0,
                    true,
                    (n == 0).then_some(0.95),
                    &params,
                    &mut rng,
                );
                if n >= 1_600 {
                    for (bin, frequency) in [
                        (&mut center, harmonic as f32 * pitch),
                        (&mut above, (harmonic as f32 + 2.0) * pitch),
                        (&mut below, (harmonic as f32 - 2.0) * pitch),
                    ] {
                        let phase = (n as f32 * frequency / 48_000.0) % 1.0;
                        bin[0] += output * sine(phase);
                        bin[1] += output * cosine(phase);
                    }
                }
            }
            let energy = |bin: [f32; 2]| bin[0] * bin[0] + bin[1] * bin[1];
            assert!(
                energy(center) > 4.0 * energy(above).max(energy(below)),
                "{pitch}"
            );
        }
        // Measure an off-center sideband relative to the formant carrier in the grain.
        let sideband_ratio = |skirt| {
            let (mut main, mut side) = (0.0, 0.0);
            for n in 0..256 {
                let phase = n as f32 / 256.0;
                let grain = fs1r_formant_grain(phase, 0.0, 16.0 * phase, skirt, 0.0);
                main += grain * sine(16.0 * phase);
                side += grain * sine(19.0 * phase);
            }
            (side / main).abs()
        };
        assert!(sideband_ratio(1.0) > sideband_ratio(0.0) + 0.1);
    }

    #[test]
    fn converter_crossfades_ignore_tiny_edits_at_both_boundaries() {
        for (before, after) in [(0.30, 0.299_99), (0.549_99, 0.550_01)] {
            let mut voice = Voice::default();
            let mut params = FS77_DEFAULTS;
            params[7] = before;
            let mut switched_params = params;
            switched_params[7] = after;
            let mut rng = 1;
            for sample in 0..4_096 {
                let mut switched = voice.clone();
                let mut switched_rng = rng;
                let trigger = (sample == 0).then_some(0.95);
                let unchanged = voice.step(220.0, 48_000.0, true, trigger, &params, &mut rng);
                let changed = switched.step(
                    220.0,
                    48_000.0,
                    true,
                    trigger,
                    &switched_params,
                    &mut switched_rng,
                );
                assert!(
                    (changed - unchanged).abs() < 0.002,
                    "{sample} {before} {after}"
                );
            }
        }
        let mut voice = Voice::default();
        for _ in 0..4_800 {
            voice.converter_mix(0.6, 48_000.0, 0.0);
        }
        assert!(voice.cz_mix > 0.99);
        voice.converter_mix(0.6, 48_000.0, 1.0);
        assert!(voice.cz_mix > 0.99 && (0.0..0.01).contains(&voice.ym_mix));
        for _ in 0..4_800 {
            voice.converter_mix(0.6, 48_000.0, 1.0);
        }
        assert!(voice.cz_mix < 1e-4 && voice.ym_mix > 0.99);
    }

    #[test]
    fn zero_velocity_is_silent_and_release_clears_the_voice() {
        let mut voice = Voice::default();
        let mut rng = 1;
        let mut params = FS77_DEFAULTS;
        params[0] = 0.0;
        assert_eq!(
            voice.step(220.0, 48_000.0, true, Some(0.0), &params, &mut rng),
            0.0
        );
        voice.step(220.0, 48_000.0, true, Some(1.0), &params, &mut rng);
        for _ in 0..12_000 {
            voice.step(220.0, 48_000.0, false, None, &params, &mut rng);
        }
        assert_eq!(voice.phases, [0.0; 4]);
        assert_eq!(
            voice.step(220.0, 48_000.0, false, None, &params, &mut rng),
            0.0
        );
    }
}
