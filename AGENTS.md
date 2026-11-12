# AGENTS.md — Multi-Agent Engineering Guide for Leviathan

This guide defines architectural invariants, clean-room licensing rules, pinned crate APIs, and coordination conventions for coding assistants (ChatGPT, Codex, Gemini, Claude, Cursor) and human contributors working on Leviathan.

## 1. Clean-Room & Licensing Guardrails (`BSD-3-Clause`)

- **License**: Everything in this repository is licensed under `BSD-3-Clause` (see [LICENSE](LICENSE) and [CLEANROOM.md](CLEANROOM.md)).
- **Strict Clean-Room Boundary**:
  - **Allowed**: Public `BSD-3-Clause` normative specifications from `Gorgon/spec` (Score v3 timing, `lowbias32` constant math, `1V/oct` CV conventions, 9-channel `0V`/`10V` ByteBus semantics, and the 336-byte `#[repr(C)]` `SoundLinkPacket` wire layout).
  - **Strictly Forbidden**: Never copy, translate, or adapt source code from `AGPL-3.0` (`hydrapy`) or `GPL-3.0` (`VCV Rack` or `vcv/adapter`) codebases.

## 2. Hexagonal Crate Boundaries

| Crate | `#![no_std]` | Allowed Dependencies | Architectural Rules |
| :--- | :--- | :--- | :--- |
| `crates/leviathan-core` | **Yes** (mandatory) | Optional `serde` (`default-features = false`) | Deterministic Score v3 model, `TrackerEvaluator`, `CvFrame`, `ByteBus`, `SoundLinkPacket`. Zero heap allocations in `step_sample`. |
| `crates/leviathan-dsp` | **Yes** (mandatory) | `leviathan-core` only | Zero-allocation synthesis (`SynthRack`, 4-bank `UQ16.16` `VarSampler`, 4-op `FourOpFm`, TPT SVF). No `std` and no `alloc` on the audio path. |
| `crates/leviathan-gpu` | No | `bytemuck` (plus `wgpu` if headless-safe) | `VisualUniforms` (`#[repr(C)]`, `Pod`, `Zeroable`), `FEEDBACK_SHADER_WGSL`, and software `FeedbackSurface` fallback. Keep testable without a window. |
| `crates/leviathan-app` | No | `leviathan-core`, `leviathan-dsp`, `leviathan-gpu`, `cpal`, `midir`, `rtrb`, `eframe`, `serde`, `serde_json` | Native desktop shell: `cpal` RT callback (`RtAudioWorker`), `rtrb` lock-free rings, WAV loader, and `egui` tracker UI. |

## 3. Real-Time Audio & Pinned Crate Gotchas

1. **Real-Time Audio Thread (`RtAudioWorker` in `crates/leviathan-app/src/audio.rs`)**:
   - Never allocate (`Box::new`, `Vec::push`, `format!`), lock a `Mutex`/`RwLock`, perform file/network I/O, or drop the final reference to an `Arc` inside the `cpal` callback thread.
   - Route commands through `AudioControl`, which retains pattern, sample and project snapshots on the UI thread and retries commands in order when the ring is full.
   - Use indexed `AudioCommand::SetPattern` for bulk edits. Use `ReplaceProject` for a validated session so patterns, sample banks and transport change at the same callback boundary.
2. **Pinned Crate Versions (`Cargo.toml`)**:
   - `eframe = "0.31"` (`egui 0.31`, `wgpu 24`):
     - Use `ctx.wants_keyboard_input()` (not the removed `egui_wants_keyboard_input()`).
     - Use `egui::Frame::new()` (not deprecated `Frame::none()`).
     - Intercept both `egui::Event::Copy`/`Cut`/`Paste` and `Modifiers::COMMAND`/`CTRL` for clipboard shortcuts (see `tracker_keys.rs`).
     - Add explicit `_f32` suffixes on ambiguous float literals (for example `1.5_f32`) to avoid `float_literal_f32_fallback` warnings on modern `rustc`.
   - `cpal = "0.17"`:
     - `cpal::StreamConfig.sample_rate` is a plain `u32` (not `SampleRate(u32)`), and `build_output_stream` takes `&StreamConfig`.
3. **Zero Dead-Code Warnings in `leviathan-app`**:
   - Because `leviathan-app` is a binary crate (`main.rs`) with private submodules (`wav`, `sampler_ui`, `tracker_edit`, `tracker_keys`), any unused `pub` item, struct field, or helper method triggers a compiler warning. Either wire new methods into the UI/toolbar or gate test-only helpers with `#[cfg(test)]`.

## 4. Mandatory Verification Gate (Zero Warnings)

Every change must pass all four checks across Linux, macOS, and Windows (enforced by `.github/workflows/ci.yml`):

```bash
cargo check -p leviathan-core --no-default-features
cargo check -p leviathan-dsp --no-default-features
cargo build --workspace --all-targets
cargo test --workspace
```

Target: **0 compiler warnings** and all workspace unit tests green.

## 5. Coordinating Work Across Assistants (`ROADMAP.md`)

1. Read [ROADMAP.md](ROADMAP.md) to pick an unclaimed ticket (`T1` through `T6`).
2. Each ticket owns specific files/crates so assistants can work sequentially on `main` or in parallel on feature branches (`feat/t1-wgpu-feedback`, `feat/t2-project-io`, etc.) with minimal merge friction in `main.rs`.
3. When completing a ticket:
   - Add unit tests covering the new logic.
   - Verify **0 compiler warnings** locally or in GitHub Actions CI.
   - Update the ticket status in `ROADMAP.md` with the commit SHA and a one-line note on any new keyboard shortcuts or `AudioCommand` variants.
