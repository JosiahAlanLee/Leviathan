use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use leviathan_core::NUM_LANES;
use leviathan_dsp::otis_kit::{self, Lane, Slot, MAX_FRAMES, SLOTS};

use crate::capture::Take;
use crate::wav;

#[derive(Debug)]
pub struct KitSample {
    id: u64,
    pub name: String,
    pub rate: u32,
    pub pcm: Vec<i16>,
}

impl KitSample {
    pub fn new(name: String, rate: u32, pcm: Vec<i16>) -> Result<Arc<Self>, String> {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        if name.is_empty()
            || name.len() > 256
            || !(1_000..=768_000).contains(&rate)
            || !(2..=MAX_FRAMES).contains(&pcm.len())
        {
            return Err("Invalid OtisKit sample name, rate or length".into());
        }
        Ok(Arc::new(Self {
            id: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            name,
            rate,
            pcm,
        }))
    }

    pub fn generation(&self) -> u64 {
        self.id
    }

    fn sample(&self) -> otis_kit::Sample<'_> {
        otis_kit::Sample {
            generation: self.id,
            rate: self.rate,
            pcm: &self.pcm,
        }
    }
}

#[derive(Clone, Default)]
pub struct Kit {
    pub samples: [Option<Arc<KitSample>>; SLOTS],
    pub params: [Slot; SLOTS],
    pub lanes: [Lane; NUM_LANES],
}

impl Kit {
    pub fn is_valid(&self) -> bool {
        self.samples
            .iter()
            .flatten()
            .map(|s| s.pcm.len())
            .sum::<usize>()
            <= MAX_FRAMES
            && self.params.iter().all(|slot| slot.is_valid())
            && self.lanes.iter().all(|lane| lane.clock < 3)
    }

    pub fn load_wav(&mut self, slot: usize, path: &Path) -> Result<(), String> {
        let (name, rate, pcm) = wav::load_kit_wav(path)?;
        self.replace(slot, KitSample::new(name, rate, pcm)?)
    }

    pub fn load_folder(&mut self, path: &Path) -> Result<(), String> {
        let mut paths = path
            .read_dir()
            .map_err(|err| err.to_string())?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| err.to_string())?;
        paths.retain(|p| {
            p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))
        });
        paths.sort();
        if paths.is_empty() {
            return Err("No WAV files in this folder".into());
        }
        let mut updated = self.clone();
        for slot in &mut updated.samples[..paths.len().min(SLOTS)] {
            *slot = None;
        }
        for (slot, path) in paths.iter().take(SLOTS).enumerate() {
            updated.load_wav(slot, path)?;
        }
        *self = updated;
        Ok(())
    }

    pub fn load_factory(&mut self) {
        let family = [
            ("Kick", 53.0, 0.10, 0.10),
            ("Snare", 173.0, 0.30, 0.042),
            ("Closed hat", 4813.0, 0.83, 0.011),
            ("Open hat", 3713.0, 0.68, 0.052),
            ("Low tom", 73.0, 0.04, 0.085),
            ("Mid tom", 118.0, 0.05, 0.063),
            ("Rim", 846.0, 0.31, 0.016),
            ("Metal", 1337.0, 0.30, 0.048),
        ];
        self.samples = std::array::from_fn(|slot| {
            let (name, hz, noise_mix, decay) = family[slot];
            let mut rnd = 0x941b_3185 ^ (slot as u32 + 1);
            let mut last = 0.0;
            let pcm = (0..(44_100.0 * decay * 6.0) as usize)
                .map(|i| {
                    let t = i as f32 / 44_100.0;
                    rnd ^= rnd << 13;
                    rnd ^= rnd >> 17;
                    rnd ^= rnd << 5;
                    let noise = rnd as i32 as f32 / 2_147_483_648.0;
                    let high = noise - 0.75 * last;
                    last = noise;
                    let phase =
                        std::f32::consts::TAU * (hz * t + 0.75 * (1.0 - (-t * 120.0).exp()));
                    let wave = phase.sin() * (1.0 - noise_mix) + 0.65 * high * noise_mix;
                    (wave * (-t / decay).exp() * 26_000.0) as i16
                })
                .collect();
            Some(KitSample::new(name.into(), 44_100, pcm).expect("factory sample is bounded"))
        });
        self.params = [Slot::default(); SLOTS];
    }

    pub fn capture_into(&mut self, slot: usize, take: &Take) -> Result<(), String> {
        let pcm = take
            .frames
            .iter()
            .map(|&[l, r]| ((i32::from(l) + i32::from(r)) / 2) as i16)
            .collect();
        let sample = KitSample::new(format!("Capture {}", slot + 1), take.rate, pcm)?;
        self.replace(slot, sample)
    }

    pub fn replace(&mut self, slot: usize, sample: Arc<KitSample>) -> Result<(), String> {
        let used: usize = self
            .samples
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != slot)
            .filter_map(|(_, existing)| existing.as_ref().map(|s| s.pcm.len()))
            .sum();
        if used + sample.pcm.len() > MAX_FRAMES {
            return Err("OtisKit exceeds 1,048,320 mono frames".into());
        }
        self.samples[slot] = Some(sample);
        Ok(())
    }
}

impl otis_kit::Memory for Kit {
    fn current(&self, slot: usize) -> Option<otis_kit::Sample<'_>> {
        self.samples[slot].as_ref().map(|s| s.sample())
    }

    fn previous(&self, _: usize, _: usize, _: u64) -> Option<otis_kit::Sample<'_>> {
        None
    }
}

pub type Readers = [[[Option<u64>; 2]; SLOTS]; NUM_LANES];

#[derive(Default)]
pub struct KitBank {
    current: Kit,
    previous: [[[Option<Arc<KitSample>>; 2]; SLOTS]; NUM_LANES],
}

impl KitBank {
    pub fn install(&mut self, kit: &Kit, used: Readers) {
        let replaced = std::mem::replace(&mut self.current, kit.clone());
        for (lane, slots) in used.into_iter().enumerate() {
            for (slot, readers) in slots.into_iter().enumerate() {
                let previous = &mut self.previous[lane][slot];
                *previous = readers.map(|id| {
                    let id = id?;
                    if self.current.samples[slot]
                        .as_ref()
                        .is_some_and(|s| s.id == id)
                    {
                        return None;
                    }
                    replaced.samples[slot]
                        .iter()
                        .chain(previous.iter().flatten())
                        .find(|s| s.id == id)
                        .cloned()
                });
            }
        }
    }

    pub fn release_unused(&mut self, used: Readers) {
        for (slots, used) in self.previous.iter_mut().zip(used) {
            for (samples, readers) in slots.iter_mut().zip(used) {
                for sample in samples {
                    if sample
                        .as_ref()
                        .is_some_and(|s| !readers.contains(&Some(s.id)))
                    {
                        *sample = None;
                    }
                }
            }
        }
    }
}

impl otis_kit::Memory for KitBank {
    fn current(&self, slot: usize) -> Option<otis_kit::Sample<'_>> {
        self.current.current(slot)
    }

    fn previous(&self, lane: usize, slot: usize, generation: u64) -> Option<otis_kit::Sample<'_>> {
        self.previous[lane][slot]
            .iter()
            .flatten()
            .find(|s| s.id == generation)
            .map(|s| s.sample())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otis_kit::Memory;

    #[test]
    fn replacing_all_slots_retains_both_live_generations_until_readers_finish() {
        let make_kit = |level| Kit {
            samples: std::array::from_fn(|slot| {
                Some(KitSample::new(format!("Slot {slot}"), 48_000, vec![level; 4]).unwrap())
            }),
            ..Kit::default()
        };
        let first = make_kit(101);
        let second = make_kit(202);
        let third = make_kit(303);
        let mut bank = KitBank::default();
        let mut readers = [[[None; 2]; SLOTS]; NUM_LANES];
        bank.install(&first, readers);
        for slot in 0..SLOTS {
            readers[0][slot][1] = first.current(slot).map(|sample| sample.generation);
        }
        bank.install(&second, readers);
        for slot in 0..SLOTS {
            readers[0][slot][0] = second.current(slot).map(|sample| sample.generation);
        }
        bank.install(&third, readers);
        for slot in 0..SLOTS {
            let [current_hit, fade] = readers[0][slot].map(Option::unwrap);
            assert_eq!(bank.current(slot).unwrap().pcm, [303; 4]);
            assert_eq!(bank.previous(0, slot, current_hit).unwrap().pcm, [202; 4]);
            assert_eq!(bank.previous(0, slot, fade).unwrap().pcm, [101; 4]);
        }
        for slot in &mut readers[0] {
            slot[1] = None;
        }
        bank.release_unused(readers);
        for slot in 0..SLOTS {
            let first_id = first.current(slot).unwrap().generation;
            assert!(bank.previous(0, slot, first_id).is_none());
            assert!(bank
                .previous(0, slot, readers[0][slot][0].unwrap())
                .is_some());
        }
    }
}
