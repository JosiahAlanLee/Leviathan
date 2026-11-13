use crate::voice::{approach, choice, cosine, noise, phase_step};
use crate::wrap_turns;
use leviathan_core::SOUND_PARAMS;

const WAVE_COUNT: usize = 8;

#[derive(Clone, Copy)]
enum Wave {
    Saw,
    Square,
    Pulse,
    DoubleSine,
    SawPulse,
    Reso1,
    Reso2,
    Reso3,
}

impl Wave {
    const ALL: [Self; WAVE_COUNT] = [
        Self::Saw,
        Self::Square,
        Self::Pulse,
        Self::DoubleSine,
        Self::SawPulse,
        Self::Reso1,
        Self::Reso2,
        Self::Reso3,
    ];
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    phase: [f32; 2],
    amp: f32,
    dcw: f32,
    velocity: f32,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 256);

impl Voice {
    #[inline]
    pub(super) fn step(
        &mut self,
        base_hz: f32,
        sample_rate: f32,
        gate: bool,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
        rng: &mut u32,
        calibrated: bool,
    ) -> f32 {
        let [wave_a, wave_b_mix, depth, dcw_decay, amp_attack, amp_release, mod_mode, detune] =
            *params;
        if let Some(velocity) = trigger {
            self.velocity = velocity;
            self.dcw = 1.0;
        }
        let target = if gate { self.velocity } else { 0.0 };
        let attack = 0.001 + 0.15 * amp_attack * amp_attack;
        let release = 0.006 + 1.5 * amp_release * amp_release;
        self.amp = approach(
            self.amp,
            target,
            if target > self.amp { attack } else { release },
            sample_rate,
        );
        self.dcw = approach(
            self.dcw,
            0.0,
            0.012 + 1.2 * dcw_decay * dcw_decay,
            sample_rate,
        );
        if self.amp == 0.0 {
            return 0.0;
        }

        // The lower half of Wave B / Mix scans from line A to B. The upper
        // half scans B's waveforms while keeping both lines audible.
        let wave_b = choice((2.0 * wave_b_mix - 1.0).max(0.0), WAVE_COUNT);
        let blend = wave_b_mix.min(0.5);
        // Reduce the warp as the fundamental approaches the Nyquist limit.
        let detune = detune * 2.0 - 1.0;
        let (hz_a, hz_b) = if calibrated {
            let spread = 1.0 + 0.03 * detune.abs();
            if detune >= 0.0 {
                (base_hz / spread, base_hz * spread)
            } else {
                (base_hz * spread, base_hz / spread)
            }
        } else {
            (base_hz, base_hz * (1.0 + 0.06 * detune))
        };
        let step_a = phase_step(hz_a, sample_rate);
        let step_b = phase_step(hz_b, sample_rate);
        let shape = depth * (0.2 + 0.8 * self.dcw);
        self.phase[0] = wrap_turns(self.phase[0] + step_a);
        self.phase[1] = wrap_turns(self.phase[1] + step_b);
        let a = waveform(
            Wave::ALL[choice(wave_a, WAVE_COUNT)],
            self.phase[0],
            shape,
            step_a,
        );
        let b = waveform(Wave::ALL[wave_b], self.phase[1], shape, step_b);
        let mixed = a + blend * (b - a);
        let output = if mod_mode <= 0.5 {
            mixed + (2.0 * mod_mode) * (a * b - mixed)
        } else {
            a * b + (2.0 * mod_mode - 1.0) * (a * noise(rng) - a * b)
        };
        let gain = if calibrated { 0.76 } else { 1.0 };
        output * self.amp * gain
    }
}

#[inline]
fn bent_phase(phase: f32, knee: f32) -> f32 {
    if phase < knee {
        0.5 * phase / knee
    } else {
        0.5 + 0.5 * (phase - knee) / (1.0 - knee)
    }
}

#[inline]
fn waveform(wave: Wave, phase: f32, depth: f32, step: f32) -> f32 {
    let depth = depth * (1.0 - step * 2.0);
    let knee = 0.5 - 0.46 * depth;
    match wave {
        // Complete both halves of the cosine at different speeds.
        Wave::Saw => cosine(bent_phase(phase, knee)),
        Wave::Square => {
            let transition = 0.25 - 0.21 * depth;
            let half = if phase < 0.5 { phase } else { phase - 0.5 };
            let head = 0.25 * (half / transition).min(1.0);
            let tail = 0.25 * ((half - (0.5 - transition)) / transition).max(0.0);
            let offset = if phase < 0.5 { 0.25 } else { 0.75 };
            cosine(offset + head + tail - 0.25)
        }
        Wave::Pulse => {
            let width = 1.0 - 0.82 * depth;
            cosine((phase / width).min(1.0))
        }
        Wave::DoubleSine => {
            let base = cosine(phase);
            let doubled = cosine(2.0 * phase);
            base + depth * (doubled - base)
        }
        Wave::SawPulse => {
            let warped = bent_phase(phase, knee);
            cosine((warped / (1.0 - 0.62 * depth)).min(1.0))
        }
        Wave::Reso1 | Wave::Reso2 | Wave::Reso3 => {
            let base = cosine(phase);
            let triangle = 1.0 - (2.0 * phase - 1.0).abs();
            let (window, harmonics) = match wave {
                Wave::Reso1 => (triangle * (1.0 - phase), 9.0),
                Wave::Reso2 => (triangle, 12.0),
                _ => (triangle * phase, 15.0),
            };
            let resonance = (1.0 + depth * harmonics).min(0.45 / step.max(0.000_01));
            base + depth * window * (cosine(phase * resonance) - base)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::{pitch_hz, sine};
    use crate::MachineKind;

    #[test]
    fn all_waveforms_remain_bounded_over_midi_pitches_and_ratchets() {
        for sample_rate in [8_000.0, 48_000.0, 96_000.0] {
            for wave in 0..WAVE_COUNT {
                let mut params = MachineKind::CzPhaseDist.spec().default_params();
                params[0] = (wave as f32 + 0.5) / WAVE_COUNT as f32;
                params[1] = 0.5 + 0.5 * params[0];
                params[2] = 1.0;
                params[6] = if wave % 2 == 0 { 0.5 } else { 1.0 };
                for note in 0..=127 {
                    let hz = pitch_hz((note as f32 - 60.0) / 12.0);
                    let mut voice = Voice::default();
                    let mut rng = 123;
                    let mut peak = 0.0_f32;
                    for i in 0..512 {
                        let gate = i % 128 < 100;
                        let sample = voice.step(
                            hz,
                            sample_rate,
                            gate,
                            (i % 128 == 0).then_some(1.0),
                            &params,
                            &mut rng,
                            false,
                        );
                        assert!(
                            (-1.001..=1.001).contains(&sample),
                            "wave {wave}, note {note}"
                        );
                        peak = peak.max(sample.abs());
                    }
                    assert!(peak > 0.02);
                }
            }
        }
    }

    #[test]
    fn dcw_increases_second_harmonic_and_retrigger_reopens_the_envelope() {
        let second_harmonic = |depth| {
            let mut cosine_sum = 0.0;
            let mut sine_sum = 0.0;
            for sample in 0..256 {
                let phase = sample as f32 / 256.0;
                let signal = waveform(Wave::Saw, phase, depth, 1.0 / 256.0);
                cosine_sum += signal * cosine(2.0 * phase);
                sine_sum += signal * sine(2.0 * phase);
            }
            (cosine_sum * cosine_sum + sine_sum * sine_sum) / (128.0 * 128.0)
        };
        assert!(second_harmonic(0.0) < 1e-6);
        assert!(second_harmonic(1.0) > 0.0225);

        let mut voice = Voice::default();
        let mut params = MachineKind::CzPhaseDist.spec().default_params();
        params[3] = 0.0;
        let mut rng = 123;
        voice.step(220.0, 48_000.0, true, Some(0.8), &params, &mut rng, false);
        for _ in 0..4096 {
            voice.step(220.0, 48_000.0, true, None, &params, &mut rng, false);
        }
        assert!(voice.dcw < 0.01);
        let previous_amp = voice.amp;
        voice.step(220.0, 48_000.0, true, Some(0.8), &params, &mut rng, false);
        assert!(voice.dcw > 0.99);
        assert!((voice.amp - previous_amp).abs() < 0.001);
        for _ in 0..96_000 {
            voice.step(220.0, 48_000.0, false, None, &params, &mut rng, false);
        }
        assert_eq!(voice.amp, 0.0);
    }

    #[test]
    fn all_shapes_begin_as_a_cosine_and_diverge_at_full_dcw() {
        let waves = Wave::ALL.map(|wave| {
            core::array::from_fn::<_, 128, _>(|sample| {
                let phase = sample as f32 / 128.0;
                assert!((waveform(wave, phase, 0.0, 0.002) - cosine(phase)).abs() < 1e-6);
                waveform(wave, phase, 1.0, 0.002)
            })
        });
        for (index, wave) in waves.iter().enumerate() {
            for other in &waves[..index] {
                let distance: f32 = wave.iter().zip(other).map(|(a, b)| (a - b).abs()).sum();
                assert!(
                    distance > 1.0,
                    "wave {index} is too close to an earlier shape"
                );
            }
        }
    }

    #[test]
    fn calibrated_lines_center_and_exchange_detune() {
        let mut center = Voice::default();
        let mut down = center.clone();
        let mut up = center.clone();
        let params = crate::LanePatch::for_machine_with_calibration(MachineKind::CzPhaseDist).params;
        let mut down_params = params;
        down_params[7] = 0.25;
        let mut up_params = params;
        up_params[7] = 0.75;
        let mut rng = 123;
        for i in 0..512 {
            let trigger = (i == 0).then_some(0.8);
            center.step(220.0, 48_000.0, true, trigger, &params, &mut rng, true);
            down.step(220.0, 48_000.0, true, trigger, &down_params, &mut rng, true);
            up.step(220.0, 48_000.0, true, trigger, &up_params, &mut rng, true);
        }
        assert_eq!(center.phase[0], center.phase[1]);
        assert_eq!(down.phase, [up.phase[1], up.phase[0]]);
    }
}
