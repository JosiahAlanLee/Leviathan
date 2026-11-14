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
}

const ONE: u64 = 1 << 32;
const PCM_GAIN: f32 = 1.0 / 32768.0;

fn exp2(octaves: f32) -> f32 {
    let part = crate::v_oct_to_hz(octaves / 3.0) / 261.625_58;
    part * part * part
}

fn log_velocity(velocity: f32) -> f32 {
    if !velocity.is_finite() || velocity <= 0.0 {
        0.0
    } else {
        let steps = (192.0 * (1.0 - velocity.clamp(0.0, 1.0)) + 0.5) as u32;
        exp2(-0.375 * steps as f32 / 6.020_6)
    }
}

impl Hit {
    fn new(
        sample: Sample<'_>,
        params: Slot,
        pitch_oct: f32,
        velocity: f32,
        clock: u8,
        drop_samples: bool,
        rate: f32,
    ) -> Self {
        let chip = CHIP_RATES[usize::from(clock).min(CHIP_RATES.len() - 1)];
        let end_frame = sample.pcm.len() - 1;
        let start_frame = ((end_frame as f32 * params.start) as usize).min(end_frame - 1);
        let start = start_frame as u64 * ONE;
        let end = sample.pcm.len() as u64 * ONE;
        let amp = log_velocity(velocity) * params.level * params.level;
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
            cutoff: 0.015 + 0.985 * params.filter * params.filter,
            amp,
            decay,
            held: 0.0,
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
        if self.amp == 0.0 && self.phase >= 1.0 {
            self.generation = None;
            return 0.0;
        }
        while self.phase >= 1.0 {
            self.phase -= 1.0;
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
                // Keep the final held sample through its whole chip interval.
                self.amp = 0.0;
                break;
            }
        }
        self.phase += self.ratio;
        self.held
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
                    settings.drop_samples,
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

    struct Bank {
        current: Sample<'static>,
        old: Option<Sample<'static>>,
    }
    impl Memory for Bank {
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
                false,
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
}
