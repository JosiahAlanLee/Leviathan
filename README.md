# Leviathan

A native, single-binary audio/visual tracker workstation written in clean-room Rust (`BSD-3-Clause`).

Leviathan consolidates deterministic Score v3 sequencing, sample-accurate modular CV/ByteBus generation, internal zero-allocation synthesis, GPU ping-pong shader feedback (`wgpu`), and an immediate-mode tracker UI (`egui`) into a single reproducible Cargo workspace—eliminating cross-process interpreter, browser, and legacy OpenGL seams.

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
