//! OtisKit parity renders: one test sample in slot 1, one hit per case, from
//! the lane stem, as mono 32-bit float WAVs. Also writes the sample as raw
//! 16-bit PCM for the VCV host. Cases match `src/vcv_kit.cpp`.
//!
//! `cargo run --release -p leviathan-dsp --example parity_kit -- <out-dir>`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::otis_kit::{self, Lane, Memory, Sample, Slot, VelocityCurve};
use leviathan_dsp::{CaptureSamples, LanePatch, MachineKind, SynthRack};

const RATE: u32 = 48_000;

/// (name, velocity 0-127, tune, decay, level, filter, start, reverse, clock, drop-sample)
type Case = (&'static str, u8, f32, f32, f32, f32, f32, bool, u8, bool);
const CASES: [Case; 17] = [
    ("ref", 127, 0.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("vel110", 110, 0.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("vel94", 94, 0.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("vel52", 52, 0.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("level50", 127, 0.0, 1.0, 0.5, 1.0, 0.0, false, 1, false),
    ("tune+12", 127, 12.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("tune-12", 127, -12.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("tune+7", 127, 7.0, 1.0, 1.0, 1.0, 0.0, false, 1, false),
    ("filter50", 127, 0.0, 1.0, 1.0, 0.5, 0.0, false, 1, false),
    ("filter25", 127, 0.0, 1.0, 1.0, 0.25, 0.0, false, 1, false),
    ("decay30", 127, 0.0, 0.3, 1.0, 1.0, 0.0, false, 1, false),
    ("start50", 127, 0.0, 1.0, 1.0, 1.0, 0.5, false, 1, false),
    ("reverse", 127, 0.0, 1.0, 1.0, 1.0, 0.0, true, 1, false),
    ("clock29k", 127, 0.0, 1.0, 1.0, 1.0, 0.0, false, 0, false),
    ("clock78k", 127, 0.0, 1.0, 1.0, 1.0, 0.0, false, 2, false),
    ("clock29k-tune+12", 127, 12.0, 1.0, 1.0, 1.0, 0.0, false, 0, false),
    ("drop29k-tune+7", 127, 7.0, 1.0, 1.0, 1.0, 0.0, false, 0, true),
];

/// 0.5 s at 48 kHz: a 220 Hz band-limited saw with a 2.5 kHz click and a short noise burst.
fn test_sample() -> Vec<i16> {
    let n = 24_000;
    let mut noise = 0x9e37_79b9u32;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let mut saw = 0.0;
            for h in 1..40 {
                let f = 220.0 * h as f32;
                if f < 20_000.0 {
                    saw += (core::f32::consts::TAU * f * t).sin() / h as f32;
                }
            }
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let white = noise as f32 / u32::MAX as f32 * 2.0 - 1.0;
            let click = (core::f32::consts::TAU * 2_500.0 * t).sin() * (-t * 60.0).exp();
            let burst = white * (-t * 40.0).exp();
            let x = 0.45 * saw * (-t * 3.0).exp() + 0.3 * click + 0.25 * burst;
            (x.clamp(-1.0, 1.0) * 30_000.0) as i16
        })
        .collect()
}

struct One(Vec<i16>);
impl Memory for One {
    fn current(&self, slot: usize) -> Option<Sample<'_>> {
        (slot == 0).then_some(Sample { generation: 1, rate: RATE, pcm: &self.0 })
    }
    fn previous(&self, _: usize, _: usize, _: u64) -> Option<Sample<'_>> {
        None
    }
}

fn write_f32(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    let bytes = (samples.len() * 4) as u32;
    for chunk in [&b"RIFF"[..], &(36 + bytes).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(),
        &3u16.to_le_bytes(), &1u16.to_le_bytes(), &RATE.to_le_bytes(), &(RATE * 4).to_le_bytes(),
        &4u16.to_le_bytes(), &32u16.to_le_bytes(), b"data", &bytes.to_le_bytes()] {
        f.write_all(chunk)?;
    }
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

fn main() -> std::io::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "parity-kit".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let memory = One(test_sample());
    let mut raw = BufWriter::new(File::create(out.join("kit-sample-48k.s16"))?);
    for s in &memory.0 {
        raw.write_all(&s.to_le_bytes())?;
    }
    raw.flush()?;
    for (name, velocity, tune, decay, level, filter, start, reverse, clock, drop) in CASES {
        let mut rack = SynthRack::new(RATE as f32);
        rack.set_lane_patch(0, LanePatch::for_machine(MachineKind::OtisKit));
        let mut slots = [Slot::default(); otis_kit::SLOTS];
        slots[0] = Slot { tune, decay, level, filter, start, reverse };
        let lane = Lane { clock, drop_samples: drop, velocity: VelocityCurve::Chip, ..Lane::default() };
        rack.set_kit_controls(slots, [lane; NUM_LANES]);
        rack.reset_voices();
        let samples = CaptureSamples { kit: Some(&memory), ..CaptureSamples::default() };
        let mut stems = [[0.0; 2]; NUM_LANES];
        let mut delays = Box::new(std::array::from_fn(|_| leviathan_dsp::StereoDelay::new(RATE as f32)));
        let mut cv = CvFrame::default();
        let mut mono = Vec::with_capacity(36_000);
        for n in 0..36_000 {
            let on = (64..2_464).contains(&n);
            cv.gate_v[0] = if on { 10.0 } else { 0.0 };
            cv.velocity_v[0] = if on { 10.0 * f32::from(velocity) / 127.0 } else { 0.0 };
            rack.render_session_with_echo_stems(&cv, &samples, &mut stems, &mut delays, &mut []);
            mono.push(stems[0][0] + stems[0][1]);
        }
        write_f32(&out.join(format!("lev-{name}.wav")), &mono)?;
    }
    Ok(())
}
