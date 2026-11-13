# Leviathan

A native, single-binary audio/visual tracker workstation written in clean-room Rust (`BSD-3-Clause`).

Leviathan consolidates deterministic Score v3 sequencing, sample-accurate modular CV/ByteBus generation, internal zero-allocation synthesis, GPU ping-pong shader feedback (`wgpu`), and an immediate-mode tracker UI (`egui`) into a single reproducible Cargo workspace—eliminating cross-process interpreter, browser, and legacy OpenGL seams.

## Workspace Architecture

Leviathan uses a strict hexagonal ("ports and adapters") layout so external GPU, audio, or GUI crate updates never touch core timing, score evaluation, or DSP math:

| Crate | `#![no_std]` | External Deps | Responsibility |
| :--- | :--- | :--- | :--- |
| `crates/leviathan-core` | Yes | Optional `serde` only | Score v3 data model (6 note lanes, `FILL` conditions, 16-pattern banks, mixed-rate `PatternChain`, per-step `SoundLock`s, visual automation, undo stack), `lowbias32` probability hashing, `1V/oct` CV & gate evaluation, 9-channel `0V`/`10V` ByteBus encoding, and binary `#[repr(C)]` `SoundLinkPacket` ABI (`336` bytes). |
| `crates/leviathan-dsp` | Yes | `leviathan-core` only | Zero-allocation 7-machine instrument rack (`KickDrum`, `FourOpFm`, `SubBass`, `NoiseWash`, `VarSampler` 4-bank `UQ16.16` variable-clock sampler, `AcidLead`, `ChordStab`), TPT state-variable filters, click-free per-lane mute/solo gain slew, and stereo `MasterBus` (ping-pong delay, DC blocker, linked soft-knee limiter, peak meters). |
| `crates/leviathan-gpu` | No | `bytemuck` | Uniform buffer layouts (`VisualUniforms`), built-in WGSL ping-pong feedback shader (`FEEDBACK_SHADER_WGSL`), custom `.wgsl` starter template & ABI contract validator, and software reference `FeedbackSurface`. |
| `crates/leviathan-app` | No | `cpal`, `rtrb`, `eframe`, `midir` | Native desktop workstation hosting the lock-free `cpal` real-time audio engine, `rtrb` command/telemetry rings, `.leviathan.json` project persistence, offline WAV & stem exporter, MIDI input, non-blocking `SoundLinkPacket` UDP loopback sender, custom `.wgsl` live hot-reload viewport, and `egui` 64-row tracker editor. |

## Tracker Keyboard Shortcuts

| Action | Shortcut |
| :--- | :--- |
| Play / Stop transport | `Space` |
| Save / Save As / Open project | `Ctrl+S` / `Ctrl+Shift+S` / `Ctrl+O` (`Cmd` on macOS) |
| Navigate cursor | `Arrows`, `PgUp`/`PgDn` (±16 rows), `Home`/`End`, `Tab` / `Shift+Tab` |
| 2+ octave piano note entry | `Z S X D C V G B H N J M` (lower) & `Q 2 W 3 E R 5 T 6 Y 7 U I 9 0 O P` (upper) |
| Base octave / edit step | `F1`/`F2` or `-`/`=` (octave), `F3`/`F4` or `[`/`]` (step `0..=16`) |
| Hex velocity (`00`..`7F`) & `AIR` | `0`..`9` / `A`..`F` on `VEL` column; `` ` `` inserts `AIR` trigger |
| Rectangular block selection | `Shift + Arrows`, `Shift + PgUp/PgDn/Home/End`, `Shift + Click`, `Ctrl+A` (lane -> pattern -> view), `Esc` |
| Undo / Redo (64 steps) | `Ctrl+Z` / `Ctrl+Y` or `Ctrl+Shift+Z` (`Cmd` on macOS) |
| Copy / Cut / Paste block | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Clear / Wipe locks | `Delete` (clear notes) / `Shift+Delete` (clear notes + `SoundLock`s) |
| Semitone / Octave transpose | `Ctrl + Up/Down` (±1 semitone) / `Ctrl + Shift + Up/Down` (±12 semitones) |
| Per-lane Mute / Solo / Clear all | `Alt + 1..6` (mute), `Alt + Shift + 1..6` (solo), `Alt + Shift + A` (unmute/unsolo all) |

## Projects, Pattern Chains & Offline WAV Export

- **Save** (`Ctrl/Cmd+S`) stores the pattern bank, chain, tempo, mixer, sound locks, visual automation, custom WGSL shader, and sampler banks with their trim and loop settings in a single `.leviathan.json` file. **Save As** (`Ctrl/Cmd+Shift+S`) chooses a new path, and **Open** (`Ctrl/Cmd+O`) restores the session with playback paused.
- **New Pattern** and **Clone** build a bank of up to 16 patterns with independent playback rate multipliers (`1/4x`..`4x`). **Add Pattern** appends patterns to a 64-entry song chain, and **Cue Chain** launches the chain on the next bar.
- **Swing** delays odd rows from 50% to 75% of each two-row pair while preserving pattern length. In the step inspector, **Nudge** moves a hit by up to 24 ticks either way (48 ticks per row), and **Retrig** divides the step into up to four evenly spaced hits. Nudges stay within the pattern, with later hits taking over overlapping notes on the same lane.
- Select **PERC**, **WEST**, or **OTIS** in **Lane Patch** to play the electro percussion, West Coast LPG, or transwave scanner voice. Each also supports per-step machine and parameter locks.
- Select **OT2** in **Lane Patch** or **Step Lock** for counter-scanning transwaves. **TwinSpread** starts with one reader and blends in detuned unison, a fifth, or an octave as you turn it up. **FilterF2** adds resonance below center and raises the high-pass floor above it.
- Select **FS77** for formant FM. Lock **FormantHz** on individual steps to sequence vowels. Turn up **WaveRcm** to feed an OTIS waveform into the FM, or **SkirtUnv** to widen the formants and add breath. **ResoDac** moves from a resonant low-pass with provisional CZ coloration to a vocal band-pass with YM3012 coloration.
- Select **K26** for three-stage VAST synthesis. **SourceKey** selects detuned saws, hard sync, drawbars, or OTIS wavetables. **PreTilt** filters the source before **TripleAlg** shapes it. **PostReso** adds resonance below center or phaser notches above it.
- Select **BBUS** for the bit-stream FIR and R-2R voice. **Fixed ByteBus word** in the row inspector accepts all 256 values, including `00`. With it off, playback uses the lane gate mask. Auditioning while stopped uses the standalone generator.
- Open **Lane Mixer** in the side panel for each lane's level, constant-power pan, and delay and BEND sends. The master delay send raises all lanes' delay send together. **M** and **S** in the lane headers control mute and solo. The shared **BEND** panel controls frequency shift, feedback, decay, and Freeze. Freeze holds the current feedback tail while muting new input to the bus.
- In **VarSampler Banks**, **Start** and **End** mark the active sample region. **Sustain Loop** repeats that region while the lane gate is high and finishes the current pass on release. With sustain looping off, the step's loop and scan locks work within the trim.
- **Export WAV...** bounces the selected pattern or full chain with the same timing and mixer as live playback. Optional aligned stereo stems include each lane's delay contribution and a shared `bend.wav` return when a lane has a BEND send. These taps precede master DC blocking and limiting. Export starts with an empty tank. With Freeze enabled, the first BEND send starts a 25 ms capture ramp and the tank then holds that sound.

## Custom WGSL Shaders & SoundLink UDP

- Drop any `.wgsl` file onto the window (or use **Load .wgsl** in the **Live Feedback Surface** panel) to run custom ping-pong fragment shaders driven by Leviathan's 64-byte `VisualUniforms` and `prev_tex` feedback buffer.
- **Export Starter .wgsl** writes a documented template matching the `@group(0)` ABI, and saving edits on disk automatically hot-reloads the shader while keeping the last valid GPU pipeline running if validation fails (**Reset Built-In Shader** restores the default Möbius/PDE shader).
- Toggle **SoundLink UDP** in the top bar to stream non-blocking `336`-byte `SoundLinkPacket` frames (`127.0.0.1:9420` default) to external visualizers or modular hosts.

## MIDI Input

Select an input in the **MIDI** menu (**Refresh MIDI** rescans ports; **Off** disconnects). Holding MIDI notes auditions the active lane's instrument and cursor sound locks in real time, **MIDI REC** step-records incoming notes and velocities at the tracker cursor with full `Ctrl+Z` undo support, and MIDI `Start` / `Continue` / `Stop` messages synchronize transport playback.

## Quick Start

```bash
# Verify strict no_std core & DSP crates, build all targets, and run unit tests
cargo check -p leviathan-core --no-default-features
cargo check -p leviathan-dsp --no-default-features
cargo test --workspace

# Launch the native Leviathan A/V workstation
cargo run --release -p leviathan-app
```

## Roadmap & Multi-Agent Coordination

- [ROADMAP.md](ROADMAP.md) — completed milestones (`M1`..`M9`) and completed Wave 1–2 workstreams (`T1`..`T10`) with owned files and acceptance criteria.
- [AGENTS.md](AGENTS.md) — architectural invariants, `#![no_std]` / lock-free RT audio rules, and pinned crate API notes (`eframe 0.31`, `cpal 0.17`, `midir 0.11`) for coordinating work across coding assistants.

## Clean-Room Provenance

See [CLEANROOM.md](CLEANROOM.md). Leviathan implements the public `BSD-3-Clause` normative specifications from `Gorgon/spec` without incorporating `AGPL-3.0` or `GPL-3.0` source code.
