//! Sample-rate dependence survey: every voice (except the sample players) at
//! its defaults and with each macro at 0 and 1, rendered at 44.1, 48 and 96
//! kHz on low notes (lane stem, mono f32 raw). 48 kHz is the reference the
//! voices were tuned at; `rate_survey.py` compares the others to it.
//!
//! `cargo run --release -p leviathan-dsp --example rate_survey -- <out-dir> [machine-slug]`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{LanePatch, MachineKind, SynthRack};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "rate-survey".into());
    let only = args.next();
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    for machine in MachineKind::ALL {
        if matches!(machine, MachineKind::VarSampler | MachineKind::OtisKit) {
            continue;
        }
        let slug = machine.spec().slug;
        if only.as_deref().is_some_and(|o| o != slug) {
            continue;
        }
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
            for midi in [36u8, 48] {
                for rate in [44_100u32, 48_000, 96_000] {
                    let mut rack = SynthRack::new(rate as f32);
                    let mut patch = LanePatch::for_machine(machine);
                    patch.params = *params;
                    rack.set_lane_patch(0, patch);
                    rack.reset_voices();
                    let mut stems = [[0.0; 2]; NUM_LANES];
                    let mut cv = CvFrame::default();
                    cv.pitch_v[0] = (f32::from(midi) - 60.0) / 12.0;
                    let frames = (0.6 * rate as f32) as usize;
                    let mut f = BufWriter::new(File::create(out.join(format!("{slug}-{name}-{midi}-{rate}.f32")))?);
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
