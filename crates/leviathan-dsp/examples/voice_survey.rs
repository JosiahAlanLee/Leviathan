//! Measures every machine: tuning against 12-TET, level, decay and macro sensitivity.
//! `cargo run --release -p leviathan-dsp --example voice_survey`

mod lab;

use lab::*;
use leviathan_core::{PlaybackRate, NUM_LANES, SOUND_PARAMS};
use leviathan_dsp::{LanePatch, MachineKind};

fn one_note(machine: MachineKind, params: &[(usize, f32)], midi: u8, gate: f32) -> Vec<[f32; 2]> {
    let mut patches = [LanePatch::for_machine(MachineKind::KickDrum); NUM_LANES];
    patches[0] = patch(machine, params);
    let mut scene = default_scene(patches);
    scene.channels[0] = channel(1.0, 0.0, 0.0, 0.0);
    let mut pattern = empty();
    pattern.rate = PlaybackRate::Quarter;
    pattern.length = 4;
    put(&mut pattern, 0, 0, note(midi, 110, gate));
    let mut song = Song::new(120.0);
    song.tail_seconds = 1.0;
    song.play(pattern, scene, 1);
    render(&song, false).master
}

fn tuning(signal: &[f32], midi: u8) -> Option<f32> {
    let want = midi_hz(f32::from(midi));
    let from = RATE as usize / 25;
    let to = (from + RATE as usize / 4).min(signal.len());
    let got = f0(&signal[from..to], want * 0.45, want * 2.2)?;
    Some(1200.0 * (got / want).log2())
}

fn main() {
    println!("TUNING (cents against A440 12-TET; '-' = no clear pitch), default patch, long gate");
    print!("{:<6}", "voice");
    let notes = [24u8, 31, 36, 43, 47, 48, 55, 59, 60, 67, 71, 72, 79, 84];
    for midi in notes {
        print!("{midi:>7}");
    }
    println!();
    for machine in MachineKind::ALL {
        print!("{:<6}", machine.spec().badge);
        for midi in notes {
            let audio = mono(&one_note(machine, &[], midi, 0.9));
            match tuning(&audio, midi) {
                Some(cents) => print!("{cents:>7.1}"),
                None => print!("{:>7}", "-"),
            }
        }
        println!();
    }

    println!("\nLEVEL at MIDI 48, velocity 110, lane level 1.0 centre, default patch (0.5 s gate)");
    println!("{:<6}{:>9}{:>9}{:>12}{:>10}", "voice", "peak dB", "rms dB", "t(-30dB) s", "bright Hz");
    for machine in MachineKind::ALL {
        let audio = one_note(machine, &[], 48, 0.25);
        let half = &audio[..RATE as usize / 2];
        let signal = mono(&audio);
        let top = peak(&audio);
        let mut last = 0;
        for (i, chunk) in signal.chunks(480).enumerate() {
            let level = chunk.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            if level > top * 0.0316 {
                last = i;
            }
        }
        println!(
            "{:<6}{:>9.1}{:>9.1}{:>12.2}{:>10.0}",
            machine.spec().badge,
            db(top),
            db(rms(half)),
            (last + 1) as f32 * 0.01,
            brightness(&signal[..RATE as usize / 2])
        );
    }

    println!("\nMACRO SENSITIVITY at MIDI 48: rms dB / brightness Hz / cents for each macro at 0, 0.5, 1");
    for machine in MachineKind::ALL {
        println!("{} ({})", machine.spec().badge, machine.spec().name);
        for slot in 0..SOUND_PARAMS {
            print!("  P{slot} {:<18}", machine.spec().params[slot].name);
            for value in [0.0, 0.5, 1.0] {
                let audio = one_note(machine, &[(slot, value)], 48, 0.25);
                let signal = mono(&audio);
                let cents = tuning(&signal, 48).map_or("   -".to_string(), |c| format!("{c:>5.0}"));
                print!(
                    " | {:>6.1} {:>6.0} {}",
                    db(rms(&audio[..RATE as usize / 2])),
                    brightness(&signal[..RATE as usize / 2]),
                    cents
                );
            }
            println!();
        }
    }
}
