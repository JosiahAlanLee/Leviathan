use crate::voice::{choice, phase_step, sine};
use crate::wrap_turns;
use leviathan_core::SOUND_PARAMS;

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    carrier: f32,
    modulator: f32,
    target: f32,
    light: f32,
    strike_env: f32,
    lpg: [f32; 2],
    dc: f32,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 160);

impl Voice {
    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        gate: bool,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
    ) -> f32 {
        let [decay, drive, symmetry, fm, ratio, ring, mode, strike] = *params;
        if let Some(velocity) = trigger {
            self.target = velocity;
            self.strike_env = strike * velocity;
        }
        let dt = 1.0 / sample_rate;
        self.strike_env *= 1.0 - dt / (0.003 + 0.009 * strike);
        if self.strike_env < 1e-6 {
            self.strike_env = 0.0;
        }
        let target = ((if gate { self.target } else { 0.0 }) + self.strike_env).min(1.0);
        let release = (0.018 + 1.3 * decay * decay) * (1.0 - 0.78 * self.light);
        let tau = if target > self.light {
            0.001 + 0.004 * (1.0 - strike)
        } else {
            release
        };
        self.light += (target - self.light) * (dt / (tau + dt));
        if self.light < 1e-6 && target == 0.0 {
            self.light = 0.0;
            self.lpg = [0.0; 2];
            self.dc = 0.0;
            return 0.0;
        }

        // Each octave interval sweeps continuously, with exact harmonic ratios at
        // the segment boundaries. The midpoint gives a 2:1 modulator.
        const OCTAVE_RATIOS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];
        let octave = choice(ratio, OCTAVE_RATIOS.len());
        let ratio = OCTAVE_RATIOS[octave] * (1.0 + (4.0 * ratio - octave as f32));
        self.modulator = wrap_turns(self.modulator + phase_step(hz * ratio, sample_rate));
        let modulator = sine(self.modulator);
        let increment = (hz * (1.0 + 4.0 * fm * modulator) * dt).clamp(-0.45, 0.45);
        self.carrier = wrap_turns(self.carrier + increment);
        let carrier = sine(self.carrier);
        let modulated = carrier + ring * (carrier * modulator - carrier);
        let folded = fold(modulated, drive, symmetry);

        let cutoff = 18.0 + 14_000.0 * self.light * self.light;
        let scaled = core::f32::consts::TAU * cutoff * dt;
        let pole = scaled / (1.0 + scaled);
        self.lpg[0] += pole * (folded - self.lpg[0]);
        self.lpg[1] += pole * (self.lpg[0] - self.lpg[1]);
        let output = match choice(mode, 3) {
            // The LP mode keeps its passband gain while the gate is lit.
            // A small floor fade closes it once the vactrol is nearly dark.
            0 => self.lpg[1] * (40.0 * self.light).min(1.0),
            1 => self.lpg[1] * self.light,
            _ => folded * self.light,
        };
        let dc_step = (core::f32::consts::TAU * 7.0 * dt).min(0.01);
        self.dc += dc_step * (output - self.dc);
        (output - self.dc).clamp(-1.2, 1.2)
    }
}

#[inline]
fn fold(input: f32, drive: f32, symmetry: f32) -> f32 {
    // Five progressively mixed sinusoidal fold cells. T_n(cos(t)) = cos(n*t)
    // places each cell at a distinct overtone at center symmetry. The dry
    // fundamental falls as the cells enter, keeping the drive sweep monotonic.
    const CELLS: [(f32, f32); 5] = [
        (0.0, -0.22),
        (0.11, 0.18),
        (0.24, -0.145),
        (0.40, 0.115),
        (0.59, -0.09),
    ];
    let x = (input + 0.55 * (2.0 * symmetry - 1.0) * (1.0 - input * input)).clamp(-1.0, 1.0);
    let recurrence = 4.0 * x * x - 2.0;
    let mut previous = x;
    let mut harmonic = x * (4.0 * x * x - 3.0);
    let mut folded = x * (1.0 - 0.75 * drive);
    for (onset, gain) in CELLS {
        folded += (drive - onset).max(0.0) * (gain / (1.0 - onset)) * harmonic;
        (harmonic, previous) = (recurrence * harmonic - previous, harmonic);
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::cosine;
    use crate::MachineKind;

    #[test]
    fn modulation_can_reverse_the_carrier_mid_cycle() {
        let mut voice = Voice::default();
        let mut params = MachineKind::WestCoastLpg.spec().default_params();
        params[3] = 1.0;
        let mut bounds = [0.0_f32; 2];
        for i in 0..1024 {
            let phase = voice.carrier;
            voice.step(220.0, 48_000.0, true, (i == 0).then_some(1.0), &params);
            let increment = wrap_turns(voice.carrier - phase + 0.5) - 0.5;
            bounds[0] = bounds[0].min(increment);
            bounds[1] = bounds[1].max(increment);
        }
        assert!(bounds[0] < -0.01 && bounds[1] > 0.02);
    }

    #[test]
    fn extreme_tzfm_and_asymmetric_fold_stay_bounded_and_dc_free() {
        for mode in [0.0, 0.5, 1.0] {
            let mut voice = Voice::default();
            let params = [1.0, 1.0, 1.0, 1.0, 0.5, 0.63, mode, 1.0];
            let mut sum = 0.0;
            let mut energy = 0.0;
            for i in 0..96_000 {
                let output = voice.step(220.0, 48_000.0, true, (i == 0).then_some(1.0), &params);
                assert!((-1.2..=1.2).contains(&output));
                if i >= 48_000 {
                    sum += output;
                    energy += output * output;
                }
            }
            assert!(
                sum.abs() / 48_000.0 < 0.002,
                "mode {mode} mean {}",
                sum / 48_000.0
            );
            assert!(energy > 100.0);
        }
    }

    #[test]
    fn fold_drive_increases_overtones() {
        let mut previous = 0.0;
        for step in 0..=50 {
            let drive = step as f32 / 50.0;
            let mut total = 0.0;
            let mut fundamental = [0.0; 2];
            for i in 0..1024 {
                let phase = i as f32 / 1024.0;
                let sample = fold(sine(phase), drive, 0.5);
                total += sample * sample;
                fundamental[0] += sample * sine(phase);
                fundamental[1] += sample * cosine(phase);
            }
            let fundamental = fundamental[0] * fundamental[0] + fundamental[1] * fundamental[1];
            let ratio = 1.0 - 2.0 * fundamental / (1024.0 * total);
            assert!(
                ratio + 0.000_01 >= previous,
                "drive {drive}: {ratio} vs {previous}"
            );
            previous = ratio;
        }
        assert!(previous > 0.45);
    }

    #[test]
    fn combo_release_loses_brightness_and_level() {
        let mut params = MachineKind::WestCoastLpg.spec().default_params();
        params[0] = 0.56;
        params[1] = 0.75;
        params[3] = 0.0;
        let mut voice = Voice::default();
        let mut windows = [[0.0; 2]; 2];
        let mut previous = 0.0;
        for i in 0..35_000 {
            let output = voice.step(375.0, 48_000.0, i < 5000, (i == 0).then_some(1.0), &params);
            for (window, start) in windows.iter_mut().zip([5200, 25_000]) {
                if (start..start + 2048).contains(&i) {
                    window[0] += output * output;
                    window[1] += (output - previous) * (output - previous);
                }
            }
            previous = output;
        }
        let [early, late] = windows;
        assert!(late[0] < early[0] * 0.25 && late[0] > 1e-6);
        assert!(late[1] / late[0] < early[1] / early[0] * 0.6);
    }
}
