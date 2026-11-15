//! Tuning and tail checks. Tuning: each pitched machine at its defaults,
//! MIDI 36-96 in fifths, at 44.1 / 48 / 96 kHz; the fundamental is estimated
//! by autocorrelation of 0.25 s of sustain and reported in cents. Tails: CPU
//! per second while six lanes ring out into silence after one hit.
//!
//! `cargo run --release -p leviathan-dsp --example tuning_tails`

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{LanePatch, MachineKind, SynthRack};

fn render(machine: MachineKind, midi: f32, rate: f32, seconds: f32) -> Vec<f32> {
    let mut rack = SynthRack::new(rate);
    let mut patch = LanePatch::for_machine(machine);
    patch.params[0] = patch.params[0].max(0.8); // long decay where slot 0 is decay
    rack.set_lane_patch(0, patch);
    rack.reset_voices();
    let mut stems = [[0.0; 2]; NUM_LANES];
    let mut cv = CvFrame::default();
    cv.pitch_v[0] = (midi - 60.0) / 12.0;
    (0..(rate * seconds) as usize)
        .map(|_| {
            cv.gate_v[0] = 10.0;
            cv.velocity_v[0] = 10.0;
            rack.render_stereo_with_stems(&cv, &mut stems);
            stems[0][0] + stems[0][1]
        })
        .collect()
}

/// Period by normalised autocorrelation, refined parabolically.
fn pitch(x: &[f32], rate: f32, expect: f32) -> Option<f32> {
    let lo = (rate / (expect * 1.12)) as usize;
    let hi = (rate / (expect / 1.12)) as usize + 2;
    let n = x.len() - hi - 2;
    let ac = |lag: usize| -> f64 {
        let (mut num, mut a, mut b) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            num += f64::from(x[i] * x[i + lag]);
            a += f64::from(x[i] * x[i]);
            b += f64::from(x[i + lag] * x[i + lag]);
        }
        num / (a * b).sqrt().max(1e-30)
    };
    let values: Vec<f64> = (lo.max(2) - 1..=hi + 1).map(ac).collect();
    let (best, _) = values[1..values.len() - 1].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let i = best + 1;
    if values[i] < 0.5 {
        return None;
    }
    let (a, b, c) = (values[i - 1], values[i], values[i + 1]);
    let d = 0.5 * (a - c) / (a - 2.0 * b + c);
    let lag = (lo.max(2) - 1 + i) as f64 + d;
    Some(rate / lag as f32)
}

fn main() {
    let pitched = [
        MachineKind::FourOpFm, MachineKind::SubBass, MachineKind::AcidLead, MachineKind::ChordStab,
        MachineKind::CzPhaseDist, MachineKind::PolivoksSub, MachineKind::VastFunSynth,
        MachineKind::WestCoastLpg, MachineKind::OtisTranswave, MachineKind::ByteBusR2r,
        MachineKind::OtisTwin, MachineKind::Fs77FormantFm, MachineKind::VastTriple,
    ];
    println!("tuning, cents (MIDI 36 43 50 57 64 71 78 85 92; '--' = no stable period)");
    for machine in pitched {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut row = Vec::new();
            for midi in (36..=92).step_by(7) {
                let x = render(machine, midi as f32, rate, 0.4);
                let hz = 261.625_58 * 2f32.powf((midi as f32 - 60.0) / 12.0);
                let expect = if machine == MachineKind::SubBass { hz * 0.5 } else { hz };
                let start = (rate * 0.1) as usize;
                let cents = pitch(&x[start..start + (rate * 0.25) as usize], rate, expect)
                    .map(|f| 1200.0 * (f / expect).log2());
                row.push(cents.map_or("   --".to_string(), |c| format!("{c:+5.1}")));
            }
            println!("{:6} {:6} {}", machine.spec().slug, rate as u32, row.join(" "));
        }
    }
    println!("\ntails: six lanes, one hit each, then 20 s of ringing out (us per audio second, per 2 s block)");
    for machine in MachineKind::ALL {
        if machine == MachineKind::OtisKit || machine == MachineKind::VarSampler {
            continue;
        }
        let mut rack = SynthRack::new(48_000.0);
        for lane in 0..NUM_LANES {
            rack.set_lane_patch(lane, LanePatch::for_machine(machine));
        }
        rack.reset_voices();
        let mut cv = CvFrame::default();
        let mut blocks = Vec::new();
        let mut last = 0.0f32;
        for block in 0..10 {
            let t = std::time::Instant::now();
            for n in 0..96_000 {
                let on = block == 0 && n < 2_400;
                for lane in 0..NUM_LANES {
                    cv.gate_v[lane] = if on { 10.0 } else { 0.0 };
                    cv.velocity_v[lane] = 10.0;
                    cv.pitch_v[lane] = lane as f32 * 0.2 - 1.0;
                }
                last = rack.render_stereo(&cv).0;
            }
            blocks.push(t.elapsed().as_secs_f64() * 1e6 / 2.0);
        }
        let first = blocks[1];
        let worst = blocks[2..].iter().cloned().fold(0.0, f64::max);
        let flag = if worst > first * 1.5 { "  <-- tail cost rises" } else { "" };
        println!("{:6} {:>7.0} early {:>7.0} worst late, last sample {:e}{flag}", machine.spec().slug, first, worst, last);
    }
}
