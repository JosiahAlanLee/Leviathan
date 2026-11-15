//! 4x oversampling for voices whose operators alias at the host rate. The
//! voice runs four sub-steps per host sample; `Decimator4` low-passes and
//! keeps one. Cutoff sits at the host Nyquist; components between 0.5 and
//! 0.6 of the host rate fold above 0.4 of it, out of the passband.

pub const FACTOR: usize = 4;
const TAPS: usize = 96;

/// Blackman-windowed sinc at 1/8 of the 4x rate (the host Nyquist), unity at DC.
static KERNEL: [f32; TAPS] = make_kernel();

const fn sine_turns(turns: f32) -> f32 {
    let wrapped = turns - (turns as i32) as f32;
    let wrapped = if wrapped < 0.0 { wrapped + 1.0 } else { wrapped };
    crate::voice::sine_cycle(wrapped)
}

const fn make_kernel() -> [f32; TAPS] {
    const CUTOFF: f32 = 0.25; // of the 4x Nyquist
    let mut table = [0.0; TAPS];
    let mut sum = 0.0;
    let mut k = 0;
    while k < TAPS {
        let d = k as f32 - (TAPS - 1) as f32 / 2.0;
        let x = CUTOFF * d;
        let sinc = if x > -1.0e-6 && x < 1.0e-6 {
            1.0
        } else {
            sine_turns(0.5 * x) / (core::f32::consts::PI * x)
        };
        let w = k as f32 / (TAPS - 1) as f32;
        let window = 0.42 - 0.5 * sine_turns(w + 0.25) + 0.08 * sine_turns(2.0 * w + 0.25);
        table[k] = sinc * window;
        sum += sinc * window;
        k += 1;
    }
    k = 0;
    while k < TAPS {
        table[k] /= sum;
        k += 1;
    }
    table
}

#[derive(Clone, Debug)]
pub(super) struct Decimator4 {
    history: [f32; TAPS],
    head: usize,
}

impl Default for Decimator4 {
    fn default() -> Self {
        Self { history: [0.0; TAPS], head: 0 }
    }
}

impl Decimator4 {
    /// Push four sub-samples (oldest first) and return one host sample.
    #[inline]
    pub(super) fn process(&mut self, block: [f32; FACTOR]) -> f32 {
        for x in block {
            self.head = (self.head + 1) % TAPS;
            self.history[self.head] = if x.is_finite() { x } else { 0.0 };
        }
        let mut out = 0.0;
        for (k, &h) in KERNEL.iter().enumerate() {
            out += h * self.history[(self.head + TAPS - k) % TAPS];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain(hz: f32) -> f32 {
        // A 4x-rate sine at `hz` (host rate 48 kHz), steady-state output amplitude.
        let mut d = Decimator4::default();
        let mut peak = 0.0f32;
        for n in 0..4_000 {
            let block = core::array::from_fn(|i| {
                let t = (n * FACTOR + i) as f32 / 192_000.0;
                crate::voice::sine_cycle((hz * t).fract())
            });
            let y = d.process(block);
            if n > 200 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn passes_the_audio_band_and_rejects_what_would_fold_into_it() {
        assert!((KERNEL.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for hz in [100.0, 1_000.0, 10_000.0, 16_000.0] {
            let g = gain(hz);
            assert!((g - 1.0).abs() < 0.05, "{hz} Hz passband gain {g}");
        }
        // 30-90 kHz would fold to 18-42 kHz-ish images in the host band without the filter.
        for hz in [30_000.0, 40_000.0, 60_000.0, 90_000.0] {
            let g = gain(hz);
            assert!(g < 0.001, "{hz} Hz stopband gain {g}");
        }
    }
}
