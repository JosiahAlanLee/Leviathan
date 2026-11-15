//! Aliasing survey: each voice at its defaults and with each macro at 0 and
//! 1, sustained notes rendered at 48 kHz and at 192 kHz (lane stem, mono f32
//! raw files). Content that is part of the voice is the same at both rates;
//! fold-back (aliasing) is not. `analyze.py` in the survey folder compares.
//!
//! `cargo run --release -p leviathan-dsp --example voice_quality -- <out-dir>`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{LanePatch, MachineKind, SynthRack};

const MACHINES: [MachineKind; 6] = [
    MachineKind::OtisTwin,
    MachineKind::Fs77FormantFm,
    MachineKind::FourOpFm,
    MachineKind::CzPhaseDist,
    MachineKind::VastTriple,
    MachineKind::AcidLead,
];
const NOTES: [u8; 3] = [36, 60, 84];
const SECONDS: f32 = 0.5;

fn main() -> std::io::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "voice-quality".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    for machine in MACHINES {
        let defaults = LanePatch::for_machine(machine).params;
        let mut variants = vec![("default".to_string(), defaults)];
        for slot in 0..8 {
            for (tag, value) in [("lo", 0.0), ("hi", 1.0)] {
                let mut p = defaults;
                p[slot] = value;
                variants.push((format!("p{slot}{tag}"), p));
            }
        }
        for (name, params) in &variants {
            for midi in NOTES {
                for rate in [48_000u32, 192_000] {
                    let mut rack = SynthRack::new(rate as f32);
                    let mut patch = LanePatch::for_machine(machine);
                    patch.params = *params;
                    rack.set_lane_patch(0, patch);
                    rack.reset_voices();
                    let mut stems = [[0.0; 2]; NUM_LANES];
                    let mut cv = CvFrame::default();
                    cv.pitch_v[0] = (f32::from(midi) - 60.0) / 12.0;
                    let frames = (SECONDS * rate as f32) as usize;
                    let mut f = BufWriter::new(File::create(out.join(format!(
                        "{}-{name}-{midi}-{rate}.f32",
                        machine.short_name().to_lowercase()
                    )))?);
                    for n in 0..frames {
                        let on = n >= rate as usize / 750;
                        cv.gate_v[0] = if on { 10.0 } else { 0.0 };
                        cv.velocity_v[0] = if on { 10.0 } else { 0.0 };
                        rack.render_stereo_with_stems(&cv, &mut stems);
                        f.write_all(&(stems[0][0] + stems[0][1]).to_le_bytes())?;
                    }
                    f.flush()?;
                }
            }
        }
    }
    Ok(())
}
