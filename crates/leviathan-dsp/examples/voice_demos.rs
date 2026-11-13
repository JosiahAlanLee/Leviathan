//! Long demo songs that put all seventeen machines to work, rendered offline
//! through the same `SongPlayer` + `SynthRack` path as the app's WAV export.
//!
//! `cargo run --release -p leviathan-dsp --example voice_demos -- <out-dir> [track]`

mod lab;

use std::path::PathBuf;

use lab::*;
use leviathan_core::{Pattern, NUM_LANES};
use leviathan_dsp::{BenderParams, LanePatch, MachineKind};
use MachineKind::*;

const SNARE: f32 = 0.10;
const CLAP: f32 = 0.37;
const RIM: f32 = 0.62;
const HAT: f32 = 0.90;

/// Lane level that brings each machine's default patch to about the same RMS
/// (from `voice_survey`), so the mix numbers below are musical balances.
fn trim(machine: MachineKind) -> f32 {
    let rms_db = match machine {
        KickDrum => -18.9,
        FourOpFm => -29.6,
        SubBass => -29.8,
        NoiseWash => -30.0,
        VarSampler => -32.3,
        AcidLead => -20.8,
        ChordStab => -29.2,
        CzPhaseDist => -19.0,
        PolivoksSub => -22.7,
        VastFunSynth => -21.2,
        ElectroPerc => -25.7,
        WestCoastLpg => -22.1,
        OtisTranswave => -33.8,
        ByteBusR2r => -29.5,
        OtisTwin => -28.0,
        Fs77FormantFm => -25.1,
        VastTriple => -25.0,
    };
    10f32.powf((-29.0 - rms_db) / 20.0)
}

fn lane(machine: MachineKind, mix: f32, pan: f32, delay: f32, fx: f32) -> leviathan_dsp::LaneChannel {
    channel((trim(machine) * mix).min(1.25), pan, delay, fx)
}

fn bend(shift: f32, feedback: f32, decay: f32) -> BenderParams {
    BenderParams { shift_norm: shift, feedback, decay, freeze: false }
}

fn tri(x: f32) -> f32 {
    let f = x.rem_euclid(1.0);
    1.0 - (2.0 * f - 1.0).abs()
}

fn wave(x: f32) -> f32 {
    0.5 + 0.5 * (x * core::f32::consts::TAU).sin()
}

fn vel(base: u8, spread: u8, seed: usize) -> u8 {
    (f32::from(base) + (hash(seed as u32) - 0.5) * f32::from(spread)).clamp(1.0, 127.0) as u8
}

fn deg(root: i32, scale: &[i32], degree: i32) -> u8 {
    let n = scale.len() as i32;
    (root + 12 * degree.div_euclid(n) + scale[degree.rem_euclid(n) as usize]) as u8
}

const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
const PHRYGIAN: [i32; 7] = [0, 1, 3, 5, 7, 8, 10];
const PENTA: [i32; 5] = [0, 3, 5, 7, 10];

struct Track {
    file: &'static str,
    title: &'static str,
    bpm: f64,
    entries: usize,
    build: fn(usize) -> (Pattern, Scene),
}

// ---------------------------------------------------------------------------
// 0. VOICE TOUR: every machine alone, default patch then a two-macro sweep.
// ---------------------------------------------------------------------------

fn tour(e: usize) -> (Pattern, Scene) {
    let machine = MachineKind::ALL[e / 2];
    let sweep = e % 2 == 1;
    let (a, b) = match machine {
        KickDrum => (0, 4),
        FourOpFm => (1, 5),
        SubBass => (1, 2),
        NoiseWash => (2, 3),
        VarSampler => (3, 6),
        AcidLead => (2, 3),
        ChordStab => (1, 2),
        CzPhaseDist => (2, 0),
        PolivoksSub => (3, 2),
        VastFunSynth => (0, 3),
        ElectroPerc => (1, 5),
        WestCoastLpg => (1, 3),
        OtisTranswave => (2, 1),
        ByteBusR2r => (2, 6),
        OtisTwin => (2, 5),
        Fs77FormantFm => (3, 1),
        VastTriple => (3, 1),
    };
    let drum = matches!(machine, KickDrum | ElectroPerc | NoiseWash);
    let mut p = empty();
    let mut k = 0;
    let rhythm = if drum { "x.x.x.xxx.x.xx.x" } else { "x.x.x..xx.x..x.." };
    steps(&mut p, 0, rhythm, |row, _| {
        let bar = row / 16;
        let degree = [0, 2, 4, 3, 5, 7, 4, 2][k % 8] + [0, 5, 10, 5][bar];
        k += 1;
        let base = if matches!(machine, SubBass) { 48 } else { 36 };
        let mut cell = note(deg(base, &PENTA, degree), vel(104, 24, row + e * 64), 0.6);
        if sweep {
            let t = row as f32 / 63.0;
            cell = cell.with(lock(&[(a, t), (b, tri(t * 2.0))]));
        }
        cell
    });
    for (row, auto) in p.automation.iter_mut().enumerate() {
        auto.byte_word = ((row * 37) ^ (row >> 2) * 11) as u8;
        auto.byte_override = true;
    }
    let mut patches = [LanePatch::for_machine(KickDrum); NUM_LANES];
    patches[0] = LanePatch::for_machine(machine);
    let mut scene = default_scene(patches);
    scene.channels[0] = lane(machine, 1.0, 0.0, 0.15, 0.12);
    (p, scene)
}

// ---------------------------------------------------------------------------
// 1. UNDERTOW: 132 bpm techno. Kick, Sub, Acid, Perc, Chord Stab, Noise Wash,
//    with a few acid steps switched to 4-Op FM by step lock.
// ---------------------------------------------------------------------------

fn undertow(e: usize) -> (Pattern, Scene) {
    let sec = e / 4;
    let last = e % 4 == 3;
    let roots = [45i32, 45, 41, 43];
    let drums = matches!(sec, 1..=5 | 7..=10) || e == 44 || e == 45;
    let hats = matches!(sec, 2..=5 | 7..=10);
    let claps = matches!(sec, 4 | 5 | 7..=10);
    let acid = matches!(sec, 2..=5 | 7..=10);
    let stabs = matches!(sec, 4 | 5 | 7..=10);
    let wash = matches!(sec, 0 | 6 | 11);
    let peak = matches!(sec, 7 | 8 | 10);
    let build = ((e as f32 - 8.0) / 8.0).clamp(0.0, 1.0);
    const RIFF_A: [i32; 16] = [0, -1, 12, 0, -1, 3, 0, -1, 0, 10, -1, 0, 7, -1, 3, 12];
    const RIFF_B: [i32; 16] = [0, 0, -1, 7, 0, -1, 10, 12, -1, 0, 3, -1, 0, 15, 12, -1];
    let riff = if matches!(sec, 9 | 10) { RIFF_B } else { RIFF_A };

    let mut p = empty();
    for row in 0..64 {
        let (bar, s) = (row / 16, row % 16);
        let root = roots[bar];
        let seed = e * 64 + row;
        if drums && s % 4 == 0 {
            let mut cell = note(36, if s == 0 { 124 } else { 112 }, 0.5);
            if sec >= 9 {
                cell = cell.with(lock(&[(4, 0.85), (0, 0.5)]));
            }
            put(&mut p, 0, row, cell);
        }
        if drums && last && bar == 3 && s == 14 {
            put(&mut p, 0, row, note(36, 100, 0.5).ratchet(2));
        }
        if drums && sec != 11 && (matches!(s, 2 | 6 | 10 | 14) || (bar == 3 && s == 15)) {
            let lift = if matches!(sec, 9 | 10) && s == 10 { 12 } else { 0 };
            put(&mut p, 1, row, note((root + lift) as u8, vel(112, 10, seed), 0.7).with(lock(&[(0, 0.28)])));
        }
        if acid && riff[s] >= 0 {
            let accent = matches!(s, 2 | 9 | 15);
            let cut = if sec < 4 {
                0.16 + 0.45 * build
            } else if peak {
                0.45 + 0.35 * tri(row as f32 / 32.0 + e as f32 * 0.25)
            } else {
                0.5
            } + if accent { 0.14 } else { 0.0 }
                + 0.06 * wave(row as f32 / 12.0);
            let mut cell = note((root - 12 + riff[s]) as u8, if accent { 124 } else { 92 }, if accent { 0.9 } else { 0.45 })
                .with(lock(&[(2, cut.min(1.0)), (0, if accent { 0.45 } else { 0.14 })]));
            if peak && e % 2 == 1 && matches!(s, 5 | 12) {
                cell = note((root + 12 + riff[s]) as u8, 118, 0.5)
                    .with(lock(&[(1, 0.8), (0, 0.22), (5, 0.2)]))
                    .machine(FourOpFm);
            }
            if peak && last && bar == 3 && s >= 12 {
                cell = cell.ratchet(2);
            }
            put(&mut p, 2, row, cell);
        }
        if sec == 6 && s == 0 && bar % 2 == 0 {
            let t = (e - 24) as f32 / 4.0 + bar as f32 / 16.0;
            put(&mut p, 2, row, note((root - 12) as u8, 110, 0.95).with(lock(&[(2, 0.2 + 0.6 * t), (0, 0.95), (3, 0.85)])));
        }
        if hats {
            if claps && matches!(s, 4 | 12) {
                put(&mut p, 3, row, note(60, 116, 0.5).with(lock(&[(1, CLAP), (0, 0.3)])));
            } else if s % 4 == 2 {
                put(&mut p, 3, row, note(60, 104, 0.5).with(lock(&[(1, HAT), (0, 0.3)])));
            } else if sec >= 3 {
                put(&mut p, 3, row, note(60, vel(66, 30, seed), 0.3).with(lock(&[(1, HAT), (0, 0.07)])));
            }
            if matches!(sec, 9 | 10) && matches!(s, 7 | 13) {
                put(&mut p, 3, row, note(60, 84, 0.3).with(lock(&[(1, RIM), (0, 0.15)])).chance(60));
            }
        }
        if stabs && matches!(s, 3 | 10) {
            put(&mut p, 4, row, note((root + 12) as u8, vel(104, 16, seed), 0.5)
                .with(lock(&[(1, 0.3 + 0.5 * hash(seed as u32)), (2, 0.45 + 0.3 * wave(e as f32 / 8.0))])));
        }
        if wash && s == 0 {
            put(&mut p, 4, row, note((root + 12) as u8, 96, 0.95)
                .with(lock(&[(0, 0.95), (2, 0.3 + 0.12 * (e % 4) as f32), (1, 0.7)])));
            if bar % 2 == 0 {
                put(&mut p, 5, row, note(60, 90, 0.95).with(lock(&[(0, 1.0), (2, 0.25 + 0.15 * (e % 4) as f32)])));
            }
        }
        if !wash && last && bar == 3 && s % 2 == 0 {
            put(&mut p, 5, row, note(60, 50 + 9 * s as u8, 0.9).with(lock(&[(2, 0.3 + 0.04 * s as f32), (0, 0.2)])));
        } else if peak && s % 2 == 1 {
            put(&mut p, 5, row, note(60, vel(52, 20, seed), 0.3).with(lock(&[(0, 0.04), (2, 0.85)])).chance(85));
        }
    }
    let scene = Scene {
        patches: [
            patch(KickDrum, &[(1, 0.5), (0, 0.32)]),
            patch(SubBass, &[(1, if peak { 0.62 } else { 0.4 })]),
            patch(AcidLead, &[(3, if peak { 0.84 } else { 0.6 }), (1, if peak { 0.7 } else { 0.45 })]),
            patch(ElectroPerc, &[(3, 0.7)]),
            patch(ChordStab, &[(0, 0.3)]),
            patch(NoiseWash, &[]),
        ],
        channels: [
            lane(KickDrum, 1.6, 0.0, 0.0, 0.0),
            lane(SubBass, 1.2, 0.0, 0.0, 0.0),
            lane(AcidLead, 0.8, 0.08, 0.3, if sec == 6 { 0.6 } else { 0.15 }),
            lane(ElectroPerc, 0.7, 0.15, 0.25, 0.05),
            lane(ChordStab, 1.0, -0.25, 0.6, 0.45),
            lane(NoiseWash, 0.6, 0.3, 0.5, 0.5),
        ],
        bender: if sec == 6 { bend(0.53, 0.7, 0.85) } else { bend(0.5, 0.35, 0.45) },
        send: if sec == 6 { 0.2 } else { 0.05 },
    };
    (p, scene)
}

// ---------------------------------------------------------------------------
// 2. PHASE GARDEN: 118 bpm electro. Kick, Perc, CZ bass, Polivoks lead,
//    4-Op FM arpeggio, VAST pad.
// ---------------------------------------------------------------------------

fn phase_garden(e: usize) -> (Pattern, Scene) {
    let sec = e / 4;
    let roots = [38i32, 34, 36, 33];
    let tones = [[0, 3, 7], [0, 4, 7], [0, 4, 7], [0, 3, 7]];
    let pad = sec != 6;
    let arp = sec != 6;
    let bass = matches!(sec, 1..=4 | 6..=8 | 10);
    let drums = matches!(sec, 2..=4 | 6..=10);
    let lead = matches!(sec, 3 | 4 | 7 | 8 | 10);
    let metal = sec == 9;
    const MELODY: [&[(usize, i32)]; 4] = [
        &[(0, 7), (3, 9), (6, 7), (10, 4), (12, 5)],
        &[(0, 5), (4, 4), (8, 2), (11, 4)],
        &[(0, 4), (3, 6), (6, 8), (10, 7), (14, 6)],
        &[(0, 8), (4, 6), (8, 4), (12, 6)],
    ];
    let mut p = empty();
    let mut k = 0;
    for row in 0..64 {
        let (bar, s) = (row / 16, row % 16);
        let root = roots[bar];
        let seed = e * 64 + row;
        if drums && !metal && (matches!(s, 0 | 6 | 10) || (bar % 2 == 1 && s == 3)) {
            put(&mut p, 0, row, note(36, if s == 0 { 124 } else { 108 }, 0.5));
        }
        if drums {
            if matches!(s, 4 | 12) && !metal {
                put(&mut p, 1, row, note(60, 118, 0.5).with(lock(&[(1, SNARE), (0, 0.26), (3, 0.7)])));
            } else if metal {
                let t = row as f32 / 63.0;
                let mut cell = note(60, vel(96, 40, seed), 0.4)
                    .with(lock(&[(1, HAT), (0, 0.08 + 0.3 * hash(seed as u32 + 9)), (4, t), (5, tri(t * 2.0 + e as f32 * 0.25))]));
                if s % 8 == 5 {
                    cell = note(60, 100, 0.4).with(lock(&[(1, RIM), (0, 0.2), (2, 0.2 + 0.6 * hash(seed as u32))]));
                }
                if s == 15 {
                    cell = cell.ratchet(3);
                }
                if s % 2 == 0 || hash(seed as u32 + 3) > 0.4 {
                    put(&mut p, 1, row, cell);
                }
            } else if s == 14 {
                put(&mut p, 1, row, note(60, 100, 0.5).with(lock(&[(1, HAT), (0, 0.34)])));
            } else {
                put(&mut p, 1, row, note(60, vel(if s % 2 == 0 { 84 } else { 58 }, 20, seed), 0.3).with(lock(&[(1, HAT), (0, 0.06)])));
            }
        }
        if bass && matches!(s, 0 | 3 | 4 | 6 | 9 | 11 | 12 | 15) {
            let up = if matches!(s, 3 | 11 | 15) { 12 } else { 0 };
            put(&mut p, 2, row, note((root + up) as u8, vel(108, 14, seed), 0.55)
                .with(lock(&[(2, 0.3 + 0.6 * tri(row as f32 / 64.0 + e as f32 * 0.25)), (3, 0.25)])));
        }
        if lead {
            if let Some(&(_, degree)) = MELODY[bar].iter().find(|&&(step, _)| step == s) {
                let high = if matches!(sec, 8 | 10) && e % 2 == 1 { 7 } else { 0 };
                put(&mut p, 3, row, note(deg(50, &MINOR, degree + high), vel(108, 12, seed), 0.85)
                    .with(lock(&[(3, 0.34 + 0.3 * wave(row as f32 / 20.0 + e as f32 * 0.1))])));
            }
        }
        if arp && "x.xx.x.xx.xx.xx.".as_bytes()[s] == b'x' {
            let tone = tones[bar][k % 3] + 12 * ((k / 3) % 2) as i32;
            k += 1;
            if sec != 0 || e > 0 || bar > 0 {
                put(&mut p, 4, row, note((root + 24 + tone) as u8, vel(92, 30, seed), 0.5)
                    .with(lock(&[(1, 0.15 + 0.4 * wave(row as f32 / 23.0 + e as f32 * 0.13)), (5, 0.15 + 0.3 * hash(seed as u32))])));
            }
        }
        if pad && s == 0 {
            let fifth = if (e + bar) % 2 == 1 { 7 } else { 0 };
            put(&mut p, 5, row, note((root + 24 + fifth) as u8, 100, 0.97)
                .with(lock(&[(3, 0.2 + 0.6 * tri(e as f32 / 8.0 + bar as f32 / 16.0))])));
        }
    }
    let wave_a = [0.06, 0.06, 0.06, 0.06, 0.3, 0.06, 0.6, 0.3, 0.6, 0.06, 0.3, 0.06][sec];
    let algorithm = [0.12, 0.12, 0.12, 0.3, 0.3, 1.0, 0.12, 0.75, 0.75, 1.0, 0.3, 0.12][sec];
    let bell = matches!(sec, 5 | 9 | 11);
    let wide = matches!(sec, 5 | 11);
    let scene = Scene {
        patches: [
            patch(KickDrum, &[(0, 0.28), (1, 0.45), (4, 0.7)]),
            patch(ElectroPerc, &[]),
            patch(CzPhaseDist, &[(0, wave_a), (5, 0.15)]),
            patch(PolivoksSub, &[(1, 0.5), (7, 0.5), (4, 0.58), (2, if matches!(sec, 7 | 8) { 0.45 } else { 0.21 })]),
            patch(FourOpFm, &[(4, if bell { 1.0 } else { 0.45 }), (3, if bell { 0.0 } else { 0.28 }), (0, if bell { 0.6 } else { 0.3 })]),
            patch(VastFunSynth, &[(0, algorithm), (7, 1.0), (4, 0.2)]),
        ],
        channels: [
            lane(KickDrum, 1.5, 0.0, 0.0, 0.0),
            lane(ElectroPerc, 0.75, 0.1, 0.2, if metal { 0.35 } else { 0.05 }),
            lane(CzPhaseDist, 1.0, 0.0, 0.1, 0.0),
            lane(PolivoksSub, 0.8, 0.15, 0.5, 0.25),
            lane(FourOpFm, 0.75, -0.35, 0.6, if wide { 0.6 } else { 0.3 }),
            lane(VastFunSynth, 0.65, 0.2, 0.4, 0.5),
        ],
        bender: if wide { bend(0.51, 0.65, 0.8) } else { bend(0.5, 0.35, 0.5) },
        send: if wide { 0.15 } else { 0.04 },
    };
    (p, scene)
}

// ---------------------------------------------------------------------------
// 3. COLD TABLES: 92 bpm downtempo. West Coast LPG, OTIS, OTIS Twin, FS77,
//    K26, and one drum lane that swaps Kick and Perc by step lock.
// ---------------------------------------------------------------------------

fn cold_tables(e: usize) -> (Pattern, Scene) {
    let sec = e / 4;
    let roots = [36i32, 32, 41, 43];
    let tones = [[0, 3, 7], [0, 4, 7], [0, 3, 7], [0, 4, 7]];
    let drums = matches!(sec, 1..=4 | 6 | 7);
    let bass = matches!(sec, 1..=7);
    let voice = matches!(sec, 2..=7);
    let otis = matches!(sec, 3..=8);
    let pluck = sec != 5;
    const PHRASE: [&[(usize, usize, i32)]; 4] = [
        &[(0, 2, 12), (6, 1, 0), (10, 0, 12)],
        &[(0, 1, 12), (4, 2, 0), (8, 0, 12), (12, 1, 12)],
        &[(0, 0, 12), (6, 2, 12), (12, 1, 12)],
        &[(0, 2, 0), (4, 1, 12), (10, 0, 12), (14, 1, 12)],
    ];
    let mut p = empty();
    let mut k = 0;
    let mut vowel = e;
    for row in 0..64 {
        let (bar, s) = (row / 16, row % 16);
        let root = roots[bar];
        let seed = e * 64 + row;
        let t = row as f32 / 63.0;
        if pluck && (matches!(s, 0 | 3 | 6 | 8 | 11 | 14) || (sec >= 3 && matches!(s, 2 | 10))) {
            let tone = tones[bar][k % 3] + 12 * (k % 2) as i32;
            k += 1;
            if sec > 0 || k % 2 == 0 {
                put(&mut p, 0, row, note((root + 24 + tone) as u8, vel(100, 30, seed), 0.4)
                    .with(lock(&[(1, 0.15 + 0.65 * wave(t + e as f32 * 0.2)), (7, 0.4 + 0.5 * hash(seed as u32))])));
            }
        }
        if otis && matches!(s, 0 | 8) || otis && sec >= 6 && s == 12 {
            let tone = tones[bar][(s / 4 + e) % 3];
            put(&mut p, 1, row, note((root + 36 + tone) as u8, 104, 0.95)
                .with(lock(&[(2, tri(t + e as f32 * 0.25)), (3, 0.5 + 0.4 * wave(e as f32 / 6.0))])));
        }
        if s == 0 {
            put(&mut p, 2, row, note((root + 24) as u8, 100, 0.97)
                .with(lock(&[(4, 0.4 + 0.45 * tri(e as f32 / 6.0 + bar as f32 / 24.0)), (2, tri(e as f32 / 4.0 + bar as f32 / 8.0))])));
        }
        if voice {
            if let Some(&(_, tone, octave)) = PHRASE[bar].iter().find(|&&(step, _, _)| step == s) {
                vowel += 1;
                put(&mut p, 3, row, note((root + 24 + tones[bar][tone] + octave) as u8, vel(104, 14, seed), 0.9)
                    .with(lock(&[(3, [0.1, 0.5, 0.3, 0.9, 0.7][vowel % 5]), (1, 0.25 + 0.3 * wave(t * 2.0))])));
            }
        }
        if bass && matches!(s, 0 | 7 | 10) {
            put(&mut p, 4, row, note((root + if s == 7 { 12 } else { 0 }) as u8, 112, 0.8)
                .with(lock(&[(4, [0.0, 0.2, 0.4, 0.6][bar]), (5, 0.3 + 0.4 * wave(e as f32 / 5.0))])));
        }
        if drums {
            if matches!(s, 0 | 10) || (bar == 3 && s == 13) {
                put(&mut p, 5, row, note(36, if s == 0 { 122 } else { 104 }, 0.5));
            } else if s == 8 {
                put(&mut p, 5, row, note(60, 116, 0.5).with(lock(&[(1, SNARE), (0, 0.4), (3, 0.6)])).machine(ElectroPerc));
            } else if s == 13 {
                put(&mut p, 5, row, note(60, 80, 0.4).with(lock(&[(1, RIM), (0, 0.18)])).machine(ElectroPerc).chance(50));
            } else if s % 2 == 0 {
                put(&mut p, 5, row, note(60, vel(70, 30, seed), 0.3).with(lock(&[(1, HAT), (0, 0.09)])).machine(ElectroPerc).chance(92));
            }
        }
    }
    let table = [0.0, 0.0, 0.34, 0.34, 0.67, 1.0, 0.34, 0.0, 0.0][sec];
    let scene = Scene {
        patches: [
            patch(WestCoastLpg, &[
                (6, [0.5, 0.5, 0.0, 0.5, 1.0, 0.5, 0.0, 0.5, 0.5][sec]),
                (3, [0.1, 0.2, 0.25, 0.3, 0.45, 0.25, 0.5, 0.35, 0.15][sec]),
                (5, if sec == 7 { 0.6 } else { 0.0 }),
                (0, 0.5),
            ]),
            patch(OtisTranswave, &[(1, table), (0, 0.85)]),
            patch(OtisTwin, &[(1, table), (6, 0.5), (0, 1.0)]),
            patch(Fs77FormantFm, &[(0, 0.62), (7, [0.32, 0.32, 0.32, 0.1, 0.32, 0.8, 0.1, 0.32, 0.32][sec])]),
            patch(VastTriple, &[
                (3, [0.3, 0.3, 0.0, 0.3, 0.6, 0.9, 0.3, 0.0, 0.3][sec]),
                (1, [0.25, 0.25, 0.0, 0.6, 0.25, 0.9, 0.25, 0.6, 0.25][sec]),
                (7, if sec == 5 { 0.8 } else { 0.36 }),
            ]),
            patch(KickDrum, &[(0, 0.42), (1, 0.3), (4, 0.4)]),
        ],
        channels: [
            lane(WestCoastLpg, 0.8, -0.3, 0.5, 0.3),
            lane(OtisTranswave, 1.2, 0.3, 0.5, 0.5),
            lane(OtisTwin, 0.8, -0.1, 0.4, 0.5),
            lane(Fs77FormantFm, 0.9, 0.1, 0.45, 0.35),
            lane(VastTriple, 1.0, 0.0, 0.1, 0.05),
            lane(KickDrum, 1.4, 0.0, 0.05, 0.03),
        ],
        bender: if sec == 5 { bend(0.49, 0.7, 0.9) } else { bend(0.5, 0.4, 0.6) },
        send: if sec == 5 { 0.2 } else { 0.08 },
    };
    (p, scene)
}

// ---------------------------------------------------------------------------
// 4. BYTE BUS: 140 bpm industrial. Kick, Perc, ByteBus, VarSampler,
//    K26 trading steps with West Coast, Acid trading steps with Noise Wash.
// ---------------------------------------------------------------------------

fn byte_bus(e: usize) -> (Pattern, Scene) {
    let sec = e / 4;
    let last = e % 4 == 3;
    let kick = matches!(sec, 1..=5 | 7..=11);
    let broken = sec == 10;
    let perc = matches!(sec, 2..=5 | 8..=11);
    let sampler = matches!(sec, 2..=6 | 8..=12);
    let riff = matches!(sec, 3..=5 | 8..=11);
    let acid = matches!(sec, 5 | 7..=9 | 11);
    let glitch = sec == 6;
    let mut p = empty();
    for (row, auto) in p.automation.iter_mut().enumerate() {
        auto.byte_override = true;
        auto.byte_word = if matches!(sec, 0 | 1 | 12) {
            [0xa4, 0x5a, 0xc3, 0x3c][row / 16]
        } else if glitch {
            (hash((e * 64 + row) as u32) * 255.0) as u8
        } else {
            ((row * 29) ^ ((row >> 1) * 7) ^ (e * 13)) as u8
        };
    }
    for row in 0..64 {
        let (bar, s) = (row / 16, row % 16);
        let seed = e * 64 + row;
        let t = row as f32 / 63.0;
        let on = if broken { matches!(s, 0 | 3 | 6 | 8 | 11 | 14) } else { s % 4 == 0 || (last && bar == 3 && s >= 12) };
        if kick && on {
            put(&mut p, 0, row, note(36, if s % 4 == 0 { 124 } else { 104 }, 0.5)
                .with(lock(&[(0, if s % 4 == 0 { 0.3 } else { 0.15 })])));
        }
        if perc {
            if matches!(s, 4 | 12) {
                put(&mut p, 1, row, note(60, 120, 0.5).with(lock(&[(1, CLAP), (0, 0.3), (6, 0.7)])));
            } else {
                let mut cell = note(60, vel(if s % 4 == 2 { 100 } else { 64 }, 24, seed), 0.3)
                    .with(lock(&[(1, HAT), (0, if s % 4 == 2 { 0.22 } else { 0.05 }), (4, 0.3 + 0.6 * wave(t + e as f32 * 0.3))]));
                if hash(seed as u32 + 77) > 0.88 {
                    cell = cell.ratchet(2 + (seed % 2) as u8);
                }
                put(&mut p, 1, row, cell);
            }
        }
        if "x.xxx.x.xx.xx.xx".as_bytes()[s] == b'x' || glitch {
            let octave = if matches!(s, 3 | 9 | 15) { 12 } else { 0 };
            let mut cell = note((28 + octave + if bar == 3 && s >= 8 { 1 } else { 0 }) as u8, vel(108, 20, seed), 0.6)
                .with(lock(&[(1, hash((e * 4 + bar) as u32)), (2, 0.1 + 0.8 * tri(t + e as f32 * 0.25)), (6, 0.2 + 0.6 * hash(seed as u32 + 5))]));
            if glitch {
                cell = cell.ratchet(1 + (hash(seed as u32 + 1) * 3.9) as u8).chance(70);
            }
            put(&mut p, 2, row, cell);
        }
        if sampler && (matches!(s, 2 | 5 | 8 | 11 | 13) || glitch && s % 2 == 1) {
            let bank = (hash(seed as u32 + 31) * 4.99).floor() / 4.0;
            put(&mut p, 3, row, note(deg(52, &PHRYGIAN, [0, 1, 4, 0, 3, 6, 4, 1][(seed / 3) % 8]), vel(104, 24, seed), 0.6)
                .with(lock(&[(3, bank), (1, 0.2 + 0.4 * hash(seed as u32 + 2)), (6, 0.1 + 0.6 * t), (0, 0.22)])));
        }
        if riff {
            if let Some(i) = [0usize, 3, 6, 9, 12, 14].iter().position(|&step| step == s) {
                let degree = [0, 1, 0, 3, 0, 6][i] + if bar == 2 { 2 } else { 0 };
                let midi = deg(40, &PHRYGIAN, degree);
                let cell = if i % 2 == 1 {
                    note(midi + 12, 112, 0.5).with(lock(&[(1, 0.55 + 0.4 * wave(t)), (3, 0.5), (0, 0.3)])).machine(WestCoastLpg)
                } else {
                    note(midi, 116, 0.7).with(lock(&[(3, [0.0, 0.3, 0.6, 0.9][(bar + e) % 4]), (4, 0.2 * (i as f32))]))
                };
                put(&mut p, 4, row, cell);
            }
        }
        if acid {
            if matches!(s, 4 | 12) {
                put(&mut p, 5, row, note(60, 110, 0.5).with(lock(&[(0, 0.18), (2, 0.7), (1, 0.8)])).machine(NoiseWash));
            } else if s % 2 == 0 || hash(seed as u32 + 11) > 0.6 {
                let degree = [0, 0, 7, 1, 0, 3, 8, 0][(row / 2 + e) % 8];
                put(&mut p, 5, row, note(deg(40, &PHRYGIAN, degree), vel(104, 20, seed), 0.5)
                    .with(lock(&[(2, 0.35 + 0.55 * tri(t * 2.0 + e as f32 * 0.25)), (0, 0.12 + 0.2 * hash(seed as u32))])));
            }
        } else if !matches!(sec, 0 | 12) && last && bar >= 2 && s % 2 == 0 {
            let lift = ((bar - 2) * 16 + s) as f32 / 32.0;
            put(&mut p, 5, row, note(60, 60 + (60.0 * lift) as u8, 0.9).with(lock(&[(2, 0.2 + 0.7 * lift), (0, 0.15), (3, 0.7)])).machine(NoiseWash));
        }
    }
    let wet = matches!(sec, 0 | 6 | 12);
    let scene = Scene {
        patches: [
            patch(KickDrum, &[(1, 0.85), (4, 0.65)]),
            patch(ElectroPerc, &[(6, 0.6)]),
            patch(ByteBusR2r, &[(0, 0.3), (7, [0.35, 0.35, 0.35, 0.6, 0.35, 0.8, 0.35, 0.6, 0.8, 0.35, 0.6, 0.8, 0.35][sec])]),
            patch(VarSampler, &[(7, [0.55, 0.55, 0.55, 0.2, 0.55, 0.9, 0.2, 0.55, 0.9, 0.2, 0.55, 0.9, 0.55][sec]), (5, 0.6)]),
            patch(VastTriple, &[(1, 0.0), (0, 0.3), (2, 0.4)]),
            patch(AcidLead, &[(3, 0.86), (1, 0.75)]),
        ],
        channels: [
            lane(KickDrum, 1.6, 0.0, 0.0, 0.03),
            lane(ElectroPerc, 0.75, -0.12, 0.2, 0.08),
            lane(ByteBusR2r, 0.6, 0.0, 0.2, if wet { 0.6 } else { 0.1 }),
            lane(VarSampler, 1.1, 0.35, 0.5, if wet { 0.6 } else { 0.25 }),
            lane(VastTriple, 0.9, -0.3, 0.3, 0.15),
            lane(AcidLead, 0.65, 0.2, 0.4, 0.2),
        ],
        bender: if glitch {
            BenderParams { shift_norm: 0.56, feedback: 0.8, decay: 0.9, freeze: e % 2 == 1 }
        } else if wet {
            bend(0.47, 0.6, 0.75)
        } else {
            bend(0.5, 0.35, 0.4)
        },
        send: if wet { 0.18 } else { 0.03 },
    };
    (p, scene)
}

const TRACKS: [Track; 5] = [
    Track { file: "00-voice-tour", title: "Voice tour", bpm: 120.0, entries: 34, build: tour },
    Track { file: "01-undertow", title: "Undertow", bpm: 132.0, entries: 48, build: undertow },
    Track { file: "02-phase-garden", title: "Phase Garden", bpm: 118.0, entries: 48, build: phase_garden },
    Track { file: "03-cold-tables", title: "Cold Tables", bpm: 92.0, entries: 36, build: cold_tables },
    Track { file: "04-byte-bus", title: "Byte Bus", bpm: 140.0, entries: 52, build: byte_bus },
];

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "voice-demos".into()));
    let only = args.next();
    std::fs::create_dir_all(&out)?;
    for track in &TRACKS {
        if only.as_deref().is_some_and(|name| !track.file.contains(name)) {
            continue;
        }
        let mut song = Song::new(track.bpm);
        for e in 0..track.entries {
            let (pattern, scene) = (track.build)(e);
            song.play(pattern, scene, 1);
        }
        let mut audio = render(&song, true);
        let top = peak(&audio.master);
        let hot = audio.master.iter().filter(|f| f[0].abs() >= 0.999 || f[1].abs() >= 0.999).count();
        let gain = 0.891 / top.max(1e-6);
        for frame in &mut audio.master {
            frame[0] *= gain;
            frame[1] *= gain;
        }
        let seconds: Vec<f32> = audio.master.chunks(RATE as usize).map(|c| db(rms(c))).collect();
        let quiet = seconds.iter().filter(|&&level| level < -60.0).count();
        println!(
            "{} ({}): {:.0} bpm, {}:{:02}, raw peak {:.1} dB, {} full-scale frames, gain {:+.1} dB, rms {:.1} dB, {} silent seconds",
            track.file, track.title, track.bpm,
            seconds.len() / 60, seconds.len() % 60,
            db(top), hot, db(gain), db(rms(&audio.master)), quiet
        );
        print!("  lane rms dB:");
        for lane in 0..NUM_LANES {
            let stem: Vec<[f32; 2]> = audio.stems.iter().map(|s| s[lane]).collect();
            print!(" {:.1}", db(rms(&stem)));
        }
        println!();
        print!("  per 8 s rms:");
        for chunk in seconds.chunks(8) {
            print!(" {:.0}", chunk.iter().sum::<f32>() / chunk.len() as f32);
        }
        println!();
        write_wav(&out.join(format!("{}.wav", track.file)), &audio.master)?;
    }
    Ok(())
}
