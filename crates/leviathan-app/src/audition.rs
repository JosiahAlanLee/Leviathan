use std::ops::Range;
use std::sync::Arc;

use crate::capture::Take;
use crate::kit::KitSample;

pub enum Sample {
    Take(Arc<Take>),
    Kit(Arc<KitSample>),
}

pub struct Request {
    pub sample: Sample,
    pub range: Range<usize>,
    pub reverse: bool,
}

impl Request {
    fn rate(&self) -> u32 {
        match &self.sample {
            Sample::Take(take) => take.rate,
            Sample::Kit(sample) => sample.rate,
        }
    }

    fn frame(&self, i: usize) -> [f32; 2] {
        let absolute = if self.reverse {
            self.range.end - 1 - i
        } else {
            self.range.start + i
        };
        let pair = match &self.sample {
            Sample::Take(take) => take.frames[absolute],
            Sample::Kit(sample) => [sample.pcm[absolute]; 2],
        };
        pair.map(|v| f32::from(v) * (1.0 / 32768.0))
    }
}

#[derive(Default)]
pub struct Player {
    request: Option<Arc<Request>>,
    position: f64,
}

impl Player {
    pub fn start(&mut self, request: Option<Arc<Request>>) {
        self.request = request.filter(|r| !r.range.is_empty());
        self.position = 0.0;
    }

    pub fn step(&mut self, rate: f64) -> [f32; 2] {
        let Some(request) = &self.request else {
            return [0.0; 2];
        };
        let len = request.range.len();
        if self.position >= len as f64 {
            self.request = None;
            return [0.0; 2];
        }
        let index = self.position as usize;
        let fraction = (self.position - index as f64) as f32;
        let [a, b] = [
            request.frame(index),
            request.frame((index + 1).min(len - 1)),
        ];
        let ramp = (f64::from(request.rate()) * 0.003)
            .min(len as f64 * 0.5)
            .max(1.0);
        let stride = f64::from(request.rate()) / rate;
        // A slice that fits in one output frame would otherwise be entirely
        // silent: the ramp starts at zero and the next sample is out of range.
        let gain = if stride >= len as f64 {
            0.55
        } else {
            (self.position.min(len as f64 - self.position) / ramp).min(1.0) as f32 * 0.55
        };
        self.position += stride;
        std::array::from_fn(|ch| (a[ch] + (b[ch] - a[ch]) * fraction) * gain)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_high_rate_audition_emits_one_frame_instead_of_silence() {
        for (rate, audible_on) in [(192_000, 0), (48_000, 1)] {
            let sample = KitSample::new("tick".into(), rate, vec![16_384; 2]).unwrap();
            let mut player = Player::default();
            player.start(Some(Arc::new(Request { sample: Sample::Kit(sample), range: 0..2, reverse: false })));
            let frames = std::array::from_fn::<_, 3, _>(|_| player.step(48_000.0));
            assert!(frames[audible_on][0] > 0.2);
            assert_eq!(frames[2], [0.0; 2]);
        }
    }
}
