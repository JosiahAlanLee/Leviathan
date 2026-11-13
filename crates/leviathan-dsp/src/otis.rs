use crate::voice::{approach, choice, sine_cycle};
use crate::wrap_turns;
use leviathan_core::SOUND_PARAMS;

const TABLES: usize = 4;
const FRAMES: usize = 16;
const SAMPLES: usize = 128;
const FRACTION_BITS: u32 = 12;
const FRACTION: u32 = 1 << FRACTION_BITS;
const CYCLE: u32 = SAMPLES as u32 * FRACTION;

// Compile-time, single-cycle Fourier banks. All six lanes share these samples.
static ROM: [[[i16; SAMPLES]; FRAMES]; TABLES] = make_rom();

const fn rom_sine(index: usize) -> f32 {
    sine_cycle((index % SAMPLES) as f32 / SAMPLES as f32)
}

const fn make_rom() -> [[[i16; SAMPLES]; FRAMES]; TABLES] {
    let mut rom = [[[0; SAMPLES]; FRAMES]; TABLES];
    let mut table = 0;
    while table < TABLES {
        let mut frame = 0;
        while frame < FRAMES {
            let scan = frame as f32 / (FRAMES - 1) as f32;
            let mut sample = 0;
            while sample < SAMPLES {
                if table == 3 {
                    rom[table][frame][sample] = sync_sample(frame, sample);
                    sample += 1;
                    continue;
                }
                let mut wave = 0.0;
                let mut harmonic = 1;
                while harmonic <= 16 {
                    let n = harmonic as f32;
                    let weight = match table {
                        0 => {
                            let offset = (n - (2.0 + 12.0 * scan)) * 0.44;
                            1.0 / (n * (1.0 + offset * offset))
                        }
                        1 => match harmonic {
                            1 => 0.45,
                            3 => 0.35 * (1.0 - scan),
                            5 => 0.22,
                            8 => 0.31 * scan,
                            13 => 0.18 * scan,
                            _ => 0.0,
                        },
                        _ => rom_sine(harmonic * (9 + frame)) / n,
                    };
                    let phase = if table == 2 {
                        // Center the pulse at half its width and omit its DC term.
                        harmonic * (sample + SAMPLES - (9 + frame)) + SAMPLES / 4
                    } else {
                        harmonic * sample
                    };
                    wave += weight * rom_sine(phase);
                    harmonic += 1;
                }
                // Gentle normalization leaves headroom while retaining differences between frames.
                rom[table][frame][sample] = (29_000.0 * wave / (1.0 + wave.abs())) as i16;
                sample += 1;
            }
            frame += 1;
        }
        table += 1;
    }
    rom
}

const fn sync_sample(frame: usize, sample: usize) -> i16 {
    // A slave sine restarted once per fundamental cycle. Integrate its sine
    // and cosine coefficients and keep sixteen harmonics of the reset wave.
    let ratio = 1.0 + frame as f32 * 0.375;
    let fraction = (frame * 3) % 8 * (SAMPLES / 8);
    if fraction == 0 {
        return (22_000.0 * rom_sine(sample * ratio as usize)) as i16;
    }
    let edge_sin = rom_sine(fraction);
    let edge_cos = rom_sine(fraction + SAMPLES / 4);
    let mut wave = 0.0;
    let mut harmonic = 1;
    while harmonic <= 16 {
        let n = harmonic as f32;
        let denominator = core::f32::consts::PI * (ratio * ratio - n * n);
        let cosine = ratio * (1.0 - edge_cos) / denominator;
        let sine = n * edge_sin / denominator;
        let phase = harmonic * sample;
        wave += cosine * rom_sine(phase + SAMPLES / 4) + sine * rom_sine(phase);
        harmonic += 1;
    }
    (22_000.0 * wave.clamp(-1.4, 1.4)) as i16
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voice {
    phase: u32,
    scan: f32,
    frame: f32,
    amp: f32,
    filtered: [f32; 2],
    held_sample: f32,
    hold: u8,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 128);
const _: () = assert!(core::mem::size_of_val(&ROM) == 16 * 1024);

impl Voice {
    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        sample_rate: f32,
        trigger: Option<f32>,
        params: &[f32; SOUND_PARAMS],
    ) -> f32 {
        let [decay, table, position, depth, k1, k2, grit, loop_mode] = *params;
        if let Some(velocity) = trigger {
            self.amp = velocity;
            self.phase = 0;
            self.scan = 0.0;
            self.frame = position;
            self.hold = 0;
        }
        self.amp = approach(self.amp, 0.0, 0.016 + 2.2 * decay * decay, sample_rate);
        if self.amp == 0.0 {
            return 0.0;
        }

        let scan_step = (0.55 + 3.2 * depth + 1.3 * self.amp) / sample_rate;
        let scan = match choice(loop_mode, 3) {
            0 => {
                self.scan = wrap_turns(self.scan + scan_step);
                self.scan
            }
            1 => {
                self.scan = wrap_turns(self.scan + 0.5 * scan_step);
                1.0 - (2.0 * self.scan - 1.0).abs()
            }
            _ => {
                self.scan = (self.scan + scan_step).min(1.0);
                self.scan
            }
        };
        let target = (position + depth * scan * (0.65 + 0.35 * self.amp)).clamp(0.0, 1.0);
        self.frame = approach(self.frame, target, 0.004, sample_rate);
        let increment = ((hz / sample_rate).clamp(0.0, 0.45) * CYCLE as f32 + 0.5) as u32;
        self.phase = advance(self.phase, increment);
        // Grit lowers the scanner update rate and address precision. The
        // accumulator still advances every sample so pitch stays independent.
        if self.hold == 0 {
            self.hold = (grit * grit * 15.0) as u8;
            let discarded = (grit * 12.0) as u32;
            let phase = (self.phase >> discarded) << discarded;
            let frame = (self.frame * ((FRAMES - 1) as u32 * FRACTION) as f32 + 0.5) as u32;
            self.held_sample = scan_sample(choice(table, TABLES), frame, phase) * (1.0 / 32_768.0);
        } else {
            self.hold -= 1;
        }
        let input = self.held_sample - 0.88 * k2 * self.filtered[1];
        let first = 0.012 + 0.78 * k1 * k1;
        let second = 0.04 + 0.68 * k1;
        self.filtered[0] += first * (input - self.filtered[0]);
        self.filtered[1] += second * (self.filtered[0] - self.filtered[1]);
        (self.filtered[1] * self.amp).clamp(-1.0, 1.0)
    }
}

#[inline]
fn advance(phase: u32, increment: u32) -> u32 {
    phase.wrapping_add(increment) & (CYCLE - 1)
}

#[inline]
fn scan_sample(table: usize, frame: u32, phase: u32) -> f32 {
    let index = (phase >> FRACTION_BITS) as usize;
    let next = (index + 1) & (SAMPLES - 1);
    let phase_frac = (phase & (FRACTION - 1)) as f32 / FRACTION as f32;
    let lower = (frame >> FRACTION_BITS) as usize;
    let upper = (lower + 1).min(FRAMES - 1);
    let frame_frac = (frame & (FRACTION - 1)) as f32 / FRACTION as f32;
    let pair = [lower, upper].map(|row| {
        let a = f32::from(ROM[table][row][index]);
        let b = f32::from(ROM[table][row][next]);
        a + phase_frac * (b - a)
    });
    pair[0] + frame_frac * (pair[1] - pair[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MachineKind;

    #[test]
    fn phase_wrap_matches_integer_modulus_over_long_runs() {
        for increment in [1, 1297, CYCLE / 2 + 61, u32::MAX] {
            let mut phase = CYCLE - 11;
            for i in 1..=150_000 {
                phase = advance(phase, increment);
                let exact = (u64::from(CYCLE - 11) + i * u64::from(increment)) % u64::from(CYCLE);
                assert_eq!(u64::from(phase), exact);
            }
        }
    }

    #[test]
    fn all_banks_interpolate_both_sides_of_frame_and_sample_boundaries() {
        for table in 0..TABLES {
            for frame in 0..FRAMES - 1 {
                for sample in 0..SAMPLES {
                    let row = frame as u32 * FRACTION;
                    let phase = sample as u32 * FRACTION;
                    let a = scan_sample(table, row, phase);
                    let b = scan_sample(table, row + FRACTION, phase);
                    let mid = scan_sample(table, row + FRACTION / 2, phase);
                    assert!((mid - 0.5 * (a + b)).abs() < 0.005);
                    assert!((scan_sample(table, row + FRACTION - 1, phase) - b).abs() < 16.1);
                    let edge = scan_sample(table, row, advance(phase, FRACTION - 1));
                    assert!(
                        (edge - scan_sample(table, row, advance(phase, FRACTION))).abs() < 16.1
                    );
                }
            }
        }
    }

    #[test]
    fn scans_are_bounded_and_one_shot_stops_at_the_end() {
        let mut params = MachineKind::OtisTranswave.spec().default_params();
        params[0] = 1.0;
        params[3] = 1.0;
        params[5] = 1.0;
        params[6] = 1.0;
        for mode in [0.0, 0.5, 1.0] {
            params[7] = mode;
            let mut voice = Voice::default();
            let mut energy = 0.0;
            for i in 0..48_000 {
                let sample = voice.step(440.0, 48_000.0, (i == 0).then_some(1.0), &params);
                assert!((-1.0..=1.0).contains(&sample));
                energy += sample * sample;
            }
            assert!(energy > 10.0);
            assert_eq!(voice.scan == 1.0, mode == 1.0);
        }
    }
}
