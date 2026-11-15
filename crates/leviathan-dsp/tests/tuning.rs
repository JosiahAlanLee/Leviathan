//! Tuning guard: calibrated voices must sound at the played pitch, at every
//! host rate. The period is found by normalised autocorrelation of a steady
//! stretch of a held note and refined parabolically.

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{LanePatch, MachineKind, SynthRack};

const TOLERANCE_CENTS: f64 = 1.5;

fn render(patch: LanePatch, midi: f32, rate: f32) -> Vec<f32> {
    let mut rack = SynthRack::new(rate);
    rack.set_voice_calibration(true);
    rack.set_lane_patch(0, patch);
    rack.reset_voices();
    let mut stems = [[0.0; 2]; NUM_LANES];
    let mut cv = CvFrame::default();
    cv.pitch_v[0] = (midi - 60.0) / 12.0;
    (0..(rate * 0.25) as usize)
        .map(|_| {
            cv.gate_v[0] = 10.0;
            cv.velocity_v[0] = 10.0;
            rack.render_stereo_with_stems(&cv, &mut stems);
            stems[0][0] + stems[0][1]
        })
        .collect()
}

fn period(x: &[f32], expect: f64) -> Option<f64> {
    let lo = (expect / 1.06) as usize;
    let hi = (expect * 1.06) as usize + 2;
    let n = x.len() - hi - 2;
    let ac = |lag: usize| {
        let (mut num, mut a, mut b) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let (p, q) = (f64::from(x[i]), f64::from(x[i + lag]));
            num += p * q;
            a += p * p;
            b += q * q;
        }
        num / (a * b).sqrt().max(1e-30)
    };
    let values: Vec<f64> = (lo - 1..=hi + 1).map(ac).collect();
    let i = (1..values.len() - 1).max_by(|&a, &b| values[a].total_cmp(&values[b]))?;
    if values[i] < 0.8 {
        return None;
    }
    let (a, b, c) = (values[i - 1], values[i], values[i + 1]);
    Some((lo - 1 + i) as f64 + 0.5 * (a - c) / (a - 2.0 * b + c))
}

#[test]
fn calibrated_voices_play_in_tune_at_every_rate() {
    // (machine, macro overrides, octave of the sounding fundamental)
    let cases: [(MachineKind, &[(usize, f32)], f64); 10] = [
        (MachineKind::FourOpFm, &[], 1.0),
        (MachineKind::SubBass, &[], 0.5),
        (MachineKind::AcidLead, &[], 1.0),
        (MachineKind::ChordStab, &[], 1.0),
        (MachineKind::PolivoksSub, &[], 1.0),
        (MachineKind::WestCoastLpg, &[], 1.0),
        (MachineKind::OtisTranswave, &[], 1.0),
        (MachineKind::Fs77FormantFm, &[], 1.0),
        // Modulation and unison spread move the pitch on purpose.
        (MachineKind::VastFunSynth, &[(3, 0.0)], 1.0),
        (MachineKind::OtisTwin, &[(6, 0.0)], 1.0),
    ];
    let mut failures = Vec::new();
    for (machine, overrides, octave) in cases {
        let mut patch = LanePatch::for_machine_with_calibration(machine);
        for &(slot, value) in overrides {
            patch.params[slot] = value;
        }
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            for midi in [43.0f32, 67.0] {
                let x = render(patch, midi, rate);
                let hz = 261.625_58 * 2f64.powf(f64::from(midi - 60.0) / 12.0) * octave;
                let expect = f64::from(rate) / hz;
                let start = (rate * 0.1) as usize;
                match period(&x[start..], expect) {
                    Some(p) => {
                        let cents = 1200.0 * (expect / p).log2();
                        if cents.abs() > TOLERANCE_CENTS {
                            failures.push(format!("{} MIDI {midi} @ {rate}: {cents:+.2} cents", machine.spec().slug));
                        }
                    }
                    None => failures.push(format!("{} MIDI {midi} @ {rate}: no stable period", machine.spec().slug)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
