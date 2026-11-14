//! Clocked stereo echo. The caller supplies its delay storage.

use crate::bender::FrequencyShifter;
use crate::voice::{cosine, sine, StateVariable};
use leviathan_core::CvFrame;

pub const RATIOS: [(u8, u8); 10] = [
    (1, 4),
    (1, 3),
    (1, 2),
    (2, 3),
    (3, 4),
    (1, 1),
    (3, 2),
    (2, 1),
    (3, 1),
    (4, 1),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sync {
    #[default]
    Free,
    Transport,
    ByteBus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub echo_mix: f32,
    pub space_mix: f32,
    pub time: f32,
    pub sync: Sync,
    pub ratio: u8,
    pub feedback: f32,
    pub tone: f32,
    pub spread: f32,
    pub shift_hz: f32,
    pub shift_in_loop: bool,
    pub esp: crate::esp::Mode,
    pub limit_64k: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            echo_mix: 0.0,
            space_mix: 1.0,
            time: 0.76,
            sync: Sync::Free,
            ratio: 5,
            feedback: 0.38,
            tone: 0.5,
            spread: 0.0,
            shift_hz: 0.0,
            shift_in_loop: true,
            esp: crate::esp::Mode::Off,
            limit_64k: true,
        }
    }
}

impl Params {
    pub fn is_valid(self) -> bool {
        [
            self.echo_mix,
            self.space_mix,
            self.time,
            self.tone,
            self.spread,
        ]
        .iter()
        .all(|v| (0.0..=1.0).contains(v))
            && (0.0..=1.10).contains(&self.feedback)
            && (-200.0..=200.0).contains(&self.shift_hz)
            && usize::from(self.ratio) < RATIOS.len()
    }

    pub fn free_seconds(self) -> f32 {
        // Split the 2000:1 ratio into three bounded tuning conversions.
        if self.time >= 1.0 {
            return 2.0;
        }
        let octave = self.time * 3.655_261_5;
        let ratio = crate::v_oct_to_hz(octave) / 261.625_58;
        (0.001 * ratio * ratio * ratio).clamp(0.001, 2.0)
    }

    pub fn max_seconds(self, host_rate: f32) -> f32 {
        let rate = self.esp.rate().unwrap_or(host_rate);
        let configured = (capacity(host_rate) - 2) as f32;
        let memory = if self.esp != crate::esp::Mode::Off && self.limit_64k {
            configured.min(32_768.0)
        } else {
            configured
        };
        (memory / rate).min(2.0)
    }
}

pub fn capacity(host_rate: f32) -> usize {
    (2.0 * host_rate.clamp(78_125.0, 96_000.0)) as usize + 2
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    pub target_seconds: f32,
    pub effective_seconds: f32,
    pub awaiting_clock: bool,
}

#[derive(Default)]
pub(super) struct Clock {
    sample: u64,
    last_edge: Option<u64>,
    period: Option<f32>,
    high: bool,
    reset_high: bool,
}

impl Clock {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn step(&mut self, cv: &CvFrame, host_rate: f32) {
        let reset = cv.reset_v > 2.5;
        let running = cv.run_v > 2.5;
        let high = running && cv.bytebus_v[8] > 5.0;
        if reset && !self.reset_high {
            self.last_edge = None;
            self.period = None;
            self.high = false;
        }
        if running {
            if high && !self.high {
                if let Some(last) = self.last_edge {
                    let interval = self.sample.wrapping_sub(last);
                    if interval > 0 {
                        self.period = Some(interval as f32 / host_rate);
                    }
                }
                self.last_edge = Some(self.sample);
            }
            self.high = high;
        } else {
            self.last_edge = None;
        }
        self.reset_high = reset;
        self.sample = self.sample.wrapping_add(1);
    }

    pub(super) fn timing(&self, params: Params, free_seconds: f32, bpm: f32, host: f32) -> Timing {
        let (n, d) = RATIOS[usize::from(params.ratio)];
        let ratio = f32::from(n) / f32::from(d);
        let target_seconds = match params.sync {
            Sync::Free => free_seconds,
            Sync::Transport => ratio * (60.0 / bpm),
            Sync::ByteBus => self.period.map_or(free_seconds, |period| ratio * period),
        };
        Timing {
            target_seconds,
            effective_seconds: target_seconds.clamp(0.001, params.max_seconds(host)),
            awaiting_clock: params.sync == Sync::ByteBus && self.period.is_none(),
        }
    }
}

pub(super) struct Echo {
    pos: usize,
    filled: usize,
    time_frames: f32,
    target_frames: f32,
    glide_step: f32,
    glide_left: u32,
    lows: [[f32; 2]; 2],
    guards: [StateVariable; 2],
    shifter: FrequencyShifter,
    rate: f32,
    glide_length: u32,
    highpass_g: f32,
    tone_g: f32,
    guard_g: f32,
}

impl Echo {
    pub(super) fn new(rate: f32) -> Self {
        let pole = |hz| {
            let alpha = core::f32::consts::TAU * hz / rate;
            alpha / (1.0 + alpha)
        };
        let cycles = 0.5 * (18_000.0 / rate).min(0.4);
        Self {
            pos: 0,
            filled: 0,
            time_frames: 0.0,
            target_frames: 0.0,
            glide_step: 0.0,
            glide_left: 0,
            lows: [[0.0; 2]; 2],
            guards: core::array::from_fn(|_| StateVariable::default()),
            shifter: FrequencyShifter::default(),
            rate,
            glide_length: (rate * 0.05 + 0.5) as u32,
            highpass_g: pole(40.0),
            tone_g: pole(1100.0),
            guard_g: sine(cycles) / cosine(cycles),
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.rate);
    }

    pub(super) fn reset_shift(&mut self) {
        self.shifter = FrequencyShifter::default();
    }

    pub(super) fn process(
        &mut self,
        input: [f32; 2],
        storage: &mut [[f32; 2]],
        params: Params,
        seconds: f32,
    ) -> [f32; 2] {
        if storage.len() < 3 {
            return [0.0; 2];
        }
        let fixed = params.esp != crate::esp::Mode::Off;
        let storage = if fixed && params.limit_64k {
            let frames = storage.len().min(32_768 + 2);
            &mut storage[..frames]
        } else {
            storage
        };
        let node = |sample| crate::esp::node(sample, fixed);
        let coef = |value| crate::esp::coefficient(value, fixed);
        let target = (seconds * self.rate).clamp(1.0, (storage.len() - 2) as f32);
        if self.time_frames == 0.0 {
            self.time_frames = target;
            self.target_frames = target;
        }
        if target != self.target_frames {
            self.target_frames = target;
            self.glide_left = self.glide_length;
            self.glide_step = (target - self.time_frames) / self.glide_length as f32;
        }
        if self.glide_left > 0 {
            self.glide_left -= 1;
            self.time_frames = if self.glide_left == 0 {
                self.target_frames
            } else {
                self.time_frames + self.glide_step
            };
        }
        let integer = self.time_frames as usize;
        let fraction = self.time_frames - integer as f32;
        let read = |back| {
            if back <= self.filled {
                storage[(self.pos + storage.len() - back) % storage.len()]
            } else {
                [0.0; 2]
            }
        };
        let near = read(integer);
        let far = read(integer + 1);
        let delayed = core::array::from_fn(|ch| node(near[ch] + fraction * (far[ch] - near[ch])));
        let filtered = core::array::from_fn(|ch| {
            let [dc, tone] = &mut self.lows[ch];
            *dc = node(*dc + coef(self.highpass_g) * (delayed[ch] - *dc));
            let high = node(delayed[ch] - *dc);
            *tone = node(*tone + coef(self.tone_g) * (high - *tone));
            let color = (2.0 * params.tone - 1.0).abs();
            let shaped = if params.tone < 0.5 {
                high + coef(color) * (*tone - high)
            } else {
                high - coef(color) * *tone
            };
            // TPT keeps the tangent coefficient above unity at high cutoff.
            let g = self.guard_g;
            node(
                self.guards[ch]
                    .step(node(shaped), g, core::f32::consts::SQRT_2, 0.0)
                    .low,
            )
        });
        let shift = |shifter: &mut FrequencyShifter, frame| {
            if params.shift_hz == 0.0 {
                frame
            } else {
                shifter.step_mode(frame, params.shift_hz, self.rate, fixed)
            }
        };
        let feedback = if params.shift_in_loop {
            shift(&mut self.shifter, filtered)
        } else {
            filtered
        };
        for ch in 0..2 {
            let feedback = feedback[ch] + coef(params.spread) * (feedback[ch ^ 1] - feedback[ch]);
            let gain = if fixed {
                // 110% uses a representable half-gain with an exact binary scale.
                2.0 * coef(params.feedback * 0.5)
            } else {
                params.feedback
            };
            let drive = node(feedback * gain);
            let excess = (drive.abs() - 0.45).max(0.0);
            let limited = drive.signum() * (drive.abs().min(0.45) + excess / (1.0 + 5.0 * excess));
            storage[self.pos][ch] = crate::esp::memory(node(input[ch] + limited), fixed);
        }
        self.pos = (self.pos + 1) % storage.len();
        self.filled = (self.filled + 1).min(storage.len() - 1);
        if params.shift_in_loop {
            delayed
        } else {
            shift(&mut self.shifter, delayed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ratios_hold_and_reclock_from_the_selected_source() {
        let mut clock = Clock::default();
        let mut p = Params {
            sync: Sync::ByteBus,
            ..Params::default()
        };
        for (i, (num, den)) in RATIOS.iter().enumerate() {
            p.ratio = i as u8;
            p.sync = Sync::Transport;
            for bpm in [80.0, 120.0, 180.0] {
                let t = clock.timing(p, p.free_seconds(), bpm, 48_000.0);
                assert!(
                    (t.target_seconds - 60.0 / bpm * f32::from(*num) / f32::from(*den)).abs()
                        < 1e-6
                );
                assert_eq!(t.effective_seconds, t.target_seconds.min(2.0));
            }
        }
        p.sync = Sync::ByteBus;
        p.ratio = 5;
        let mut cv = CvFrame {
            run_v: 10.0,
            ..CvFrame::default()
        };
        for i in 0..=6000 {
            cv.bytebus_v[8] = if i % 6000 < 100 { 10.0 } else { 0.0 };
            clock.step(&cv, 48_000.0);
            assert_eq!(
                clock
                    .timing(p, p.free_seconds(), 120.0, 48_000.0)
                    .awaiting_clock,
                i < 6000
            );
        }
        for (i, &(n, d)) in RATIOS.iter().enumerate() {
            p.ratio = i as u8;
            assert!(
                (clock.timing(p, 0.01, 120.0, 48_000.0).effective_seconds
                    - 0.125 * f32::from(n) / f32::from(d))
                .abs()
                    < 1e-6
            );
        }
        p.ratio = 5;
        cv.run_v = 0.0;
        for _ in 0..20_000 {
            clock.step(&cv, 48_000.0);
        }
        assert_eq!(
            clock
                .timing(p, p.free_seconds(), 120.0, 48_000.0)
                .effective_seconds,
            0.125
        );
        cv.run_v = 10.0;
        for i in 0..=9600 {
            cv.bytebus_v[8] = if i % 4800 < 100 { 10.0 } else { 0.0 };
            clock.step(&cv, 48_000.0);
            if i < 9600 {
                assert_eq!(
                    clock.timing(p, 0.01, 150.0, 48_000.0).effective_seconds,
                    0.125
                );
            }
        }
        for (i, &(n, d)) in RATIOS.iter().enumerate() {
            p.ratio = i as u8;
            assert!(
                (clock.timing(p, 0.01, 150.0, 48_000.0).effective_seconds
                    - 0.1 * f32::from(n) / f32::from(d))
                .abs()
                    < 1e-6
            );
        }
        cv.run_v = 0.0;
        cv.reset_v = 10.0;
        clock.step(&cv, 48_000.0);
        assert!(
            clock
                .timing(p, p.free_seconds(), 120.0, 48_000.0)
                .awaiting_clock
        );
    }

    #[test]
    fn loop_accumulates_frequency_shift_and_post_shifts_each_repeat_once() {
        let rate = 48_000.0;
        for in_loop in [false, true] {
            let mut memory = std::vec![[0.0; 2]; capacity(rate)];
            let mut echo = Echo::new(rate);
            let params = Params {
                feedback: 0.7,
                shift_hz: 80.0,
                shift_in_loop: in_loop,
                ..Params::default()
            };
            let mut bins = [[[0.0_f64; 2]; 3]; 3];
            const LENGTH: usize = 6000;
            for i in 0..(4 * LENGTH) {
                let x = if i < LENGTH / 2 {
                    0.2 * sine(i as f32 * 400.0 / rate % 1.0)
                } else {
                    0.0
                };
                let out = echo.process([x, 0.0], &mut memory, params, LENGTH as f32 / rate)[0];
                let repeat = i / LENGTH;
                if repeat > 0 && (400..2400).contains(&(i % LENGTH)) {
                    for (bin, hz) in bins[repeat - 1].iter_mut().zip([400.0, 480.0, 560.0]) {
                        let phase = i as f32 * hz / rate % 1.0;
                        bin[0] += f64::from(out * sine(phase));
                        bin[1] += f64::from(out * cosine(phase));
                    }
                }
            }
            for (repeat, bins) in bins.into_iter().enumerate() {
                let power = bins.map(|[x, y]| x * x + y * y);
                let wanted = if in_loop { repeat } else { 1 };
                assert!(power[wanted] > 1.0);
                for (i, other) in power.into_iter().enumerate() {
                    if i != wanted {
                        assert!(
                            power[wanted] > other * 25.0,
                            "loop={in_loop} repeat={repeat} {power:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn esp_memory_limits_apply_without_changing_the_ratio() {
        for mode in crate::esp::Mode::ALL.into_iter().skip(1) {
            let params = Params {
                sync: Sync::Transport,
                ratio: 9,
                esp: mode,
                ..Params::default()
            };
            let clock = Clock::default();
            let limited = clock.timing(params, params.free_seconds(), 120.0, 44_100.0);
            assert_eq!(limited.target_seconds, 2.0);
            assert_eq!(limited.effective_seconds, 32_768.0 / mode.rate().unwrap());
            let unlimited = Params {
                limit_64k: false,
                ..params
            };
            let unlimited = clock.timing(unlimited, params.free_seconds(), 120.0, 44_100.0);
            assert_eq!(unlimited.effective_seconds, 2.0);
        }
    }

    #[test]
    fn echo_spacing_and_max_feedback_are_bounded() {
        let rate = 48_000.0;
        let mut mem = std::vec![[0.0; 2]; capacity(rate)];
        let mut echo = Echo::new(rate);
        let mut p = Params {
            feedback: 1.1,
            spread: 1.0,
            ..Params::default()
        };
        let mut peaks = [0.0_f32; 2];
        for i in 0..130_000 {
            if i == 80_000 {
                p.feedback = 0.35;
            }
            let input = if i == 0 { [0.5, 0.0] } else { [0.0; 2] };
            let out = echo.process(input, &mut mem, p, 0.01);
            if i < 480 {
                assert_eq!(out, [0.0; 2]);
            }
            if i == 480 {
                assert_eq!(out, [0.5, 0.0]);
            }
            for ch in 0..2 {
                assert!(out[ch].is_finite() && out[ch].abs() <= 1.0);
                peaks[ch] = peaks[ch].max(out[ch].abs());
            }
            if i > 120_000 {
                assert!(out.iter().all(|s| s.abs() < 1e-4));
            }
        }
        assert!(peaks[1] > 0.1);
    }
}
