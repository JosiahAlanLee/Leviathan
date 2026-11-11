use crate::{CvFrame, EngineConfig, Pattern, PatternTiming, TrackerEvaluator};

pub const MAX_CHAIN_ENTRIES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternChain {
    entries: [u8; MAX_CHAIN_ENTRIES],
    len: usize,
}

impl Default for PatternChain {
    fn default() -> Self {
        Self {
            entries: [0; MAX_CHAIN_ENTRIES],
            len: 0,
        }
    }
}

impl PatternChain {
    pub fn as_slice(&self) -> &[u8] {
        &self.entries[..self.len]
    }

    pub fn push(&mut self, pattern: u8) -> bool {
        if self.len == MAX_CHAIN_ENTRIES {
            return false;
        }
        self.entries[self.len] = pattern;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<u8> {
        self.len = self.len.checked_sub(1)?;
        Some(core::mem::take(&mut self.entries[self.len]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    Pattern(usize),
    Chain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SongStatus {
    pub pattern: usize,
    pub chain_entry: Option<usize>,
    pub queued: Option<LaunchTarget>,
}

/// Adds pattern-local timing and launches to a single continuous sample clock.
pub struct SongPlayer {
    pub evaluator: TrackerEvaluator,
    chain: PatternChain,
    chain_entry: Option<usize>,
    queued: Option<LaunchTarget>,
    pattern_start_beat: f64,
    timing: Option<PatternTiming>,
    previous_plays: u32,
    needs_retrigger: bool,
}

impl SongPlayer {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            evaluator: TrackerEvaluator::new(config),
            chain: PatternChain::default(),
            chain_entry: None,
            queued: None,
            pattern_start_beat: 0.0,
            timing: None,
            previous_plays: 0,
            needs_retrigger: true,
        }
    }

    pub fn status(&self) -> SongStatus {
        SongStatus {
            pattern: self.evaluator.active_pattern,
            chain_entry: self.chain_entry,
            queued: self.queued,
        }
    }

    pub fn set_chain(&mut self, chain: PatternChain) {
        self.chain = chain;
        if chain.as_slice().is_empty() {
            self.chain_entry = None;
            if self.queued == Some(LaunchTarget::Chain) {
                self.queued = None;
            }
        }
    }

    pub fn invalidate_pattern_timing(&mut self, pattern_index: usize) {
        if pattern_index == self.evaluator.active_pattern {
            self.timing = None;
        }
    }

    fn resolve(
        &self,
        target: LaunchTarget,
        pattern_count: usize,
    ) -> Option<(usize, Option<usize>)> {
        let (pattern, entry) = match target {
            LaunchTarget::Pattern(pattern) => (pattern, None),
            LaunchTarget::Chain => (usize::from(*self.chain.as_slice().first()?), Some(0)),
        };
        (pattern < pattern_count).then_some((pattern, entry))
    }

    pub fn launch(&mut self, target: LaunchTarget, pattern_count: usize) {
        let Some((pattern, entry)) = self.resolve(target, pattern_count) else {
            return;
        };
        if self.evaluator.playing {
            self.queued = Some(target);
        } else {
            self.queued = None;
            self.select(pattern, entry, self.evaluator.clock.beat());
        }
    }

    fn select(&mut self, pattern: usize, entry: Option<usize>, beat: f64) {
        self.evaluator.active_pattern = pattern;
        self.chain_entry = entry;
        self.pattern_start_beat = beat;
        self.timing = None;
        self.previous_plays = 0;
        self.needs_retrigger = true;
    }

    pub fn reset_transport(&mut self, pattern_count: usize) {
        let (pattern, entry) = self
            .chain_entry
            .and_then(|_| self.resolve(LaunchTarget::Chain, pattern_count))
            .unwrap_or((self.evaluator.active_pattern, None));
        self.evaluator.reset_transport();
        self.queued = None;
        self.select(pattern, entry, 0.0);
    }

    fn next_pattern(&mut self, pattern_count: usize) -> (usize, Option<usize>) {
        if let Some(target) = self.queued.take() {
            if let Some(resolved) = self.resolve(target, pattern_count) {
                return resolved;
            }
        }
        let entries = self.chain.as_slice();
        if let Some(entry) = self.chain_entry.filter(|_| !entries.is_empty()) {
            let next = if entry < entries.len() {
                (entry + 1) % entries.len()
            } else {
                0
            };
            let pattern = usize::from(entries[next]);
            if pattern < pattern_count {
                return (pattern, Some(next));
            }
        }
        (self.evaluator.active_pattern, None)
    }

    /// Returns the CV frame and whether the instrument rack should retrigger held gates.
    pub fn step_sample(&mut self, patterns: &[Pattern]) -> (CvFrame, bool) {
        let Some(mut pattern) = patterns.get(self.evaluator.active_pattern) else {
            return (CvFrame::default(), false);
        };
        let beat = self.evaluator.take_sample();
        let mut timing = self.timing.unwrap_or_else(|| pattern.timing());
        if self.evaluator.playing && beat.is_finite() {
            loop {
                let boundary = self.pattern_start_beat + timing.duration_beats();
                if beat < boundary || boundary <= self.pattern_start_beat {
                    break;
                }
                let (next, entry) = self.next_pattern(patterns.len());
                self.previous_plays = if next == self.evaluator.active_pattern {
                    self.previous_plays.wrapping_add(1)
                } else {
                    0
                }
                ;
                self.evaluator.active_pattern = next;
                self.chain_entry = entry;
                // Advance by the musical duration, keeping sub-sample overshoot out of the anchor.
                self.pattern_start_beat = boundary;
                self.needs_retrigger = true;
                pattern = &patterns[next];
                timing = pattern.timing();
            }
            self.timing = Some(timing);
        }
        let cv = self.evaluator.evaluate_sample(
            pattern,
            timing,
            beat,
            self.pattern_start_beat,
            self.previous_plays,
        );
        let retrigger = self.evaluator.playing && core::mem::take(&mut self.needs_retrigger);
        (cv, retrigger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AutomationRow, NoteCell, PlaybackRate};

    #[test]
    fn queued_launch_changes_audio_and_automation_on_first_sample_of_incoming_row_zero() {
        let first = Pattern::default();
        let mut second = Pattern {
            rate: PlaybackRate::Half,
            ..Pattern::default()
        };
        second.lanes[0][0] = NoteCell {
            active: true,
            midi: 72,
            ..NoteCell::default()
        };
        second.automation[0] = AutomationRow {
            z: 0.75,
            ..AutomationRow::default()
        };
        let patterns = [first, second];
        let mut song = SongPlayer::new(EngineConfig {
            bpm: 120.0,
            sample_rate: 100.0,
            ..EngineConfig::default()
        });
        song.evaluator.playing = true;
        song.step_sample(&patterns);
        song.launch(LaunchTarget::Pattern(1), patterns.len());
        for _ in 1..800 {
            song.step_sample(&patterns);
        }
        assert_eq!(song.status().pattern, 0);
        let (cv, retrigger) = song.step_sample(&patterns);
        assert!(retrigger);
        assert_eq!(song.status().pattern, 1);
        assert_eq!(song.status().queued, None);
        assert_eq!(
            (cv.beat, cv.row, cv.pitch_v[0], cv.gate_v[0], cv.z_v),
            (16.0, 0, 1.0, 10.0, 7.5)
        );
        assert_eq!((cv.clock_v, cv.reset_v), (10.0, 0.0));
    }

    #[test]
    fn active_pattern_rate_edit_updates_cached_timing_without_resetting_clock() {
        let mut patterns = [Pattern::default()];
        let mut song = SongPlayer::new(EngineConfig {
            bpm: 120.0,
            sample_rate: 100.0,
            ..EngineConfig::default()
        });
        song.evaluator.playing = true;
        for _ in 0..100 {
            song.step_sample(&patterns);
        }
        song.evaluator.playing = false;
        patterns[0].rate = PlaybackRate::TwoX;
        song.invalidate_pattern_timing(0);
        song.evaluator.playing = true;

        for _ in 100..400 {
            let (_, retrigger) = song.step_sample(&patterns);
            assert!(!retrigger);
        }
        let (cv, retrigger) = song.step_sample(&patterns);
        assert!(retrigger);
        assert_eq!((cv.beat, cv.row), (8.0, 0));
    }

    #[test]
    fn mixed_rate_chain_uses_ideal_boundaries_through_a_tempo_change() {
        let patterns = [
            Pattern {
                length: 3,
                rate: PlaybackRate::TwoX,
                ..Pattern::default()
            },
            Pattern {
                length: 5,
                rate: PlaybackRate::FourX,
                ..Pattern::default()
            },
        ];
        let mut chain = PatternChain::default();
        for pattern in [0, 1, 1] {
            assert!(chain.push(pattern));
        }
        let mut song = SongPlayer::new(EngineConfig {
            bpm: 120.0,
            sample_rate: 997.0,
            ..EngineConfig::default()
        });
        song.set_chain(chain);
        song.launch(LaunchTarget::Chain, patterns.len());
        song.evaluator.playing = true;
        let mut boundary = 0.0;
        let mut entry = 0;
        let mut boundaries_seen = 0;
        let mut previous_beat = -1.0;
        for sample in 0..15_000 {
            let beat = if sample < 1000 {
                sample as f64 * 2.0 / 997.0
            } else {
                1000.0 * 2.0 / 997.0 + (sample - 1000) as f64 * 1.5 / 997.0
            };
            if sample == 1000 {
                song.evaluator.config.bpm = 90.0;
            }
            let (cv, started) = song.step_sample(&patterns);
            assert!((cv.beat - beat).abs() < 1e-11);
            if beat >= boundary && previous_beat < boundary {
                assert!(started, "missed boundary {boundary} at sample {sample}");
                assert_eq!(song.status().chain_entry, Some(entry));
                assert_eq!(cv.row, 0);
                boundary += patterns[usize::from(chain.as_slice()[entry])]
                    .timing()
                    .duration_beats();
                entry = (entry + 1) % chain.as_slice().len();
                boundaries_seen += 1;
            } else {
                assert!(!started);
            }
            previous_beat = beat;
        }
        assert!(boundaries_seen > 60);
    }
}
