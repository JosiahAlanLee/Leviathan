# Leviathan# Leviathan

A native, single-binary audio/visual tracker workstation written in clean-room Rust (`BSD-3-Clause`).

Leviathan consolidates deterministic Score v3 sequencing, sample-accurate modular CV/ByteBus generation, internal zero-allocation synthesis, GPU ping-pong shader feedback (`wgpu`), and an immediate-mode tracker UI (`egui`) into a single reproducible Cargo workspace—eliminating cross-process interpreter, browser, and legacy OpenGL seams.

## Workspace Architecture

Leviathan uses a strict hexagonal ("ports and adapters") layout so external GPU, audio, or GUI crate updates never touch core timing, score evaluation, or DSP math:

| Crate | `#![no_std]` | External Deps | Responsibility |
| :--- | :--- | :--- | :--- |
| `crates/leviathan-core` | Yes | Optional `serde` only | Score v3 data model (6 note lanes, `FILL` conditions, per-step `SoundLock`s, visual automation), `lowbias32` probability hashing, `1V/oct` CV & gate evaluation, 9-channel `0V`/`10V` ByteBus encoding, and binary `#[repr(C)]` `SoundLinkPacket` ABI (`336` bytes). |
| `crates/leviathan-dsp` | Yes | `leviathan-core` only | Zero-allocation 7-machine instrument rack (`KickDrum`, `FourOpFm`, `SubBass`, `NoiseWash`, `VarSampler` 4-bank `UQ16.16` variable-clock sampler, `AcidLead`, `ChordStab`), TPT state-variable filters, and per-step sound-lock overrides. |
| `crates/leviathan-gpu` | No | `bytemuck` | Uniform buffer layouts (`VisualUniforms`), WGSL multi-buffer feedback / reaction-diffusion shader (`FEEDBACK_SHADER_WGSL`), and software reference `FeedbackSurface`. |
| `crates/leviathan-app` | No | `cpal`, `rtrb`, `eframe` | Native desktop workstation hosting the lock-free `cpal` real-time audio engine, `rtrb` command/telemetry rings, RIFF/WAV drag-and-drop sample loader, and keyboard-driven `egui` 64-row tracker editor. |

## Tracker Keyboard Shortcuts

| Action | Shortcut |
| :--- | :--- |
| Play / Stop transport | `Space` |
| Navigate cursor | `Arrows`, `PgUp`/`PgDn` (±16 rows), `Home`/`End`, `Tab` / `Shift+Tab` |
| 2+ octave piano note entry | `Z S X D C V G B H N J M` (lower) & `Q 2 W 3 E R 5 T 6 Y 7 U I 9 O 0 P` (upper) |
| Base octave / edit step | `F1`/`F2` or `-`/`=` (octave), `F3`/`F4` or `[`/`]` (step `0..=16`) |
| Hex velocity (`00`–`7F`) & `AIR` | `0`–`9` / `A`–`F` on `VEL` column; `` ` `` inserts `AIR` trigger |
| Rectangular block selection | `Shift + Arrows`, `Shift + PgUp/PgDn/Home/End`, `Shift + Click`, `Ctrl+A` (lane → all), `Esc` |
| Undo / Redo (64 steps) | `Ctrl+Z` / `Ctrl+Y` or `Ctrl+Shift+Z` (`Cmd` on macOS) |
| Copy / Cut / Paste block | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Clear / Wipe locks | `Delete` (clear notes) / `Shift+Delete` (clear notes + `SoundLock`s) |
| Semitone / Octave transpose | `Ctrl + Up/Down` (±1 semitone) / `Ctrl + Shift + Up/Down` (±12 semitones) |

## Projects and pattern chains

- **Save** (`Ctrl/Cmd+S`) stores the pattern bank, chain, tempo, sound locks, visual automation and decoded WAV banks in one `.leviathan.json` file. The first save asks for a path. **Save As** (`Ctrl/Cmd+Shift+S`) chooses a new path.
- **Open** (`Ctrl/Cmd+O`) restores the session with playback paused. The project carries its own sampler data.
- **New Pattern** and **Clone** build a bank of up to 16 patterns. Select a pattern to edit it, and set its rate in the pattern bar. While playing, **Queue Pattern** switches at the end of the active pattern. A rate edit to the active pattern takes effect on its next pass.
- **Add Pattern** appends the selected pattern to the chain. Entries can repeat. **Cue Chain** starts the chain from its first entry when you press Play. **Queue Chain** launches it at the next pattern boundary during playback.

The transport's **Delay send** feeds a short stereo ping-pong delay and saves with the project. The L/R meters show output peaks in dBFS. **CLIP** holds for one second when the mix exceeds full scale before limiting.

Use **M** and **S** in each lane header, or `Alt+1`–`Alt+6` to mute, `Alt+Shift+1`–`Alt+Shift+6` to solo, and `Alt+0` to clear both. Mute takes precedence on a soloed lane. Both settings save with the project.

**Export WAV…** bounces the selected pattern or song chain with a loop count and tail length. Choose 16-bit PCM or 32-bit float at 44.1, 48, or 96 kHz. Stem export creates a new folder with six stereo lane files taken before the master bus plus `master.wav`. The bounce captures the current lane mix, delay send, and FILL state.

## Quick Start

```bash
# Verify strict no_std core & DSP crates, build all targets, and run unit tests
cargo check -p leviathan-core --no-default-features
cargo check -p leviathan-dsp --no-default-features
cargo test --workspace

# Launch the native Leviathan A/V workstation
cargo run --release -p leviathan-app
```

## MIDI input

Select an input in the **MIDI** menu. Use **Refresh MIDI** to rescan and **Off** to disconnect. A held note auditions the instrument and sound locks at the tracker cursor on that lane. Release returns the lane to the sequence.

Arm **MIDI REC** to enter notes and velocities at the cursor with the current edit step. MIDI Start resets playback, Continue resumes from the current position, and Stop pauses. Playback uses the project BPM.

## Roadmap & Multi-Agent Coordination

- [ROADMAP.md](ROADMAP.md) — completed milestones (`M1`–`M9`) and scoped next-step workstreams (`T1`–`T6`) with owned files and acceptance criteria.
- [AGENTS.md](AGENTS.md) — architectural invariants, `#![no_std]` / lock-free RT audio rules, and pinned crate API notes (`eframe 0.31`, `cpal 0.17`) for coordinating work across coding assistants.

## Clean-Room Provenance

See [CLEANROOM.md](CLEANROOM.md). Leviathan implements the public `BSD-3-Clause` normative specifications from `Gorgon/spec` without incorporating `AGPL-3.0` or `GPL-3.0` source code.

A native, single-binary audio/visual tracker workstation written in clean-room Rust (`BSD-3-Clause`).

Leviathan consolidates deterministic Score v3 sequencing, sample-accurate modular CV/ByteBus generation, internal zero-allocation synthesis, GPU ping-pong shader feedback (`wgpu`), and an immediate-mode tracker UI (`egui`) into a single reproducible Cargo workspace—eliminating cross-process interpreter, browser, and legacy OpenGL seams.

## Workspace Architecture

Leviathan uses a strict hexagonal ("ports and adapters") layout so external GPU, audio, or GUI crate updates never touch core timing, score evaluation, or DSP math:

| Crate | `#![no_std]` | External Deps | Responsibility |
| :--- | :--- | :--- | :--- |
| `crates/leviathan-core` | Yes | Optional `serde` only | Score v3 data model (6 note lanes, `FILL` conditions, per-step `SoundLock`s, visual automation), `lowbias32` probability hashing, `1V/oct` CV & gate evaluation, 9-channel `0V`/`10V` ByteBus encoding, and binary `#[repr(C)]` `SoundLinkPacket` ABI (`336` bytes). |
| `crates/leviathan-dsp` | Yes | `leviathan-core` only | Zero-allocation 7-machine instrument rack (`KickDrum`, `FourOpFm`, `SubBass`, `NoiseWash`, `VarSampler` 4-bank `UQ16.16` variable-clock sampler, `AcidLead`, `ChordStab`), TPT state-variable filters, and per-step sound-lock overrides. |
| `crates/leviathan-gpu` | No | `bytemuck` | Uniform buffer layouts (`VisualUniforms`), WGSL multi-buffer feedback / reaction-diffusion shader (`FEEDBACK_SHADER_WGSL`), and software reference `FeedbackSurface`. |
| `crates/leviathan-app` | No | `cpal`, `rtrb`, `eframe` | Native desktop workstation hosting the lock-free `cpal` real-time audio engine, `rtrb` command/telemetry rings, RIFF/WAV drag-and-drop sample loader, and keyboard-driven `egui` 64-row tracker editor. |

## Tracker Keyboard Shortcuts

| Action | Shortcut |
| :--- | :--- |
| Play / Stop transport | `Space` |
| Navigate cursor | `Arrows`, `PgUp`/`PgDn` (±16 rows), `Home`/`End`, `Tab` / `Shift+Tab` |
| 2+ octave piano note entry | `Z S X D C V G B H N J M` (lower) & `Q 2 W 3 E R 5 T 6 Y 7 U I 9 O 0 P` (upper) |
| Base octave / edit step | `F1`/`F2` or `-`/`=` (octave), `F3`/`F4` or `[`/`]` (step `0..=16`) |
| Hex velocity (`00`–`7F`) & `AIR` | `0`–`9` / `A`–`F` on `VEL` column; `` ` `` inserts `AIR` trigger |
| Rectangular block selection | `Shift + Arrows`, `Shift + PgUp/PgDn/Home/End`, `Shift + Click`, `Ctrl+A` (lane → all), `Esc` |
| Undo / Redo (64 steps) | `Ctrl+Z` / `Ctrl+Y` or `Ctrl+Shift+Z` (`Cmd` on macOS) |
| Copy / Cut / Paste block | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Clear / Wipe locks | `Delete` (clear notes) / `Shift+Delete` (clear notes + `SoundLock`s) |
| Semitone / Octave transpose | `Ctrl + Up/Down` (±1 semitone) / `Ctrl + Shift + Up/Down` (±12 semitones) |

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

- [ROADMAP.md](ROADMAP.md) — completed milestones (`M1`–`M9`) and scoped next-step workstreams (`T1`–`T6`) with owned files and acceptance criteria.
- [AGENTS.md](AGENTS.md) — architectural invariants, `#![no_std]` / lock-free RT audio rules, and pinned crate API notes (`eframe 0.31`, `cpal 0.17`) for coordinating work across coding assistants.

## Clean-Room Provenance

See [CLEANROOM.md](CLEANROOM.md). Leviathan implements the public `BSD-3-Clause` normative specifications from `Gorgon/spec` without incorporating `AGPL-3.0` or `GPL-3.0` source code.Leviathan consolidates deterministic Score v3 sequencing, sample-accurate modular CV/ByteBus generation, internal zero-allocation synthesis, GPU ping-pong shader feedback (`wgpu`), and an immediate-mode tracker UI (`egui`) into a single reproducible Cargo workspace—eliminating cross-process interpreter, browser, and legacy OpenGL seams.

## Workspace Architecture

Leviathan uses a strict hexagonal ("ports and adapters") layout so external GPU, audio, or GUI crate updates never touch core timing, score evaluation, or DSP math:

| Crate | `#![no_std]` | External Deps | Responsibility |
| :--- | :--- | :--- | :--- |
| `crates/leviathan-core` | Yes | Optional `serde` only | Score v3 data model (128 patterns, 6 note lanes, `1/4x`–`8x` rates, `FILL` layers), `lowbias32` probability hashing, `1V/oct` CV & gate evaluation, 9-channel `0V`/`10V` ByteBus encoding, and binary `#[repr(C)]` `SoundLinkPacket` ABI (`336` bytes). |
| `crates/leviathan-dsp` | Yes | `leviathan-core` only | Zero-allocation 6-lane internal instrument rack (`DrumPerc`, `FmGlass`, `SubBass`, `NoiseWash`, `ChordStab`, `AcidLead`), TPT state-variable filters, per-hit sound-lock overrides, and 8-bit `ByteBus` counter (`Loop`, `OneShot`, `Pendulum`). |
| `crates/leviathan-gpu` | No | `bytemuck` | Uniform buffer layouts (`VisualUniforms`) and WGSL multi-buffer feedback / reaction-diffusion shader sources decoupled from windowing backends. |
| `crates/leviathan-app` | No | `cpal`, `rtrb`, `eframe` | Native desktop workstation hosting the lock-free `cpal` real-time audio engine, `rtrb` telemetry ring, `egui` 64-row tracker view, and live shader preview. |

## Quick Start

```bash
# Run all core, DSP, and shader unit tests
cargo test --workspace

# Launch the native Leviathan A/V workstation
cargo run --release -p leviathan-app
```

## Clean-Room Provenance

See [`CLEANROOM.md`](CLEANROOM.md). Leviathan implements the public `BSD-3-Clause` normative specifications from [`JosiahAlanLee/Gorgon/spec`](https://github.com/JosiahAlanLee/Gorgon/tree/main/spec) without incorporating `AGPL-3.0` or `GPL-3.0` source code.
