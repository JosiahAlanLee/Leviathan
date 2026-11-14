use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use leviathan_core::NUM_LANES;
use leviathan_dsp::{CaptureSample, CaptureSamples, CaptureTap};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub const MAX_CAPTURE_FRAMES: usize = 192_000;
pub const MAX_CAPTURE_BEATS: u8 = 16;

#[derive(Debug)]
pub struct Take {
    generation: u64,
    pub rate: u32,
    pub frames: Vec<[i16; 2]>,
}

impl Take {
    pub fn new(rate: u32, frames: Vec<[i16; 2]>) -> Option<Arc<Self>> {
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        if rate == 0 || frames.is_empty() || frames.len() > MAX_CAPTURE_FRAMES {
            return None;
        }
        Some(Arc::new(Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            rate,
            frames,
        }))
    }

    pub fn sample(&self) -> CaptureSample<'_> {
        CaptureSample {
            generation: self.generation,
            rate: self.rate,
            frames: &self.frames,
        }
    }
}

/// Audio-side references. The UI retains the final Arc of every installed take.
#[derive(Default)]
pub struct TakeBank {
    current: Option<Arc<Take>>,
    previous: [Option<Arc<Take>>; NUM_LANES],
}

impl TakeBank {
    pub fn install(&mut self, current: Option<Arc<Take>>, used: [Option<u64>; NUM_LANES]) {
        let old = std::mem::replace(&mut self.current, current);
        self.previous = used.map(|generation| {
            let generation = generation?;
            if self
                .current
                .as_ref()
                .is_some_and(|take| take.generation == generation)
            {
                return None;
            }
            old.iter()
                .chain(self.previous.iter().flatten())
                .find(|take| take.generation == generation)
                .cloned()
        });
    }

    pub fn release_unused(&mut self, used: [Option<u64>; NUM_LANES]) {
        for slot in &mut self.previous {
            if slot
                .as_ref()
                .is_some_and(|take| !used.contains(&Some(take.generation)))
            {
                *slot = None;
            }
        }
    }

    pub fn samples(&self) -> CaptureSamples<'_> {
        CaptureSamples {
            current: self.current.as_deref().map(Take::sample),
            previous: self
                .previous
                .each_ref()
                .map(|take| take.as_deref().map(Take::sample)),
        }
    }
}

pub struct Request {
    project: u64,
    tap: CaptureTap,
    beats: u8,
    frames: Vec<[i16; 2]>,
    written: usize,
}

pub enum Outcome {
    Complete(u32),
    Cancelled,
    Full,
}

pub struct CaptureResult {
    pub project: u64,
    pub outcome: Outcome,
    pub frames: Vec<[i16; 2]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Idle,
    Armed,
    Recording,
}

pub struct CapturePort {
    request: Producer<Request>,
    result: Consumer<CaptureResult>,
    in_flight: bool,
}

impl CapturePort {
    pub fn busy(&self) -> bool {
        self.in_flight
    }

    /// Allocate the recording buffer on the UI thread before queuing the arm command.
    pub fn request(&mut self, project: u64, tap: CaptureTap, beats: u8) -> bool {
        if self.in_flight || self.request.is_abandoned() {
            return false;
        }
        let mut frames = Vec::with_capacity(MAX_CAPTURE_FRAMES);
        // Touch each page on the UI thread. The callback only overwrites existing frames.
        frames.resize_with(MAX_CAPTURE_FRAMES, || std::hint::black_box([0; 2]));
        let sent = self
            .request
            .push(Request {
                project,
                tap,
                beats: beats.clamp(1, MAX_CAPTURE_BEATS),
                frames,
                written: 0,
            })
            .is_ok();
        self.in_flight = sent;
        sent
    }

    pub fn poll(&mut self) -> Option<CaptureResult> {
        let result = self.result.pop().ok()?;
        self.in_flight = false;
        Some(result)
    }
}

pub struct Recorder {
    request: Consumer<Request>,
    result: Producer<CaptureResult>,
    active: Option<Request>,
    pending: Option<CaptureResult>,
    start_beat: Option<f64>,
    end_beat: Option<f64>,
}

impl Recorder {
    pub fn channel() -> (CapturePort, Self) {
        let (request_tx, request_rx) = RingBuffer::new(1);
        let (result_tx, result_rx) = RingBuffer::new(1);
        (
            CapturePort {
                request: request_tx,
                result: result_rx,
                in_flight: false,
            },
            Self {
                request: request_rx,
                result: result_tx,
                active: None,
                pending: None,
                start_beat: None,
                end_beat: None,
            },
        )
    }

    pub fn arm(&mut self, project: u64) {
        if self.active.is_some() || self.pending.is_some() {
            return;
        }
        self.active = self.request.pop().ok();
        if self
            .active
            .as_ref()
            .is_some_and(|request| request.project != project)
        {
            self.cancel();
        }
    }

    pub fn cancel(&mut self) {
        self.finish(Outcome::Cancelled);
    }

    pub fn stage(&self) -> Stage {
        if self.end_beat.is_some() {
            Stage::Recording
        } else if self.active.is_some() {
            Stage::Armed
        } else {
            Stage::Idle
        }
    }

    pub fn beats_left(&self, beat: f64) -> Option<f64> {
        self.end_beat.or(self.start_beat).map(|target| (target - beat).max(0.0))
    }

    pub fn tap(
        &mut self,
        beat: f64,
        previous_beat: Option<f64>,
        playing: bool,
        rate: u32,
    ) -> Option<CaptureTap> {
        let request = self.active.as_ref()?;
        if !playing {
            if self.start_beat.is_some() {
                self.cancel();
            }
            return None;
        }
        let start = *self.start_beat.get_or_insert_with(|| {
            let boundary = beat as u64 as f64;
            if previous_beat.is_some_and(|previous| previous < boundary) {
                boundary
            } else {
                beat.ceil()
            }
        });
        if beat < start {
            return None;
        }
        let end = *self
            .end_beat
            .get_or_insert(start + f64::from(request.beats));
        if beat >= end {
            self.finish(Outcome::Complete(rate));
            return None;
        }
        if request.written == request.frames.len() {
            self.finish(Outcome::Full);
            return None;
        }
        Some(request.tap)
    }

    pub fn push(&mut self, sample: [f32; 2]) {
        if let Some(request) = &mut self.active {
            // The tap checks the frame bound before rendering this sample.
            request.frames[request.written] = sample.map(|value| (value * 32768.0) as i16);
            request.written += 1;
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        if let Some(mut request) = self.active.take() {
            self.start_beat = None;
            self.end_beat = None;
            request.frames.truncate(request.written);
            self.pending = Some(CaptureResult {
                project: request.project,
                outcome,
                frames: request.frames,
            });
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        if let Some(result) = self.pending.take() {
            if let Err(PushError::Full(result)) = self.result.push(result) {
                self.pending = Some(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_uses_half_open_beats_and_preserves_stereo_across_blocks() {
        let (mut port, mut recorder) = Recorder::channel();
        assert!(port.request(7, CaptureTap::Bend, 4));
        recorder.arm(7);
        assert_eq!(recorder.tap(0.25, None, true, 48_000), None);
        for frame in 12_000..120_000 {
            let beat = f64::from(frame) / 24_000.0;
            if frame % 257 == 0 {
                recorder.flush();
            }
            if recorder.tap(beat, None, true, 48_000).is_some() {
                recorder.push([0.25, -0.5]);
            }
        }
        assert_eq!(recorder.tap(5.0, None, true, 48_000), None);
        let result = port.poll().unwrap();
        assert_eq!(result.project, 7);
        assert!(matches!(result.outcome, Outcome::Complete(48_000)));
        assert_eq!(result.frames.len(), 96_000);
        assert!(result.frames.iter().all(|&frame| frame == [8192, -16384]));
    }

    #[test]
    fn an_arm_on_the_first_sample_after_a_fractional_crossing_can_capture_that_beat() {
        let (mut port, mut recorder) = Recorder::channel();
        assert!(port.request(1, CaptureTap::Lane(0), 1));
        recorder.arm(1);
        assert_eq!(
            recorder.tap(1.00002, Some(0.99998), true, 48_000),
            Some(CaptureTap::Lane(0))
        );
        recorder.push([0.2, 0.0]);
        assert_eq!(recorder.tap(2.00001, None, true, 48_000), None);
        assert_eq!(port.poll().unwrap().frames.len(), 1);
    }

    #[test]
    fn cancellation_overflow_and_stale_project_return_buffers_and_allow_retry() {
        let (mut port, mut recorder) = Recorder::channel();
        assert!(port.request(1, CaptureTap::Lane(0), 1));
        recorder.arm(2);
        assert!(matches!(port.poll().unwrap().outcome, Outcome::Cancelled));
        assert!(port.request(2, CaptureTap::Bend, MAX_CAPTURE_BEATS));
        recorder.arm(2);
        for _ in 0..MAX_CAPTURE_FRAMES {
            assert_eq!(
                recorder.tap(0.0, None, true, 48_000),
                Some(CaptureTap::Bend)
            );
            recorder.push([0.0; 2]);
        }
        assert_eq!(recorder.tap(0.0, None, true, 48_000), None);
        assert!(matches!(port.poll().unwrap().outcome, Outcome::Full));
        assert!(port.request(2, CaptureTap::Bend, 1));
        recorder.arm(2);
        recorder.cancel();
        assert!(matches!(port.poll().unwrap().outcome, Outcome::Cancelled));
    }
}
