use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;

use leviathan_core::{encode_bytebus_frame, rack_routing::ByteRoute, CvFrame, LaunchTarget, NUM_LANES};
use leviathan_dsp::{ByteBusReceiver, ByteBusSource};
use serde::{Deserialize, Serialize};

// Over half an hour at 240 BPM and four pulses per beat. Only a recording
// allocates this buffer, on the host.
const MAX_PULSES: usize = 32_768;
const UNSET: u16 = 256;
pub const CV_CHANNELS: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Route {
    pub lane: Option<u8>,
    pub feedback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Patch {
    pub route: Route,
    pub rotation: u16,
    pub threshold: u8,
    pub take: Option<Take>,
}

impl Default for Patch {
    fn default() -> Self {
        Self {
            route: Route::default(),
            rotation: 0,
            threshold: 112,
            take: None,
        }
    }
}

impl Patch {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn is_valid(&self, pattern_count: usize) -> bool {
        self.rotation < 360
            && self
                .route
                .lane
                .is_none_or(|lane| usize::from(lane) < NUM_LANES)
            && self
                .take
                .as_ref()
                .is_none_or(|take| take.is_valid(pattern_count))
    }

    pub fn take_for(&self, target: LaunchTarget) -> Option<&Take> {
        self.take.as_ref().filter(|take| {
            self.route.lane == Some(take.lane) && take.target == Target::from(target)
        })
    }

    pub fn replay(&self, target: LaunchTarget) -> Replay<'_> {
        Replay {
            take: self.take_for(target),
            pulse: None,
            held: None,
            started: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Pattern(usize),
    Chain,
}

impl From<LaunchTarget> for Target {
    fn from(target: LaunchTarget) -> Self {
        match target {
            LaunchTarget::Pattern(index) => Self::Pattern(index),
            LaunchTarget::Chain => Self::Chain,
        }
    }
}

impl Target {
    pub fn label(self) -> String {
        match self {
            Self::Pattern(index) => format!("Pattern {index:02X}"),
            Self::Chain => "Chain".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Take {
    pub target: Target,
    pub lane: u8,
    pub words: Vec<u16>,
}

impl Take {
    fn is_valid(&self, pattern_count: usize) -> bool {
        usize::from(self.lane) < NUM_LANES
            && match self.target {
                Target::Pattern(index) => index < pattern_count,
                Target::Chain => true,
            }
            && (1..=MAX_PULSES).contains(&self.words.len())
            && self.words.iter().all(|&word| word <= UNSET)
    }
}

// Single writer (audio), single reader (host). A published prefix is immutable
// and can be copied even while the next pulse is being recorded.
pub struct Recorder {
    target: Target,
    pub lane: u8,
    words: Box<[AtomicU16]>,
    len: AtomicUsize,
    done: AtomicBool,
}

impl Recorder {
    pub fn new(target: LaunchTarget, lane: u8) -> Self {
        Self {
            target: target.into(),
            lane,
            words: std::iter::repeat_with(|| AtomicU16::new(UNSET))
                .take(MAX_PULSES)
                .collect(),
            len: AtomicUsize::new(0),
            done: AtomicBool::new(false),
        }
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn push(&self, word: Option<u8>) -> bool {
        let len = self.len.load(Ordering::Relaxed);
        let Some(slot) = self.words.get(len) else {
            return false;
        };
        slot.store(word.map_or(UNSET, u16::from), Ordering::Relaxed);
        self.len.store(len + 1, Ordering::Release);
        len + 1 < MAX_PULSES
    }

    fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }

    pub fn snapshot(&self) -> Option<Take> {
        let len = self.len();
        (len > 0).then(|| Take {
            target: self.target,
            lane: self.lane,
            words: self.words[..len]
                .iter()
                .map(|word| word.load(Ordering::Relaxed))
                .collect(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub epoch: u64,
    pub pulse: u64,
    pub word: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub request: Request,
    pub word: u8,
    pub rotation: u16,
}

#[derive(Clone, Copy, Default)]
pub struct Status {
    pub request: Option<Request>,
    pub latched: Option<Reply>,
    pub missed: u32,
}

// The evaluator derives ByteBus high from this same pulse phase. Keep the
// ordinal across pause/continue so resuming halfway through high is no edge.
fn new_pulse(last: &mut Option<u64>, cv: &CvFrame, ppqn: u32) -> Option<u64> {
    if cv.run_v <= 2.5 || cv.bytebus_v[8] <= 5.0 {
        return None;
    }
    let pulse = (cv.beat * f64::from(ppqn)) as u64;
    if last.is_some_and(|previous| previous >= pulse) {
        return None;
    }
    *last = Some(pulse);
    Some(pulse)
}

#[derive(Default)]
pub struct Clock {
    route: Route,
    epoch: u64,
    pulse: Option<u64>,
    status: Status,
    pending: Option<Reply>,
    clear_at_edge: bool,
    reset_pending: bool,
    recorder: Option<Arc<Recorder>>,
}

impl Clock {
    pub fn set_route(&mut self, route: Route) {
        if route.lane != self.route.lane {
            self.finish_recording();
            self.invalidate();
            self.status.latched = None;
            self.status.missed = 0;
            self.reset_pending = true;
        }
        self.route = route;
    }

    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.pending = None;
        self.status.request = None;
    }

    pub fn clear_field(&mut self) {
        self.invalidate();
        self.clear_at_edge = true;
    }

    pub fn reset_transport(&mut self) {
        self.finish_recording();
        self.invalidate();
        self.pulse = None;
        self.status.latched = None;
        self.status.missed = 0;
        self.clear_at_edge = false;
        self.reset_pending |= self.route.lane.is_some();
    }

    pub fn record(&mut self, recorder: Arc<Recorder>) {
        self.finish_recording();
        if self.route.lane == Some(recorder.lane) {
            self.recorder = Some(recorder);
        } else {
            recorder.finish();
        }
    }

    pub fn finish_recording(&mut self) {
        if let Some(recorder) = self.recorder.take() {
            recorder.finish();
        }
    }

    pub fn reply(&mut self, reply: Reply) {
        if self.status.request == Some(reply.request) && self.pending.is_none() {
            self.pending = Some(reply);
        }
    }

    /// Returns true on a published request, to send telemetry at the edge.
    pub fn step(&mut self, cv: &CvFrame, ppqn: u32, lookup: ByteRoute) -> bool {
        if self.route.lane.is_none() {
            return false;
        }
        let Some(pulse) = new_pulse(&mut self.pulse, cv, ppqn) else {
            return false;
        };
        if std::mem::take(&mut self.clear_at_edge) {
            self.reset_pending |= self.status.latched.take().is_some();
        }
        let due = pulse.checked_sub(1);
        if let Some(reply) = self.pending.take().filter(|r| Some(r.request.pulse) == due) {
            self.status.latched = Some(reply);
        } else if self.status.request.is_some_and(|r| Some(r.pulse) == due) {
            self.status.missed = self.status.missed.saturating_add(1);
        }
        let held = self.status.latched.map(|reply| reply.word);
        let input = if self.route.feedback {
            held.unwrap_or(lookup.reset_word)
        } else {
            // Decode the exact eight rails on this edge, including 00.
            ByteBusReceiver::default()
                .step(ByteBusSource::Current, &cv.bytebus_v)
                .unwrap_or_default()
        };
        self.status.request = Some(Request {
            epoch: self.epoch,
            pulse,
            word: lookup.lookup.map(input),
        });
        if self
            .recorder
            .as_ref()
            .is_some_and(|recorder| !recorder.push(held))
        {
            self.finish_recording();
        }
        true
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn frame(&self) -> Option<(usize, u8)> {
        Some((usize::from(self.route.lane?), self.status.latched?.word))
    }

    pub fn output_frame(&mut self, edge: bool) -> ReturnFrame {
        ReturnFrame::new(self.frame(), edge, std::mem::take(&mut self.reset_pending))
    }
}

pub struct Replay<'a> {
    take: Option<&'a Take>,
    pulse: Option<u64>,
    held: Option<u8>,
    started: bool,
}

#[derive(Clone, Copy, Default)]
pub struct ReturnFrame {
    pub address: Option<(usize, u8)>,
    clock: bool,
    reset: bool,
}

impl ReturnFrame {
    fn new(address: Option<(usize, u8)>, edge: bool, reset: bool) -> Self {
        Self {
            address,
            clock: edge && address.is_some(),
            reset,
        }
    }

    /// Channels 1–9 carry the Rack cable at normalized levels and channel
    /// 10 carries the separate reset strobe. Both rails use 0.0 or 1.0.
    pub fn cv_samples(self) -> [f32; CV_CHANNELS] {
        let word = self.address.map_or(0, |(_, word)| word);
        let cable = encode_bytebus_frame(word, self.clock);
        let mut samples = [0.0; CV_CHANNELS];
        for (sample, voltage) in samples.iter_mut().zip(cable) {
            *sample = if voltage > 0.0 { 1.0 } else { 0.0 };
        }
        samples[9] = if self.reset { 1.0 } else { 0.0 };
        samples
    }
}

impl Replay<'_> {
    pub fn step(&mut self, cv: &CvFrame, ppqn: u32) -> ReturnFrame {
        let Some(take) = self.take else {
            return ReturnFrame::default();
        };
        let first = !std::mem::replace(&mut self.started, true);
        let previous = self.held;
        let edge = new_pulse(&mut self.pulse, cv, ppqn);
        if let Some(pulse) = edge {
            self.held = take
                .words
                .get(pulse as usize)
                .or_else(|| take.words.last())
                .and_then(|&word| u8::try_from(word).ok());
        }
        ReturnFrame::new(
            self.held.map(|word| (usize::from(take.lane), word)),
            edge.is_some(),
            first || (previous.is_some() && self.held.is_none()),
        )
    }
}

#[derive(Clone, Copy)]
pub struct Probe {
    pub request: Request,
    pub rotation: u16,
    pub threshold: u8,
}

impl Probe {
    pub fn points(rotation: u16) -> [[f32; 2]; 8] {
        std::array::from_fn(|bit| {
            let turns = f32::from(rotation) / 360.0 + bit as f32 / 8.0;
            let (y, x) = (turns * std::f32::consts::TAU).sin_cos();
            [0.5 + 0.18 * x, 0.5 + 0.32 * y]
        })
    }

    pub fn positions(self, width: u32, height: u32) -> [(u32, u32); 8] {
        Self::points(self.rotation)
            .map(|[x, y]| ((x * width as f32) as u32, (y * height as f32) as u32))
    }

    pub fn reply(self, pixels: &[[u8; 4]]) -> Reply {
        debug_assert_eq!(pixels.len(), 8);
        let threshold = u32::from(self.threshold) * 256;
        let word = pixels
            .iter()
            .enumerate()
            .fold(0, |word, (bit, &[r, g, b, _])| {
                let luma = 54 * u32::from(r) + 183 * u32::from(g) + 19 * u32::from(b);
                word | (u8::from(luma >= threshold) << bit)
            });
        Reply {
            request: self.request,
            word,
            rotation: self.rotation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leviathan_core::{encode_bytebus_frame, rack_routing::ByteLookup};

    fn edge(pulse: u32, word: u8) -> CvFrame {
        CvFrame {
            beat: f64::from(pulse) / 4.0,
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(word, true),
            ..CvFrame::default()
        }
    }

    #[test]
    fn latch_has_a_single_deadline_and_replay_keeps_zero_and_holds() {
        let route = Route {
            lane: Some(2),
            feedback: false,
        };
        let mut clock = Clock::default();
        clock.set_route(route);
        let recorder = Arc::new(Recorder::new(LaunchTarget::Chain, 2));
        clock.record(recorder.clone());
        let lookup = ByteRoute {
            lookup: ByteLookup::Invert,
            ..Default::default()
        };
        assert!(clock.step(&edge(0, 0xff), 4, lookup));
        let first = clock.status().request.unwrap();
        assert_eq!(first.word, 0);
        let zero = Reply {
            request: first,
            word: 0,
            rotation: 34,
        };
        clock.reply(zero);
        assert_eq!(clock.frame(), None);
        assert!(!clock.step(&CvFrame::default(), 4, lookup));
        assert!(!clock.step(&edge(0, 0), 4, lookup));
        assert!(clock.step(&edge(1, 19), 4, lookup));
        assert_eq!(clock.status().latched, Some(zero));
        assert_eq!(clock.frame(), Some((2, 0)));

        let missed = clock.status().request.unwrap();
        assert!(clock.step(&edge(2, 19), 4, lookup));
        assert_eq!(clock.status().missed, 1);
        clock.reply(Reply {
            request: missed,
            word: 127,
            rotation: 0,
        });
        assert!(clock.step(&edge(3, 19), 4, lookup));
        assert_eq!(clock.frame(), Some((2, 0)));

        let before_clear = clock.status().request.unwrap();
        clock.clear_field();
        clock.reply(Reply {
            request: before_clear,
            word: 255,
            rotation: 0,
        });
        assert_eq!(clock.frame(), Some((2, 0)));
        assert!(clock.step(&edge(4, 19), 4, lookup));
        assert_eq!(clock.frame(), None);
        assert_ne!(clock.status().request.unwrap().epoch, first.epoch);
        clock.finish_recording();
        assert!(recorder.is_done());

        let take = recorder.snapshot().unwrap();
        assert_eq!(take.words, [UNSET, 0, 0, 0, UNSET]);
        let patch = Patch {
            route,
            take: Some(take),
            ..Default::default()
        };
        let mut replay = patch.replay(LaunchTarget::Chain);
        for (pulse, expected) in [None, Some(0), Some(0), Some(0), None, None]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                replay.step(&edge(pulse as u32, 3), 4).address,
                expected.map(|word| (2, word))
            );
        }
        assert_eq!(
            patch.replay(LaunchTarget::Pattern(0)).step(&edge(1, 3), 4).address,
            None
        );
    }

    #[test]
    fn feedback_reuses_the_latched_return_through_lookup_and_resets_epoch() {
        let mut clock = Clock::default();
        clock.set_route(Route {
            lane: Some(0),
            feedback: true,
        });
        let lookup = ByteRoute {
            reset_word: 0x44,
            lookup: ByteLookup::Invert,
            ..Default::default()
        };
        assert!(clock.step(&edge(0, 0), 4, lookup));
        let startup = clock.output_frame(true).cv_samples();
        assert_eq!(startup[..9], [0.0; 9]);
        assert_eq!(startup[9], 1.0);
        let sent = clock.status().request.unwrap();
        assert_eq!(sent.word, 0xbb);
        let reply = Reply {
            request: sent,
            word: 0x55,
            rotation: 0,
        };
        clock.reply(reply);
        assert!(clock.step(&edge(1, 0), 4, lookup));
        let published = clock.output_frame(true).cv_samples();
        assert_eq!(published, [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        assert_eq!(clock.status().request.unwrap().word, 0xaa);

        assert!(!clock.step(&CvFrame::default(), 4, lookup));
        let paused = clock.output_frame(false).cv_samples();
        assert_eq!(paused[..8], published[..8]);
        assert_eq!(paused[8..], [0.0; 2]);
        assert!(!clock.step(&edge(1, 0), 4, lookup));
        assert_eq!(clock.output_frame(false).cv_samples(), paused);

        clock.reset_transport();
        assert!(clock.step(&edge(0, 0), 4, lookup));
        assert_eq!(clock.output_frame(true).cv_samples(), startup);
        assert_eq!(clock.output_frame(false).cv_samples(), [0.0; CV_CHANNELS]);
        clock.reply(reply);
        assert!(clock.step(&edge(1, 0), 4, lookup));
        assert_eq!(clock.frame(), None);
        assert_eq!(clock.status().request.unwrap().word, 0xbb);

        clock.set_route(Route::default());
        assert!(!clock.step(&edge(2, 0), 4, lookup));
        assert_eq!(clock.output_frame(false).cv_samples(), startup);
        assert_eq!(clock.output_frame(false).cv_samples(), [0.0; CV_CHANNELS]);
    }
}
