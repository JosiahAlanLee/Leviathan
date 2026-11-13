use crate::bender::{BenderParams, SpoonBenderBus};

const DELAY_WORDS: usize = 16_384;
const SLAPBACK_FRAMES: usize = DELAY_WORDS / 2;
const SLAPBACK_SECONDS: [f32; 2] = [0.017, 0.021];
const LONG_SECONDS: [f32; 2] = [0.125, 1.0 / 6.0];
const Q14_SCALE: f32 = 16_384.0;
pub(super) const MAX_SEND: f32 = 0.45;
const KNEE: f32 = 0.8;

/// Output sample peaks and pre-limiter full-scale overs for a telemetry interval.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MasterPeaks {
    pub output: [f32; 2],
    pub clipped: [bool; 2],
}

impl MasterPeaks {
    pub fn merge(&mut self, other: Self) {
        for channel in 0..2 {
            self.output[channel] = self.output[channel].max(other.output[channel]);
            self.clipped[channel] |= other.clipped[channel];
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DelayMode {
    #[default]
    Slapback,
    LongPingPong,
}

impl DelayMode {
    fn offsets(self, sample_rate: f32) -> [usize; 2] {
        let (seconds, rate, frames) = match self {
            Self::Slapback => (SLAPBACK_SECONDS, sample_rate, SLAPBACK_FRAMES),
            Self::LongPingPong => (
                LONG_SECONDS,
                sample_rate.min(DELAY_WORDS as f32 / LONG_SECONDS[1]),
                DELAY_WORDS,
            ),
        };
        seconds.map(|seconds| ((seconds * rate + 0.5) as usize).clamp(1, frames))
    }
}

/// The shared stereo delay used by the master and individual export stems.
pub struct StereoDelay {
    // Slapback stores two f32 bit patterns per frame. Long Ping-Pong
    // stores one packed pair of signed Q1.14 samples in the same pool.
    delay: [u32; DELAY_WORDS],
    offsets: [usize; 2],
    cursor: usize,
    sample_rate: f32,
    mode: DelayMode,
}

const _: () = assert!(core::mem::size_of::<StereoDelay>() <= 65_600);

impl StereoDelay {
    pub fn new(sample_rate: f32) -> Self {
        let mode = DelayMode::default();
        Self {
            delay: [0; DELAY_WORDS],
            offsets: mode.offsets(sample_rate),
            cursor: 0,
            sample_rate,
            mode,
        }
    }

    pub fn set_mode(&mut self, mode: DelayMode) {
        if self.mode != mode {
            self.mode = mode;
            self.offsets = mode.offsets(self.sample_rate);
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.delay.fill(0);
        self.cursor = 0;
    }

    fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
        let [send_l, send_r] = input.map(finite_sample);
        let [left_offset, right_offset] = self.offsets;
        // Each input and each pass of feedback crosses to the opposite side.
        let (echo, frames) = match self.mode {
            DelayMode::Slapback => {
                let l = (self.cursor + SLAPBACK_FRAMES - left_offset) % SLAPBACK_FRAMES;
                let r = (self.cursor + SLAPBACK_FRAMES - right_offset) % SLAPBACK_FRAMES;
                let left = f32::from_bits(self.delay[2 * l]);
                let right = f32::from_bits(self.delay[2 * r + 1]);
                self.delay[2 * self.cursor] = flush_tail(send_r + 0.28 * right).to_bits();
                self.delay[2 * self.cursor + 1] = flush_tail(send_l + 0.28 * left).to_bits();
                ([left, right], SLAPBACK_FRAMES)
            }
            DelayMode::LongPingPong => {
                let l = (self.cursor + DELAY_WORDS - left_offset) % DELAY_WORDS;
                let r = (self.cursor + DELAY_WORDS - right_offset) % DELAY_WORDS;
                let left = f32::from(self.delay[l] as i16) / Q14_SCALE;
                let right = f32::from((self.delay[r] >> 16) as i16) / Q14_SCALE;
                let next_l = ((send_r + 0.48 * right) * Q14_SCALE) as i16 as u16;
                let next_r = ((send_l + 0.48 * left) * Q14_SCALE) as i16 as u16;
                self.delay[self.cursor] = u32::from(next_l) | (u32::from(next_r) << 16);
                ([left, right], DELAY_WORDS)
            }
        };
        self.cursor = (self.cursor + 1) & (frames - 1);
        echo
    }

    pub(super) fn mix(&mut self, dry: [f32; 2], send: [f32; 2]) -> [f32; 2] {
        let [left, right] = self.process(send);
        [dry[0] + left, dry[1] + right]
    }
}

pub(super) struct MasterBus {
    delay: StereoDelay,
    bender: SpoonBenderBus,
    send: f32,
    send_target: f32,
    send_step: f32,
    dc_input: [f32; 2],
    dc_output: [f32; 2],
    dc_pole: f32,
    dc_gain: f32,
    pub(super) peaks: MasterPeaks,
}

impl MasterBus {
    pub(super) fn new(sample_rate: f32) -> Self {
        let dc_pole = 1.0 / (1.0 + core::f32::consts::TAU * 8.0 / sample_rate);
        Self {
            delay: StereoDelay::new(sample_rate),
            bender: SpoonBenderBus::new(sample_rate),
            send: 0.0,
            send_target: 0.0,
            send_step: 1.0 / (1.0 + sample_rate * 0.01),
            dc_input: [0.0; 2],
            dc_output: [0.0; 2],
            dc_pole,
            dc_gain: (1.0 + dc_pole) * 0.5,
            peaks: MasterPeaks::default(),
        }
    }

    pub(super) fn set_send(&mut self, send: f32) {
        self.send_target = finite_sample(send).clamp(0.0, 1.0) * MAX_SEND;
    }

    pub(super) fn set_bender_params(&mut self, params: BenderParams) {
        self.bender.set_params(params);
    }

    pub(super) fn set_delay_mode(&mut self, mode: DelayMode) {
        self.delay.set_mode(mode);
    }

    pub(super) fn reset(&mut self, send: f32) {
        self.set_send(send);
        self.send = self.send_target;
        self.delay.clear();
        self.bender.reset();
        self.dc_input = [0.0; 2];
        self.dc_output = [0.0; 2];
        self.peaks = MasterPeaks::default();
    }

    pub(super) fn next_send(&mut self) -> f32 {
        let send_delta = self.send_target - self.send;
        self.send = if send_delta.abs() < 1e-6 {
            self.send_target
        } else {
            self.send + self.send_step * send_delta
        };
        self.send
    }

    pub(super) fn process(
        &mut self,
        dry: [f32; 2],
        delay_send: [f32; 2],
        fx_send: [f32; 2],
    ) -> ([f32; 2], [f32; 2]) {
        let mix = self.delay.mix(dry.map(finite_sample), delay_send);
        let wet = self.bender.process(fx_send);
        for (channel, sample) in mix.into_iter().enumerate() {
            let sample = sample + wet[channel];
            self.dc_output[channel] = flush_tail(
                (sample - self.dc_input[channel]) * self.dc_gain
                    + self.dc_pole * self.dc_output[channel],
            );
            self.dc_input[channel] = sample;
        }

        let levels = self.dc_output.map(f32::abs);
        let gain = limiter_gain(levels[0].max(levels[1]));
        let output = self
            .dc_output
            .map(|sample| (sample * gain).clamp(-1.0, 1.0));
        self.peaks.merge(MasterPeaks {
            output: output.map(f32::abs),
            clipped: levels.map(|level| level > 1.0),
        });
        (output, wet)
    }
}

fn limiter_gain(peak: f32) -> f32 {
    if peak <= KNEE {
        return 1.0;
    }
    // Unit slope at the knee, approaching full scale as the input grows.
    let headroom = 1.0 - KNEE;
    let excess = peak - KNEE;
    let limited = KNEE + headroom * (excess / (headroom + excess));
    limited / peak
}

fn finite_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

fn flush_tail(sample: f32) -> f32 {
    if sample.abs() < 1e-20 {
        0.0
    } else {
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(bus: &mut MasterBus, dry: [f32; 2]) -> [f32; 2] {
        let send = bus.next_send();
        bus.process(dry, dry.map(|sample| sample * send), [0.0; 2]).0
    }

    #[test]
    fn impulse_crosses_channels_at_the_expected_times() {
        for (rate, left_frames, right_frames) in [
            (44_100.0_f32, 750, 926),
            (48_000.0_f32, 816, 1008),
            (96_000.0_f32, 1632, 2016),
        ] {
            let mut bus = MasterBus::new(rate);
            bus.reset(1.0);
            let direct = render(&mut bus, [0.25, 0.0]);
            assert!(direct[0] > 0.24 && direct[1] == 0.0);
            for _ in 1..right_frames {
                assert_eq!(render(&mut bus, [0.0; 2])[1], 0.0);
            }
            let first_echo = render(&mut bus, [0.0; 2]);
            assert!(first_echo[0].abs() < 0.001 && first_echo[1] > 0.11);
            for _ in 1..left_frames {
                assert!(render(&mut bus, [0.0; 2])[0].abs() < 0.001);
            }
            let bounce = render(&mut bus, [0.0; 2]);
            assert!(bounce[0] > 0.03 && bounce[1].abs() < 0.001);
        }
    }

    #[test]
    fn long_ping_pong_crosses_channels_and_keeps_the_delay_ratio() {
        for (rate, left_frames, right_frames) in [
            (44_100.0, 5513, 7350),
            (48_000.0, 6000, 8000),
            (96_000.0, 12000, 16000),
            (192_000.0, 12288, 16384),
        ] {
            let mut delay = StereoDelay::new(rate);
            delay.set_mode(DelayMode::LongPingPong);
            assert_eq!(delay.process([-0.5, 0.0]), [0.0; 2]);
            for _ in 1..right_frames {
                assert_eq!(delay.process([0.0; 2]), [0.0; 2]);
            }
            assert_eq!(delay.process([0.0; 2]), [0.0, -0.5]);
            for _ in 1..left_frames {
                assert_eq!(delay.process([0.0; 2]), [0.0; 2]);
            }
            let [left, right] = delay.process([0.0; 2]);
            assert!((left + 0.24).abs() < 1.0 / Q14_SCALE);
            assert_eq!(right, 0.0);

            delay.set_mode(DelayMode::Slapback);
            for _ in 0..SLAPBACK_FRAMES {
                assert_eq!(delay.process([0.0; 2]), [0.0; 2]);
            }
        }
    }

    #[test]
    fn limiter_links_gain_and_records_overs_before_limiting() {
        let mut bus = MasterBus::new(48_000.0);
        let [left, right] = render(&mut bus, [1.5, -0.5]);
        assert!((KNEE..=1.0).contains(&left));
        assert!((left + 3.0 * right).abs() < 1e-6);
        render(&mut bus, [0.0; 2]);
        assert_eq!(bus.peaks.output, [left, right.abs()]);
        assert_eq!(bus.peaks.clipped, [true, false]);
    }

    #[test]
    fn dc_blocker_settles_and_reset_clears_delay_and_peaks() {
        let mut bus = MasterBus::new(48_000.0);
        for _ in 0..24_000 {
            render(&mut bus, [0.3, -0.3]);
        }
        let settled = render(&mut bus, [0.3, -0.3]);
        assert!(settled.iter().all(|sample| sample.abs() < 1e-6));

        bus.reset(1.0);
        render(&mut bus, [1.5, 0.0]);
        bus.reset(0.0);
        assert_eq!(bus.peaks, MasterPeaks::default());
        for _ in 0..SLAPBACK_FRAMES {
            assert_eq!(render(&mut bus, [0.0; 2]), [0.0; 2]);
        }
    }
}
