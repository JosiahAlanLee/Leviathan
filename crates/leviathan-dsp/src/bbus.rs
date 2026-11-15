use crate::dac::R2r;
use crate::voice::{approach, phase_step, StateVariable};
use leviathan_core::{CvFrame, SOUND_PARAMS};

const PHASE_SCALE: f64 = 4_294_967_296.0;
const SLOT_BITS: u32 = 28;
const SLOT_MASK: u8 = 15;
const SEED: u8 = 0xa5;
/// The cycle-lock reload, Parallel Sum's default seed. It also keeps the
/// fundamental strongest at the default macros (0xa5 favours the octave).
const LOCK_SEED: u8 = 0x00;

/// A tracker byte is current on each sample. A connected clocked source
/// publishes on rising edges and holds its last byte until disconnection.
#[derive(Clone, Copy, Debug)]
pub enum ByteBusSource {
    Standalone,
    Current,
    Clocked,
}

#[derive(Default)]
pub struct ByteBusReceiver {
    held: Option<u8>,
    clock_high: bool,
}

impl ByteBusReceiver {
    /// A clocked source returns its held word after the first rising edge.
    pub fn step(&mut self, source: ByteBusSource, volts: &[f32; 9]) -> Option<u8> {
        match source {
            ByteBusSource::Standalone => {
                *self = Self::default();
                None
            }
            ByteBusSource::Current => {
                *self = Self::default();
                Some(decode_word(volts))
            }
            ByteBusSource::Clocked => {
                let high = volts[8] > 5.0;
                if high && !self.clock_high {
                    self.held = Some(decode_word(volts));
                }
                self.clock_high = high;
                self.held
            }
        }
    }
}

#[inline]
pub(super) fn tracker_word(cv: &CvFrame) -> Option<u8> {
    (cv.run_v > 2.5).then(|| decode_word(&cv.bytebus_v))
}

fn decode_word(volts: &[f32; 9]) -> u8 {
    volts[..8]
        .iter()
        .enumerate()
        .fold(0, |word, (bit, &voltage)| {
            word | (u8::from(voltage > 5.0) << bit)
        })
}

#[derive(Clone, Debug)]
pub(super) struct Voice {
    phase: u32,
    sub_phase: u32,
    amp: f32,
    register: u8,
    fallback: u8,
    held_fir: f32,
    dac: R2r,
    filter: StateVariable,
    dc: f32,
}

const _: () = assert!(core::mem::size_of::<Voice>() <= 96);

impl Default for Voice {
    fn default() -> Self {
        Self {
            phase: 0,
            sub_phase: 0,
            amp: 0.0,
            register: SEED,
            fallback: 0xb8,
            held_fir: 0.0,
            dac: R2r::default(),
            filter: StateVariable::default(),
            dc: 0.0,
        }
    }
}

impl Voice {
    #[inline]
    pub(super) fn step(
        &mut self,
        hz: f32,
        rate: f32,
        gate: bool,
        trigger: Option<f32>,
        word: Option<u8>,
        params: &[f32; SOUND_PARAMS],
        cycle_lock: bool,
    ) -> f32 {
        let [decay, mask, tilt, skew, hold, cutoff, color, sub] = *params;
        if let Some(velocity) = trigger {
            self.amp = velocity;
            self.fallback = (self.fallback >> 1) ^ (0xb8 * (self.fallback & 1));
        }
        let time = if gate {
            0.06 + 2.2 * decay * decay
        } else {
            0.018 + 0.85 * decay * decay
        };
        self.amp = approach(self.amp, 0.0, time, rate);
        if self.amp == 0.0 {
            return 0.0;
        }

        let word = word.unwrap_or(self.fallback);
        let increment = (f64::from(phase_step(hz, rate)) * PHASE_SCALE) as u32;
        let mut slot = (self.phase >> SLOT_BITS) as u8;
        self.phase = self.phase.wrapping_add(increment);
        self.sub_phase = self.sub_phase.wrapping_add(increment >> 2);
        let destination = (self.phase >> SLOT_BITS) as u8;

        if slot != destination {
            let sense = u8::from(mask >= 0.5);
            let morph = 2.0 * mask - f32::from(sense);
            let feedback = word ^ (0xb8 ^ ((morph * 255.0) as u8).rotate_left(1));
            // The phase increment is limited below half a cycle, so at most
            // eight slots are crossed per host sample, including at wrap.
            while slot != destination {
                slot = (slot + 1) & SLOT_MASK;
                if cycle_lock && slot == 0 {
                    // Restart the register every note cycle so its sixteen
                    // steps are one period, as when Parallel Sum's cycle is
                    // the pitch. Free running (grit) lets the feedback wander
                    // for 15-255 cycles, which reads as a noisy comb.
                    self.register = LOCK_SEED;
                }
                self.register = shift(self.register, feedback, sense, word, slot);
            }
            self.held_fir = parallel_sum(self.register, tilt, color);
        }

        let converted = self.dac.step(self.held_fir, rate, hold, skew, color);
        let sub_octave = |phase| {
            if phase & 0x8000_0000_u32 == 0 {
                -1.0
            } else {
                1.0
            }
        };
        let sub_signal = 0.65 * sub_octave(self.sub_phase << 1) + 0.35 * sub_octave(self.sub_phase);
        let combined = converted * (1.0 - 0.45 * sub) + 0.55 * sub * sub_signal;
        self.dc = approach(self.dc, combined, 0.012, rate);
        let frequency = 120.0 + 15_000.0 * cutoff * cutoff;
        let g = crate::voice::rate_g(
            (core::f32::consts::PI * frequency / crate::voice::REFERENCE_RATE).clamp(0.005, 0.8),
            rate,
        );
        let filtered = self.filter.step(combined - self.dc, g, 0.72, 0.0).low;
        crate::fast_tanh(filtered) * self.amp
    }
}

#[inline]
fn shift(register: u8, feedback: u8, sense: u8, word: u8, slot: u8) -> u8 {
    let parity = (register & feedback).count_ones() as u8 & 1;
    let inject = (slot >> 3) ^ ((word >> (slot & 7)) & 1);
    (register << 1) | (parity ^ sense ^ inject)
}

fn parallel_sum(register: u8, tilt: f32, color: f32) -> f32 {
    const WEIGHTS: [[f32; 8]; 4] = [
        [1.0; 8],
        [1.0, 0.8, 0.5, 0.2, -0.2, -0.5, -0.8, -1.0],
        [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0],
        [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0],
    ];
    let position = tilt * 3.0;
    let segment = (position as usize).min(2);
    let mix = position - segment as f32;
    let mut sum = 0.0;
    for (bit, (&a, &b)) in WEIGHTS[segment]
        .iter()
        .zip(&WEIGHTS[segment + 1])
        .enumerate()
    {
        let bipolar = 2.0 * f32::from((register >> bit) & 1) - 1.0;
        sum += bipolar * (a + (b - a) * mix);
    }
    let stage_volts = 1.25 + 4.75 * color * color;
    (sum * stage_volts / 10.5).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MachineKind;
    use leviathan_core::encode_bytebus_frame;

    #[test]
    fn current_words_follow_data_and_clocked_input_holds_zero_and_repeats() {
        let mut rx = ByteBusReceiver::default();
        let check = |rx: &mut ByteBusReceiver, source, word, high| {
            rx.step(source, &encode_bytebus_frame(word, high))
        };
        assert_eq!(check(&mut rx, ByteBusSource::Clocked, 0xa4, false), None);
        assert_eq!(check(&mut rx, ByteBusSource::Clocked, 0, true), Some(0));
        assert_eq!(check(&mut rx, ByteBusSource::Clocked, 0xa4, true), Some(0));
        assert_eq!(check(&mut rx, ByteBusSource::Clocked, 0xa4, false), Some(0));
        assert_eq!(
            check(&mut rx, ByteBusSource::Clocked, 0xa4, true),
            Some(0xa4)
        );
        assert_eq!(
            check(&mut rx, ByteBusSource::Clocked, 0x00, false),
            Some(0xa4)
        );
        assert_eq!(
            check(&mut rx, ByteBusSource::Clocked, 0x00, true),
            Some(0x00)
        );
        assert_eq!(check(&mut rx, ByteBusSource::Standalone, 0, false), None);
        assert_eq!(check(&mut rx, ByteBusSource::Clocked, 0, false), None);
        assert_eq!(
            check(&mut rx, ByteBusSource::Current, 0x11, false),
            Some(0x11)
        );
        assert_eq!(check(&mut rx, ByteBusSource::Current, 0, false), Some(0));
    }

    #[test]
    fn high_notes_process_each_crossed_slot_including_wrap() {
        let mut voice = Voice::default();
        let params = MachineKind::ByteBusR2r.spec().default_params();
        voice.phase = 0xe800_0000;
        let word = 0x39;
        let mask = 0xb8 ^ ((2.0 * params[1] * 255.0) as u8).rotate_left(1) ^ word;
        let mut expected = voice.register;
        let mut locked = voice.clone();
        voice.step(7_500.0, 48_000.0, true, Some(1.0), Some(word), &params, false);
        assert_eq!(voice.phase >> SLOT_BITS, 1);
        for slot in [15, 0, 1] {
            expected = shift(expected, mask, 0, word, slot);
        }
        assert_eq!(voice.register, expected);

        // Cycle lock restarts from the seed as the wrap is crossed.
        locked.step(7_500.0, 48_000.0, true, Some(1.0), Some(word), &params, true);
        assert_eq!(locked.register, shift(shift(LOCK_SEED, mask, 0, word, 0), mask, 0, word, 1));
    }

    #[test]
    fn cycle_lock_repeats_every_note_cycle_and_grit_does_not() {
        let params = MachineKind::ByteBusR2r.spec().default_params();
        // 750 Hz at 48 kHz: 64 samples per note cycle, four per slot.
        for (lock, periodic) in [(true, true), (false, false)] {
            for word in [Some(0x01), Some(0x00), None] {
                let mut voice = Voice::default();
                let registers: Vec<u8> = (0..64 * 40)
                    .map(|frame| {
                        voice.step(750.0, 48_000.0, true, (frame == 0).then_some(1.0), word, &params, lock);
                        voice.register
                    })
                    .collect();
                let repeats = (64..registers.len() - 64).all(|n| registers[n] == registers[n + 64]);
                assert_eq!(repeats, periodic, "lock {lock} word {word:?}");
            }
        }
    }

    #[test]
    fn parallel_sum_cancels_for_complementary_words() {
        for tilt in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for word in 0..=255 {
                assert_eq!(
                    parallel_sum(word, tilt, 1.0),
                    -parallel_sum(!word, tilt, 1.0)
                );
            }
        }
    }

    #[test]
    fn fixed_zero_standalone_and_other_bus_words_produce_distinct_bounded_signals() {
        for lock in [false, true] {
        let params = MachineKind::ByteBusR2r.spec().default_params();
        let mut voices = [Voice::default(), Voice::default(), Voice::default()];
        let mut energy = [0.0; 3];
        let mut delta = [0.0; 2];
        for frame in 0..4_096 {
            let samples: [f32; 3] = core::array::from_fn(|i| {
                voices[i].step(
                    220.0,
                    48_000.0,
                    true,
                    (frame == 0).then_some(0.95),
                    [Some(0), None, Some(0x71)][i],
                    &params,
                    lock,
                )
            });
            for (sum, sample) in energy.iter_mut().zip(samples) {
                assert!((-1.0..=1.0).contains(&sample));
                *sum += sample * sample;
            }
            delta[0] += (samples[0] - samples[1]).abs();
            delta[1] += (samples[0] - samples[2]).abs();
        }
        assert!(energy.iter().all(|&sum| sum > 0.25), "{energy:?}");
        assert!(delta.iter().all(|&sum| sum > 20.0), "{delta:?}");
        }
    }
}
