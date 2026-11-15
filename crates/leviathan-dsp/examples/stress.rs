//! Stress test: every machine with random and extreme macros, pitches across
//! the CV range, fast retriggers and machine switches, at 44.1, 48 and 96 kHz.
//! Reports non-finite output, peaks, the largest sample-to-sample step at a
//! note onset relative to the lane's level, and CPU per voice-second.
//!
//! `cargo run --release -p leviathan-dsp --example stress`

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::otis_kit::{self, Memory, Sample};
use leviathan_dsp::{CaptureSamples, LanePatch, MachineKind, StereoDelay, SynthRack};

struct Kit(Vec<i16>);
impl Memory for Kit {
    fn current(&self, _: usize) -> Option<Sample<'_>> {
        Some(Sample { generation: 1, rate: 44_100, pcm: &self.0 })
    }
    fn previous(&self, _: usize, _: usize, _: u64) -> Option<Sample<'_>> {
        None
    }
}

fn xorshift(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state as f32 / u32::MAX as f32
}

fn main() {
    let kit = Kit((0..22_050).map(|i| ((i as f32 * 0.05).sin() * 20_000.0 * (1.0 - i as f32 / 22_050.0)) as i16).collect());
    println!("{:6} {:7} {:>8} {:>9} {:>9} {:>10} {:>8}", "voice", "rate", "nonfin", "peak", "rms dB", "onset/p99.9", "us/vs");
    let mut worst = Vec::new();
    for machine in MachineKind::ALL {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut rng = 0x1234_5678u32 ^ (machine as u32 * 7919);
            let mut rack = SynthRack::new(rate);
            rack.set_lane_patch(0, LanePatch::for_machine(machine));
            rack.set_kit_controls([otis_kit::Slot::default(); otis_kit::SLOTS], [otis_kit::Lane::default(); NUM_LANES]);
            rack.reset_voices();
            let samples = CaptureSamples { kit: Some(&kit), ..CaptureSamples::default() };
            let mut stems = [[0.0; 2]; NUM_LANES];
            let mut delays: Box<[StereoDelay; NUM_LANES]> = Box::new(std::array::from_fn(|_| StereoDelay::new(rate)));
            let mut cv = CvFrame::default();
            let (mut nonfinite, mut peak, mut energy, mut count) = (0usize, 0.0f32, 0.0f64, 0usize);
            let mut prev = 0.0f32;
            let mut onset_jump = 0.0f32;
            let mut steady: Vec<f32> = Vec::new();
            let frames = (rate * 8.0) as usize; // eight seconds
            let note = (rate * 0.125) as usize;
            let start = std::time::Instant::now();
            for n in 0..frames {
                if n % note == 0 {
                    let mut patch = LanePatch::for_machine(machine);
                    for p in patch.params.iter_mut() {
                        let r = xorshift(&mut rng);
                        *p = if r < 0.15 { 0.0 } else if r > 0.85 { 1.0 } else { xorshift(&mut rng) };
                    }
                    rack.set_lane_patch(0, patch);
                    cv.pitch_v[0] = -4.0 + 8.0 * xorshift(&mut rng);
                    cv.velocity_v[0] = 10.0 * xorshift(&mut rng);
                }
                // Gate on for 70 % of each note, with a ratchet-style retrigger every fourth note.
                let pos = n % note;
                cv.gate_v[0] = if pos < note * 7 / 10 { 10.0 } else { 0.0 };
                cv.retrigger_mask = u8::from((n / note) % 4 == 3 && pos == note / 3);
                let ((l, r), _) = rack.render_session_with_echo_stems(&cv, &samples, &mut stems, &mut delays, &mut []);
                let x = stems[0][0] + stems[0][1];
                if !(x.is_finite() && l.is_finite() && r.is_finite()) {
                    nonfinite += 1;
                    continue;
                }
                peak = peak.max(x.abs());
                energy += f64::from(x * x);
                count += 1;
                let step = (x - prev).abs();
                if pos < 2 || (cv.retrigger_mask != 0) {
                    onset_jump = onset_jump.max(step);
                } else if pos > note / 10 && pos < note * 6 / 10 {
                    steady.push(step);
                }
                prev = x;
            }
            let elapsed = start.elapsed().as_secs_f64();
            let rms = (energy / count.max(1) as f64).sqrt() as f32;
            steady.sort_by(f32::total_cmp);
            let typical = steady.get(steady.len() * 999 / 1000).copied().unwrap_or(0.0).max(1e-6);
            let jump = onset_jump / typical;
            let us = elapsed * 1e6 / 8.0;
            println!("{:6} {:7} {:>8} {:>9.3} {:>9.1} {:>9.1}x {:>8.0}", machine.spec().slug, rate as u32, nonfinite, peak, 20.0 * rms.max(1e-9).log10(), jump, us);
            worst.push((nonfinite, peak, machine.spec().slug, rate));
        }
    }
    let bad: Vec<_> = worst.iter().filter(|w| w.0 > 0 || w.1 > 4.0).collect();
    println!("non-finite or peak > 4: {bad:?}");
}
