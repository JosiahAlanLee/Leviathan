//! Fixed-point policies and streaming, band-limited rate conversion for BEND.

use crate::voice::sine_cycle;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Rate336,
    Rate224,
    Rate128,
}

impl Mode {
    pub const ALL: [Self; 4] = [Self::Off, Self::Rate336, Self::Rate224, Self::Rate128];

    pub const fn rate(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Rate336 => Some(10_000_000.0 / 336.0),
            Self::Rate224 => Some(10_000_000.0 / 224.0),
            Self::Rate128 => Some(10_000_000.0 / 128.0),
        }
    }
}

#[inline]
fn quantize(x: f32, scale: f32) -> f32 {
    let scaled = x.clamp(-1.0, 1.0 - 1.0 / scale) * scale;
    let truncated = scaled as i32;
    let floored = truncated - i32::from((truncated as f32) > scaled);
    floored as f32 / scale
}

#[inline]
pub(super) fn node(x: f32, fixed: bool) -> f32 {
    if fixed {
        quantize(x, 8_388_608.0)
    } else {
        x
    }
}

#[inline]
pub(super) fn memory(x: f32, fixed: bool) -> f32 {
    if fixed {
        quantize(x, 32_768.0)
    } else {
        x
    }
}

#[inline]
pub(super) fn coefficient(x: f32, fixed: bool) -> f32 {
    // Preserve exactly unity where the algorithm calls for a bypass or Freeze.
    if x == 1.0 {
        x
    } else {
        node(x, fixed)
    }
}

const TAPS: usize = 32;
const PHASES: usize = 64;
const CUTOFFS: [f32; 17] = [
    0.015625, 0.03125, 0.0625, 0.09375, 0.125, 0.15625, 0.1875, 0.21875, 0.25, 0.28125, 0.3125,
    0.34375, 0.375, 0.40625, 0.4375, 0.46875, 0.5,
];
static COEFFICIENTS: [[[f32; TAPS]; PHASES]; CUTOFFS.len()] = coefficients();

const fn sin(turns: f32) -> f32 {
    let turns = turns - (turns as i32) as f32;
    sine_cycle(if turns < 0.0 { turns + 1.0 } else { turns })
}

const fn coefficients() -> [[[f32; TAPS]; PHASES]; CUTOFFS.len()] {
    let mut tables = [[[0.0; TAPS]; PHASES]; CUTOFFS.len()];
    let mut band = 0;
    while band < CUTOFFS.len() {
        let bandwidth = 2.0 * CUTOFFS[band];
        let mut phase = 0;
        while phase < PHASES {
            let fraction = phase as f32 / PHASES as f32;
            let mut sum = 0.0;
            let mut tap = 0;
            while tap < TAPS {
                let x = tap as f32 - (TAPS / 2 - 1) as f32 - fraction;
                let sinc = if x == 0.0 {
                    bandwidth
                } else {
                    sin(x * CUTOFFS[band]) / (core::f32::consts::PI * x)
                };
                let window = 0.5 + 0.5 * sin(0.25 + x / TAPS as f32);
                let y = sinc * window;
                tables[band][phase][tap] = y;
                sum += y;
                tap += 1;
            }
            tap = 0;
            while tap < TAPS {
                tables[band][phase][tap] /= sum;
                tap += 1;
            }
            phase += 1;
        }
        band += 1;
    }
    tables
}

#[derive(Default)]
struct History {
    buffer: [[f32; 2]; TAPS],
    next: usize,
    band: usize,
}

impl History {
    fn new(input: f64, output: f64) -> Self {
        let cutoff = (0.475 * (output / input).min(1.0)) as f32;
        let band = CUTOFFS.iter().rposition(|f| *f <= cutoff).unwrap_or(0);
        Self {
            band,
            ..Self::default()
        }
    }

    fn push(&mut self, frame: [f32; 2]) {
        self.buffer[self.next] = frame;
        self.next = (self.next + 1) % TAPS;
    }

    // Fraction of a source frame after this history window's reference time.
    fn read(&self, fraction: f64) -> [f32; 2] {
        let phase = ((fraction.max(0.0) * PHASES as f64) as usize).min(PHASES - 1);
        let mut output = [0.0; 2];
        for (i, &coef) in COEFFICIENTS[self.band][phase].iter().enumerate() {
            let pair = self.buffer[(self.next + i) % TAPS];
            output[0] += pair[0] * coef;
            output[1] += pair[1] * coef;
        }
        output
    }
}

pub(super) struct Converter {
    input: History,
    output: [History; 2],
    phase: f64,
    ratio: f64,
    dc_in: [[f32; 2]; 2],
    dc_out: [[f32; 2]; 2],
    dc_pole: f32,
}

impl Converter {
    pub(super) fn new(host_rate: f32, chip_rate: f32) -> Self {
        let (host, chip) = (f64::from(host_rate), f64::from(chip_rate));
        let pole = 1.0 / (1.0 + core::f32::consts::TAU * 5.0 / host_rate);
        Self {
            input: History::new(host, chip),
            output: core::array::from_fn(|_| History::new(chip, host)),
            phase: 0.0,
            ratio: chip / host,
            dc_in: [[0.0; 2]; 2],
            dc_out: [[0.0; 2]; 2],
            dc_pole: pole,
        }
    }

    /// Convert one host frame, retaining separate Echo and Space wet signals.
    pub(super) fn step(
        &mut self,
        input: [f32; 2],
        mut process: impl FnMut([f32; 2]) -> [[f32; 2]; 2],
    ) -> [[f32; 2]; 2] {
        self.input.push(input);
        self.phase += self.ratio;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            // Internal ticks arrived within this host interval. The filter
            // history provides one sample of causal margin for these positions.
            let wet = process(self.input.read(1.0 - self.phase / self.ratio));
            for (output, frame) in self.output.iter_mut().zip(wet) {
                output.push(frame);
            }
        }
        let mut wet = self
            .output
            .each_ref()
            .map(|history| history.read(self.phase));
        for (bus, pair) in wet.iter_mut().enumerate() {
            for (ch, value) in pair.iter_mut().enumerate() {
                let filtered = *value - self.dc_in[bus][ch] + self.dc_pole * self.dc_out[bus][ch];
                self.dc_in[bus][ch] = *value;
                self.dc_out[bus][ch] = if filtered.abs() < 1e-20 {
                    0.0
                } else {
                    filtered
                };
                *value = filtered;
            }
        }
        wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturates_q23_and_floors_negative_delay_writes() {
        assert_eq!(node(-2.0, true), -1.0);
        assert_eq!(node(2.0, true), 8_388_607.0 / 8_388_608.0);
        assert_eq!(memory(-0.00001, true), -1.0 / 32768.0);
        assert_eq!(memory(0.00001, true), 0.0);
    }

    #[test]
    fn all_chip_rates_run_at_the_declared_ratio_and_reject_dc() {
        for mode in Mode::ALL.into_iter().skip(1) {
            let rate = mode.rate().unwrap();
            let mut converter = Converter::new(48_000.0, rate);
            let mut count = 0;
            let mut tail = [[0.0; 2]; 2];
            for _ in 0..48_000 {
                tail = converter.step([0.25; 2], |s| {
                    count += 1;
                    [s, [-1.0 / 32768.0; 2]]
                });
            }
            assert!((count as f32 - rate).abs() <= 1.0);
            assert!(tail.iter().flatten().all(|sample| sample.abs() < 1.58e-5));
        }
    }
}
