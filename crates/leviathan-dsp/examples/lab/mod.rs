//! Headless song builder and renderer shared by the voice-lab examples.
#![allow(dead_code)]

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use leviathan_core::{
    EngineConfig, LaunchTarget, NoteCell, Pattern, PatternChain, SongPlayer, SoundLock,
    MAX_CHAIN_ENTRIES, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::otis_kit::{self, Sample};
use leviathan_dsp::{
    echo, BenderParams, CaptureSamples, DelayMode, LaneChannel, LanePatch, MachineKind,
    StereoDelay, SynthRack,
};

pub const RATE: u32 = 48_000;

/// A lane patch from a machine and `(slot, value)` overrides of its defaults.
pub fn patch(machine: MachineKind, set: &[(usize, f32)]) -> LanePatch {
    let mut patch = LanePatch::for_machine(machine);
    for &(slot, value) in set {
        patch.params[slot] = value;
    }
    patch
}

/// A step lock of `(slot, value)` pairs.
pub fn lock(set: &[(usize, f32)]) -> SoundLock {
    let mut lock = SoundLock::default();
    for &(slot, value) in set {
        lock.mask |= 1 << slot;
        lock.values[slot] = value;
    }
    lock
}

pub fn channel(level: f32, pan: f32, delay_send: f32, fx_send: f32) -> LaneChannel {
    LaneChannel { level, pan, delay_send, fx_send }
}

/// Everything the mixer and lane patches hold while one chain entry plays.
#[derive(Clone, Copy)]
pub struct Scene {
    pub patches: [LanePatch; NUM_LANES],
    pub channels: [LaneChannel; NUM_LANES],
    pub bender: BenderParams,
    pub send: f32,
}

pub struct Song {
    pub bpm: f64,
    pub patterns: Vec<Pattern>,
    /// One `(pattern index, scene)` per chain entry.
    pub chain: Vec<(usize, Scene)>,
    pub tail_seconds: f64,
    /// Eight one-shots for OtisKit lanes (the app's kit page).
    pub kit: Option<Kit>,
    pub echo: Option<echo::Params>,
    pub delay_mode: DelayMode,
    /// Note gates (opt-in, #25): gate-off releases envelopes, held full-gate steps retrigger.
    pub note_gates: bool,
}

/// Host-owned kit memory, as the app's `kit.rs` provides it.
pub struct Kit {
    pub pcm: [Vec<i16>; otis_kit::SLOTS],
    pub slots: [otis_kit::Slot; otis_kit::SLOTS],
    pub lanes: [otis_kit::Lane; NUM_LANES],
}

impl otis_kit::Memory for Kit {
    fn current(&self, slot: usize) -> Option<Sample<'_>> {
        let pcm = self.pcm.get(slot)?;
        (!pcm.is_empty()).then_some(Sample { generation: slot as u64 + 1, rate: RATE, pcm })
    }
    fn previous(&self, _lane: usize, slot: usize, generation: u64) -> Option<Sample<'_>> {
        self.current(slot).filter(|s| s.generation == generation)
    }
}

impl Song {
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm,
            patterns: Vec::new(),
            chain: Vec::new(),
            tail_seconds: 6.0,
            kit: None,
            echo: None,
            delay_mode: DelayMode::Slapback,
            note_gates: false,
        }
    }

    /// Adds a pattern, plays it `repeats` times under `scene`, and returns its index.
    pub fn play(&mut self, pattern: Pattern, scene: Scene, repeats: usize) -> usize {
        self.patterns.push(pattern);
        let index = self.patterns.len() - 1;
        for _ in 0..repeats {
            self.chain.push((index, scene));
        }
        index
    }

    pub fn beats(&self) -> f64 {
        self.chain.iter().map(|&(index, _)| self.patterns[index].duration_beats()).sum()
    }

    pub fn seconds(&self) -> f64 {
        self.beats() * 60.0 / self.bpm + self.tail_seconds
    }
}

pub struct Rendered {
    pub master: Vec<[f32; 2]>,
    pub stems: Vec<[[f32; 2]; NUM_LANES]>,
}

/// Renders through `SongPlayer` and `SynthRack` exactly as the app's WAV export does.
pub fn render(song: &Song, keep_stems: bool) -> Rendered {
    assert!(song.chain.len() <= MAX_CHAIN_ENTRIES, "chain has {} entries", song.chain.len());
    assert!(song.patterns.len() <= 128);
    let mut chain = PatternChain::default();
    for &(index, _) in &song.chain {
        chain.push(index as u8);
    }
    let mut player = SongPlayer::new(EngineConfig {
        bpm: song.bpm,
        sample_rate: f64::from(RATE),
        ..EngineConfig::default()
    });
    player.evaluator.set_retrigger_held_notes(song.note_gates);
    player.set_chain(chain);
    player.launch(LaunchTarget::Chain, song.patterns.len());
    player.evaluator.playing = true;

    let first = song.chain[0].1;
    let mut rack = SynthRack::new(RATE as f32);
    rack.set_lane_channels(first.channels);
    rack.set_lane_patches(first.patches);
    rack.set_note_release_lanes(if song.note_gates { 0xff } else { 0 });
    if let Some(echo) = song.echo {
        rack.set_echo_params(echo);
    }
    rack.set_tempo(song.bpm);
    if let Some(kit) = &song.kit {
        rack.set_kit_controls(kit.slots, kit.lanes);
    }
    rack.set_delay_mode(song.delay_mode);
    rack.reset_voices();
    rack.set_bender_params(first.bender);
    rack.reset_master(first.send);

    let music = (song.beats() * 60.0 / song.bpm * f64::from(RATE)).round() as usize;
    let frames = music + (song.tail_seconds * f64::from(RATE)) as usize;
    let mut out = Rendered { master: Vec::with_capacity(frames), stems: Vec::new() };
    let mut stems = [[0.0; 2]; NUM_LANES];
    let mut entry = Some(0);
    let mut wraps = 0;
    let samples = CaptureSamples {
        kit: song.kit.as_ref().map(|kit| kit as &dyn otis_kit::Memory),
        ..CaptureSamples::default()
    };
    let mut echo_storage = if song.echo.is_some_and(|e| e.echo_mix > 0.0) {
        vec![[0.0; 2]; echo::capacity(RATE as f32)]
    } else {
        Vec::new()
    };
    let mut delays: Box<[StereoDelay; NUM_LANES]> = Box::new(std::array::from_fn(|_| {
        let mut delay = StereoDelay::new(RATE as f32);
        delay.set_mode(song.delay_mode);
        delay
    }));
    for frame in 0..frames {
        if frame == music {
            player.evaluator.playing = false;
        }
        let (cv, retrigger) = player.step_sample(&song.patterns);
        let now = player.status().chain_entry;
        if now != entry && frame < music {
            if now == Some(0) {
                wraps += 1;
            }
            entry = now;
            if let (Some(index), 0) = (now, wraps) {
                let scene = song.chain[index].1;
                rack.set_lane_patches(scene.patches);
                rack.set_lane_channels(scene.channels);
                rack.set_bender_params(scene.bender);
                rack.set_master_send(scene.send);
            }
        }
        if retrigger {
            rack.retrigger_gates();
        }
        let ((left, right), _) = rack.render_session_with_echo_stems(
            &cv, &samples, &mut stems, &mut delays, &mut echo_storage,
        );
        out.master.push([left, right]);
        if keep_stems {
            out.stems.push(stems);
        }
    }
    out
}

pub fn write_wav(path: &Path, frames: &[[f32; 2]]) -> io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    let bytes = (frames.len() * 6) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * 6).to_le_bytes())?;
    file.write_all(&6u16.to_le_bytes())?;
    file.write_all(&24u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for frame in frames {
        for sample in frame {
            let value = (sample.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
            file.write_all(&value.to_le_bytes()[..3])?;
        }
    }
    file.flush()
}

// ---- pattern helpers -------------------------------------------------------

pub fn note(midi: u8, velocity: u8, gate: f32) -> NoteCell {
    NoteCell { active: true, midi, velocity, gate_fraction: gate, ..NoteCell::default() }
}

pub trait CellExt {
    fn with(self, lock: SoundLock) -> Self;
    fn machine(self, machine: MachineKind) -> Self;
    fn nudge(self, ticks: i8) -> Self;
    fn ratchet(self, count: u8) -> Self;
    fn chance(self, percent: u8) -> Self;
}

impl CellExt for NoteCell {
    fn with(mut self, lock: SoundLock) -> Self {
        let machine = self.lock.machine;
        self.lock = lock;
        if lock.machine < 0 {
            self.lock.machine = machine;
        }
        self
    }
    fn machine(mut self, machine: MachineKind) -> Self {
        self.lock.machine = machine as i16;
        self
    }
    fn nudge(mut self, ticks: i8) -> Self {
        self.micro_offset = ticks;
        self
    }
    fn ratchet(mut self, count: u8) -> Self {
        self.retrig = count;
        self
    }
    fn chance(mut self, percent: u8) -> Self {
        self.probability = percent;
        self
    }
}

pub fn empty() -> Pattern {
    Pattern::default()
}

pub fn put(pattern: &mut Pattern, lane: usize, row: usize, cell: NoteCell) {
    if row < ROWS_PER_PATTERN {
        pattern.lanes[lane][row] = cell;
    }
}

/// Places `cell(step)` on every row where the text has a non-dot character.
/// The text repeats to fill the pattern; spaces are ignored.
pub fn steps(pattern: &mut Pattern, lane: usize, text: &str, mut cell: impl FnMut(usize, char) -> NoteCell) {
    let marks: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    for row in 0..pattern.length {
        let mark = marks[row % marks.len()];
        if mark != '.' {
            pattern.lanes[lane][row] = cell(row, mark);
        }
    }
}

/// Deterministic 0..1 hash for humanizing.
pub fn hash(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

// ---- measurement -----------------------------------------------------------

pub fn peak(frames: &[[f32; 2]]) -> f32 {
    frames.iter().flat_map(|f| f.iter()).fold(0.0, |a, &b| a.max(b.abs()))
}

pub fn rms(frames: &[[f32; 2]]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    let sum: f64 = frames.iter().map(|f| f64::from(f[0] * f[0] + f[1] * f[1]) * 0.5).sum();
    (sum / frames.len() as f64).sqrt() as f32
}

pub fn db(value: f32) -> f32 {
    20.0 * value.max(1e-9).log10()
}

pub fn mono(frames: &[[f32; 2]]) -> Vec<f32> {
    frames.iter().map(|f| 0.5 * (f[0] + f[1])).collect()
}

/// Fundamental by normalized autocorrelation with parabolic refinement.
pub fn f0(signal: &[f32], low_hz: f32, high_hz: f32) -> Option<f32> {
    let rate = RATE as f32;
    let min_lag = (rate / high_hz) as usize;
    let max_lag = ((rate / low_hz) as usize).min(signal.len() / 2);
    if max_lag <= min_lag + 2 {
        return None;
    }
    let n = signal.len() - max_lag - 1;
    let energy: f32 = signal[..n].iter().map(|x| x * x).sum();
    if energy < 1e-7 {
        return None;
    }
    let corr = |lag: usize| -> f32 {
        let mut a = 0.0;
        let mut e = 0.0;
        for i in 0..n {
            a += signal[i] * signal[i + lag];
            e += signal[i + lag] * signal[i + lag];
        }
        a / (energy * e).sqrt().max(1e-9)
    };
    let values: Vec<f32> = (min_lag..=max_lag).map(corr).collect();
    let best = values.iter().cloned().fold(f32::MIN, f32::max);
    if best < 0.5 {
        return None;
    }
    // First peak within 90 % of the best avoids octave-down errors.
    let mut pick = None;
    for i in 1..values.len() - 1 {
        if values[i] >= 0.9 * best && values[i] >= values[i - 1] && values[i] >= values[i + 1] {
            pick = Some(i);
            break;
        }
    }
    let i = pick?;
    let (a, b, c) = (values[i - 1], values[i], values[i + 1]);
    let shift = if (a - 2.0 * b + c).abs() > 1e-9 { 0.5 * (a - c) / (a - 2.0 * b + c) } else { 0.0 };
    Some(rate / ((min_lag + i) as f32 + shift))
}

/// Spectral centroid proxy: mean absolute first difference over mean absolute value, as Hz.
pub fn brightness(signal: &[f32]) -> f32 {
    let level: f32 = signal.iter().map(|x| x.abs()).sum();
    if level < 1e-6 {
        return 0.0;
    }
    let diff: f32 = signal.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    diff / level * RATE as f32 / (2.0 * core::f32::consts::PI)
}

pub fn midi_hz(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

pub fn default_scene(patches: [LanePatch; NUM_LANES]) -> Scene {
    Scene {
        patches,
        channels: [channel(0.8, 0.0, 0.0, 0.0); NUM_LANES],
        bender: BenderParams::default(),
        send: 0.0,
    }
}

// ---- synthetic one-shot kit ------------------------------------------------

/// Eight synthesized one-shots (no recordings): kick, snare, closed hat, open
/// hat, clap, rim, low tom, metal. Slot n plays from MIDI 60 + n.
pub fn synth_kit(clock: u8) -> Kit {
    let rate = RATE as f32;
    let mut noise = 0x1234_5678u32;
    let white = move || {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        noise as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let make = |seconds: f32, f: &mut dyn FnMut(f32, f32) -> f32| -> Vec<i16> {
        let n = (seconds * rate) as usize;
        let mut hp = 0.0f32;
        let mut prev = 0.0f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                let x = f(t, 0.0);
                // gentle DC blocker
                hp = 0.995 * (hp + x - prev);
                prev = x;
                (hp.clamp(-1.0, 1.0) * 30_000.0) as i16
            })
            .collect()
    };
    use core::f32::consts::TAU;
    let mut phase = 0.0f32;
    let kick = make(0.55, &mut |t, _| {
        let f = 48.0 + 160.0 * (-t * 38.0).exp();
        phase += TAU * f / rate;
        (phase.sin() * (-t * 6.5).exp() * 1.4).tanh()
    });
    let mut w = white.clone();
    let mut lp = 0.0f32;
    let snare = make(0.32, &mut |t, _| {
        let n = w();
        lp += 0.45 * (n - lp);
        0.55 * (TAU * 186.0 * t).sin() * (-t * 28.0).exp() + 0.8 * (n - 0.6 * lp) * (-t * 13.0).exp()
    });
    let mut w2 = white.clone();
    let mut hpz = (0.0f32, 0.0f32);
    let mut hat = |len: f32, decay: f32, w: &mut dyn FnMut() -> f32| {
        make(len, &mut |t, _| {
            let n = w();
            let y = n - hpz.0;
            hpz.0 = n;
            hpz.1 += 0.6 * (y - hpz.1);
            let metal: f32 = [317.0, 421.0, 543.0, 687.0, 811.0, 1043.0]
                .iter()
                .map(|f| ((TAU * f * 6.0 * t).sin()).signum())
                .sum::<f32>()
                / 6.0;
            (0.6 * hpz.1 + 0.35 * metal) * (-t * decay).exp()
        })
    };
    let closed = hat(0.09, 55.0, &mut w2);
    let mut w3 = white.clone();
    let open = hat(0.6, 7.0, &mut w3);
    let mut w4 = white.clone();
    let mut bp = (0.0f32, 0.0f32);
    let clap = make(0.4, &mut |t, _| {
        let n = w4();
        // two-pole bandpass near 1.2 kHz
        let f = 2.0 * (core::f32::consts::PI * 1200.0 / rate).sin();
        bp.0 += f * bp.1;
        let high = n - bp.0 - 0.5 * bp.1;
        bp.1 += f * high;
        let env = if t < 0.03 {
            (-((t * 1000.0) % 10.0) * 0.35).exp()
        } else {
            (-(t - 0.03) * 16.0).exp()
        };
        1.8 * bp.1 * env
    });
    let rim = make(0.08, &mut |t, _| {
        ((TAU * 1700.0 * t).sin() * 0.7 + (TAU * 480.0 * t).sin() * 0.5) * (-t * 70.0).exp()
    });
    let mut ph2 = 0.0f32;
    let tom = make(0.6, &mut |t, _| {
        let f = 92.0 + 60.0 * (-t * 18.0).exp();
        ph2 += TAU * f / rate;
        ph2.sin() * (-t * 7.0).exp()
    });
    let metal = make(1.2, &mut |t, _| {
        [(540.0, 1.0), (812.0, 0.7), (1263.0, 0.5), (1987.0, 0.35)]
            .iter()
            .map(|&(f, a)| a * (TAU * f * t).sin())
            .sum::<f32>()
            * 0.45
            * (-t * 3.2).exp()
    });
    Kit {
        pcm: [kick, snare, closed, open, clap, rim, tom, metal],
        slots: [otis_kit::Slot::default(); otis_kit::SLOTS],
        lanes: [otis_kit::Lane { bytebus: false, clock, drop_samples: false }; NUM_LANES],
    }
}
