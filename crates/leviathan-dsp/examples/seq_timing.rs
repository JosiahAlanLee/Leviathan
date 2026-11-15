//! Sequencer timing: gate onsets against their ideal times for straight
//! 16ths, swing, nudges and ratchets at 44.1, 48 and 96 kHz, plus drift over
//! a ten-minute loop at 120 bpm.
//!
//! `cargo run --release -p leviathan-dsp --example seq_timing`

use leviathan_core::{
    EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain, PlaybackRate, SongPlayer,
};

fn onsets(pattern: &Pattern, rate: f64, bpm: f64, seconds: f64) -> Vec<f64> {
    let mut chain = PatternChain::default();
    chain.push(0);
    let mut player = SongPlayer::new(EngineConfig { bpm, sample_rate: rate, ..EngineConfig::default() });
    player.set_chain(chain);
    player.launch(LaunchTarget::Chain, 1);
    player.evaluator.playing = true;
    let patterns = [pattern.clone()];
    let mut high = false;
    let mut out = Vec::new();
    for n in 0..(rate * seconds) as usize {
        let (cv, _) = player.step_sample(&patterns);
        let gate = cv.gate_v[0] > 2.5;
        if (gate && !high) || cv.retrigger_mask & 1 != 0 {
            out.push(n as f64 / rate);
        }
        high = gate;
    }
    out
}

fn main() {
    let bpm = 120.0;
    let row_seconds = 60.0 / bpm / 4.0; // OneX: four rows per beat
    let note = NoteCell { active: true, midi: 60, velocity: 100, gate_fraction: 0.5, ..NoteCell::default() };
    let base = || Pattern { length: 16, rate: PlaybackRate::OneX, ..Pattern::default() };

    let mut straight = base();
    let mut swing = base();
    swing.swing = 0.66;
    let mut nudged = base();
    let mut ratchet = base();
    for row in 0..16 {
        straight.lanes[0][row] = note;
        swing.lanes[0][row] = note;
        nudged.lanes[0][row] = NoteCell { micro_offset: [0, 12, -12, 24, -24, 6, 0, 0][row % 8], ..note };
        if row % 4 == 0 {
            ratchet.lanes[0][row] = NoteCell { retrig: if row == 0 { 1 } else { row as u8 / 4 + 1 }, ..note };
        }
    }
    // Ideal onset times within one bar (seconds).
    let ideal_straight: Vec<f64> = (0..16).map(|r| r as f64 * row_seconds).collect();
    let ideal_swing: Vec<f64> = (0..16).map(|r| (r as f64 + if r % 2 == 1 { 0.66 * 2.0 - 1.0 } else { 0.0 }) * row_seconds).collect();
    // Rows 3 (+24) and 4 (-24) meet at 3.5 rows; the later note takes over, so one onset.
    let mut ideal_nudged: Vec<f64> = (0..16).map(|r| (r as f64 + [0, 12, -12, 24, -24, 6, 0, 0][r % 8] as f64 / 48.0) * row_seconds).collect();
    ideal_nudged.sort_by(f64::total_cmp);
    ideal_nudged.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut ideal_ratchet = Vec::new();
    for (k, row) in [0usize, 4, 8, 12].into_iter().enumerate() {
        let hits = if k == 0 { 1 } else { k + 1 };
        for h in 0..hits {
            ideal_ratchet.push((row as f64 + h as f64 / hits as f64) * row_seconds);
        }
    }
    println!("{:9} {:6} {:>6} {:>12} {:>12}", "case", "rate", "hits", "worst ms", "mean ms");
    for (name, pattern, ideal) in [
        ("straight", &straight, &ideal_straight),
        ("swing 66", &swing, &ideal_swing),
        ("nudges", &nudged, &ideal_nudged),
        ("ratchets", &ratchet, &ideal_ratchet),
    ] {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let got = onsets(pattern, rate, bpm, 2.0); // one bar
            let mut worst: f64 = 0.0;
            let mut sum = 0.0;
            for (g, i) in got.iter().zip(ideal.iter()) {
                let e = (g - i).abs() * 1000.0;
                worst = worst.max(e);
                sum += e;
            }
            let flag = if got.len() != ideal.len() || worst > 1000.0 / rate * 1.01 { "  <--" } else { "" };
            println!("{:9} {:6} {:>3}/{:<2} {:>12.4} {:>12.4}{flag}", name, rate as u32, got.len(), ideal.len(), worst, sum / got.len().max(1) as f64);
        }
    }
    println!("\nten-minute drift, straight 16ths at {bpm} bpm:");
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        let got = onsets(&straight, rate, bpm, 600.0);
        let last = *got.last().unwrap();
        let index = got.len() - 1;
        let ideal = index as f64 * row_seconds;
        println!("  {:6}: {} onsets, last at {:.6} s, ideal {:.6} s, error {:+.4} ms", rate as u32, got.len(), last, ideal, (last - ideal) * 1000.0);
    }
}
