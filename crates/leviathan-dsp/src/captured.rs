use leviathan_core::rack_routing::SampleRegion;
use leviathan_core::NUM_LANES;

const FRAC_BITS: u32 = 32;
const ONE: u64 = 1 << FRAC_BITS;
const PCM_GAIN: f32 = 1.0 / 32_768.0;

/// Borrowed from an immutable take owned and retained by the host.
#[derive(Clone, Copy)]
pub struct CaptureSample<'a> {
    pub generation: u64,
    pub rate: u32,
    pub frames: &'a [[i16; 2]],
}

#[derive(Default)]
pub struct CaptureSamples<'a> {
    pub current: Option<CaptureSample<'a>>,
    pub previous: [Option<CaptureSample<'a>>; NUM_LANES],
}

impl<'a> CaptureSamples<'a> {
    pub(super) fn get(&self, generation: Option<u64>) -> Option<CaptureSample<'a>> {
        let generation = generation?;
        self.current
            .into_iter()
            .chain(self.previous.iter().flatten().copied())
            .find(|take| take.generation == generation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTap {
    Lane(u8),
    Bend,
}

#[derive(Clone, Copy)]
pub(super) struct FrameSource<'a> {
    pub(super) take: CaptureSample<'a>,
    pub(super) region: SampleRegion,
    pub(super) size: usize,
    pub(super) frame_byte: Option<u8>,
}

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Playback {
    generation: Option<u64>,
    region: SampleRegion,
    frame_size: usize,
    position: u64,
}

impl Playback {
    pub(super) fn trigger(
        &mut self,
        take: Option<CaptureSample<'_>>,
        region: SampleRegion,
        frame_size: usize,
    ) {
        self.generation = take.map(|take| take.generation);
        self.region = region;
        self.frame_size = frame_size;
        self.position = 0;
    }

    pub(super) fn generation(&self) -> Option<u64> {
        self.generation
    }

    pub(super) fn finish(&mut self) {
        self.generation = None;
    }

    pub(super) fn frames<'a>(
        &self,
        take: CaptureSample<'a>,
        frame_byte: Option<u8>,
    ) -> FrameSource<'a> {
        FrameSource {
            take,
            region: self.region,
            size: self.frame_size,
            frame_byte,
        }
    }

    pub(super) fn step(
        &mut self,
        take: CaptureSample<'_>,
        semitone_ratio: f32,
        rate: f32,
        gate: bool,
    ) -> [f32; 2] {
        if take.frames.is_empty() || self.generation != Some(take.generation) {
            return [0.0; 2];
        }
        let region = self.region;
        let range = region.frames(take.frames.len());
        let frames = &take.frames[range];
        let end = (frames.len() as u64) << FRAC_BITS;
        if self.position >= end {
            self.generation = None;
            return [0.0; 2];
        }
        let wrapping = gate && region.loop_enabled;
        let result = interpolate(frames, self.position, wrapping);
        let delta = (f64::from(semitone_ratio) * f64::from(take.rate) / f64::from(rate)
            * ONE as f64) as u64;
        self.position = self.position.saturating_add(delta.max(1));
        if wrapping {
            self.position %= end;
        } else if self.position >= end {
            self.generation = None;
        }
        result
    }
}

fn interpolate(frames: &[[i16; 2]], position: u64, wrapping: bool) -> [f32; 2] {
    let index = (position >> FRAC_BITS) as usize;
    let next = if index + 1 < frames.len() {
        index + 1
    } else if wrapping {
        0
    } else {
        index
    };
    let fraction = position as u32 as f32 * (1.0 / ONE as f32);
    core::array::from_fn(|ch| {
        let a = f32::from(frames[index][ch]);
        let b = f32::from(frames[next][ch]);
        (a + (b - a) * fraction) * PCM_GAIN
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Scan {
    from: f32,
    progress: u16,
    target: Option<u8>,
}

impl Default for Scan {
    fn default() -> Self {
        Self::new()
    }
}

impl Scan {
    pub(super) const fn new() -> Self {
        Self {
            from: -1.0,
            progress: u16::MAX,
            target: None,
        }
    }

    pub(super) fn read_mono(
        &mut self,
        source: FrameSource<'_>,
        position: f32,
        phase: f32,
        rate: f32,
        invert_bus: bool,
    ) -> f32 {
        let mut source = source;
        if invert_bus {
            source.frame_byte = source.frame_byte.map(|byte| 255 - byte);
        }
        let [left, right] = self.step(source, position, phase, rate);
        0.5 * (left + right)
    }

    /// Read a bounded set of equal-length single-cycle frames at a continuous phase.
    pub(super) fn step(
        &mut self,
        source: FrameSource<'_>,
        position: f32,
        phase: f32,
        rate: f32,
    ) -> [f32; 2] {
        let FrameSource {
            take,
            region,
            size,
            frame_byte,
        } = source;
        if take.frames.is_empty() || size < 2 {
            return [0.0; 2];
        }
        let frames = &take.frames[region.frames(take.frames.len())];
        let count = frames.len() / size;
        if count == 0 {
            return [0.0; 2];
        }
        let last = count - 1;
        let offset =
            ((f64::from(phase) * size as f64 * ONE as f64) as u64).min((size as u64) * ONE - 1);
        let read =
            |index: usize| interpolate(&frames[index * size..(index + 1) * size], offset, true);
        let interpolate_frame = |frame: f32| {
            let a = frame as usize;
            let b = (a + 1).min(last);
            blend(read(a), read(b), frame - a as f32)
        };
        if let Some(byte) = frame_byte {
            let frame = |word| (usize::from(word) * last + 127) / 255;
            let mut target = frame(byte);
            if let Some(previous) = self.target {
                let current = frame(previous);
                if self.progress < u16::MAX {
                    // Finish the current blend before taking the latest request.
                    target = current;
                } else if target != current {
                    self.from = current as f32;
                    self.target = Some(byte);
                    self.progress = 0;
                }
            } else {
                self.target = Some(byte);
                if self.from >= 0.0 {
                    self.progress = 0;
                }
            }
            if self.progress == u16::MAX {
                return read(target);
            }
            let full = f32::from(u16::MAX);
            let fraction = f32::from(self.progress) / full;
            let sample = blend(interpolate_frame(self.from), read(target), fraction);
            self.progress = self
                .progress
                .saturating_add((full / (0.002 * rate)).max(1.0) as u16);
            sample
        } else {
            let frame = position.clamp(0.0, 1.0) * last as f32;
            self.from = frame;
            self.progress = u16::MAX;
            self.target = None;
            interpolate_frame(frame)
        }
    }
}

fn blend(a: [f32; 2], b: [f32; 2], fraction: f32) -> [f32; 2] {
    core::array::from_fn(|ch| a[ch] + (b[ch] - a[ch]) * fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_addresses_long_stereo_and_keeps_the_triggered_generation() {
        let frames = std::vec![[1000, -2000]; 96_000];
        let take = CaptureSample {
            generation: 2,
            rate: 48_000,
            frames: &frames,
        };
        let mut play = Playback::default();
        play.trigger(Some(take), SampleRegion::default(), 256);
        let next = CaptureSample {
            generation: 3,
            ..take
        };
        for _ in 0..96_000 {
            assert_eq!(
                play.step(take, 1.0, 48_000.0, true),
                [1000.0 * PCM_GAIN, -2000.0 * PCM_GAIN]
            );
        }
        assert_eq!(play.generation(), None);
        play.trigger(Some(next), SampleRegion::default(), 256);
        assert_eq!(play.generation(), Some(3));
    }

    #[test]
    fn scanning_ignores_the_tail_and_blends_without_a_phase_reset() {
        let frames = [
            [0; 2],
            [16384; 2],
            [0; 2],
            [32767; 2],
            [0; 2],
            [-16384; 2],
            [32767; 2],
        ];
        let take = CaptureSample {
            generation: 1,
            rate: 48_000,
            frames: &frames,
        };
        let mut scan = Scan::default();
        let mut source = FrameSource {
            take,
            region: SampleRegion::default(),
            size: 2,
            frame_byte: Some(0),
        };
        assert_eq!(scan.step(source, 0.0, 0.5, 48_000.0), [0.5; 2]);
        source.frame_byte = Some(255);
        assert_eq!(scan.step(source, 0.0, 0.5, 48_000.0), [0.5; 2]);
        let blended = scan.step(source, 0.0, 0.5, 48_000.0);
        assert!(blended[0] < 0.5 && blended[0] > 0.48);
        for _ in 0..100 {
            assert!(scan.step(source, 0.0, 0.5, 48_000.0)[0] <= 0.5);
        }
        assert_eq!(scan.step(source, 0.0, 0.5, 48_000.0), [-0.5; 2]);
        assert_eq!(scan.step(source, 0.0, 0.0, 48_000.0), [0.0; 2]);
    }
}
