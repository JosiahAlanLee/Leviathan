//! BBUS parity renders: one sustained note per pitch for each stage ablation,
//! straight from the lane stem (before the master bus), as mono 32-bit float
//! WAVs. The VCV side is rendered by a separate host; `analyze.py` compares.
//!
//! `cargo run --release -p leviathan-dsp --example parity_bbus -- <out-dir>`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{encode_bytebus_frame, CvFrame, NUM_LANES};
use leviathan_dsp::{LanePatch, MachineKind, SynthRack};

const RATE: u32 = 48_000;
const NOTES: [u8; 4] = [36, 48, 60, 72];
const SECONDS: f32 = 1.0;

// Decay, BusMask, FirTilt, R2rSkew, ClockHold, Cutoff, SlewGlitch, SubBit.
// Decay is 1.0 throughout so every variant sustains the same way.
const VARIANTS: [(&str, [f32; 8]); 6] = [
    ("default", [1.0, 0.45, 0.30, 0.20, 0.15, 0.60, 0.30, 0.35]),
    ("no-sub", [1.0, 0.45, 0.30, 0.20, 0.15, 0.60, 0.30, 0.0]),
    ("no-sub-clean-dac", [1.0, 0.45, 0.30, 0.0, 0.0, 0.60, 0.0, 0.0]),
    ("core-open-filter", [1.0, 0.45, 0.30, 0.0, 0.0, 1.0, 0.0, 0.0]),
    ("core-flat-weights", [1.0, 0.45, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    ("default-dirty-dac", [1.0, 0.45, 0.30, 0.6, 0.6, 0.60, 0.8, 0.35]),
];

fn write_f32(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    let bytes = (samples.len() * 4) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + bytes).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // IEEE float
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&RATE.to_le_bytes())?;
    f.write_all(&(RATE * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&bytes.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

fn main() -> std::io::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "parity-bbus".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    // None: the standalone generator. Some(0x01): the tracker's default word
    // while lane 1 plays (the lane gate mask), as in song playback.
    for (tag, word, grit) in [
        ("solo", None, false),
        ("w01", Some(0x01u8), false),
        ("grit-solo", None, true),
        ("grit-w01", Some(0x01u8), true),
    ] {
    for (name, params) in VARIANTS {
        for midi in NOTES {
            let mut rack = SynthRack::new(RATE as f32);
            let mut patch = LanePatch::for_machine(MachineKind::ByteBusR2r);
            patch.params = params;
            rack.set_lane_patch(0, patch);
            rack.set_bbus_grit_lanes(u8::from(grit));
            rack.reset_voices();
            let mut stems = [[0.0; 2]; NUM_LANES];
            let mut cv = CvFrame { run_v: if word.is_some() { 10.0 } else { 0.0 }, ..CvFrame::default() };
            if let Some(word) = word {
                cv.bytebus_v = encode_bytebus_frame(word, false);
            }
            cv.pitch_v[0] = (f32::from(midi) - 60.0) / 12.0;
            let frames = (SECONDS * RATE as f32) as usize;
            let mut mono = Vec::with_capacity(frames);
            for frame in 0..frames {
                let on = frame >= 64;
                cv.gate_v[0] = if on { 10.0 } else { 0.0 };
                cv.velocity_v[0] = if on { 10.0 } else { 0.0 };
                rack.render_stereo_with_stems(&cv, &mut stems);
                mono.push(stems[0][0] + stems[0][1]);
            }
            write_f32(&out.join(format!("lev-{tag}-{name}-{midi}.wav")), &mono)?;
        }
    }
    }
    Ok(())
}
