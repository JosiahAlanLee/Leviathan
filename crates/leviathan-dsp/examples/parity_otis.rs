//! OTIS parity renders. Writes the transwave ROM as raw 16-bit frames for the
//! VCV host, then sustained OTIS notes on static frames (no scanning) for each
//! variant, from the lane stem, as mono 32-bit float WAVs.
//!
//! `cargo run --release -p leviathan-dsp --example parity_otis -- <out-dir>`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{transwave_frame, LanePatch, MachineKind, SynthRack};

const RATE: u32 = 48_000;
const NOTES: [u8; 4] = [36, 48, 60, 72];
/// (table, frame) pairs: one bright and one plain frame per table.
pub const FRAMES: [(usize, usize); 4] = [(0, 15), (1, 0), (2, 7), (3, 12)];

// Decay, TableSel, FramePos, ScanDepth, K1Cutoff, K2Reso, FixedGrit, LoopMode.
fn variants(table: usize, frame: usize) -> [(&'static str, [f32; 8]); 5] {
    // With warm tables on, TableSel bands are eighths: bright in the lower
    // half of each quarter, the warm twin in the upper half.
    let t = (2.0 * table as f32 + 0.5) / 8.0;
    let w = (2.0 * table as f32 + 1.5) / 8.0;
    let p = frame as f32 / 15.0;
    [
        ("warm-clean", [1.0, w, p, 0.0, 1.0, 0.0, 0.0, 0.0]),
        ("clean", [1.0, t, p, 0.0, 1.0, 0.0, 0.0, 0.0]),
        ("default-filter", [1.0, t, p, 0.0, 0.65, 0.30, 0.0, 0.0]),
        ("grit25", [1.0, t, p, 0.0, 1.0, 0.0, 0.25, 0.0]),
        ("grit60", [1.0, t, p, 0.0, 1.0, 0.0, 0.60, 0.0]),
    ]
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
    let out = std::env::args().nth(1).unwrap_or_else(|| "parity-otis".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    // All 8 x 16 frames (4 bright, then their warm twins), table-major, little-endian i16.
    let mut rom = BufWriter::new(File::create(out.join("transwave-rom.s16"))?);
    for table in 0..8 {
        for frame in 0..16 {
            for s in transwave_frame(table, frame) {
                rom.write_all(&s.to_le_bytes())?;
            }
        }
    }
    rom.flush()?;
    for (table, frame) in FRAMES {
        for (name, params) in variants(table, frame) {
            for midi in NOTES {
                let mut rack = SynthRack::new(RATE as f32);
                let mut patch = LanePatch::for_machine(MachineKind::OtisTranswave);
                patch.params = params;
                rack.set_lane_patch(0, patch);
                rack.reset_voices();
                let mut stems = [[0.0; 2]; NUM_LANES];
                let mut cv = CvFrame::default();
                cv.pitch_v[0] = (f32::from(midi) - 60.0) / 12.0;
                let mut mono = Vec::with_capacity(RATE as usize);
                for n in 0..RATE as usize {
                    let on = n >= 64;
                    cv.gate_v[0] = if on { 10.0 } else { 0.0 };
                    cv.velocity_v[0] = if on { 10.0 } else { 0.0 };
                    rack.render_stereo_with_stems(&cv, &mut stems);
                    mono.push(stems[0][0] + stems[0][1]);
                }
                write_f32(&out.join(format!("lev-t{table}f{frame}-{name}-{midi}.wav")), &mono)?;
            }
        }
    }
    Ok(())
}
