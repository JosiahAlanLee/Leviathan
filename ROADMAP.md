# Leviathan Roadmap & Multi-Agent Task Queue

Read [AGENTS.md](AGENTS.md) before picking up a ticket for `#![no_std]`, lock-free audio, and `BSD-3-Clause` clean-room invariants.

## Completed Baseline (M1-M9 — Commit `27f1057`)

- [x] **M1-M2**: Cargo workspace, `BSD-3-Clause` [CLEANROOM.md](CLEANROOM.md), and Linux/macOS/Windows GitHub Actions CI (0 warnings).
- [x] **M3**: `#![no_std]` `leviathan-core` — Score v3 `Pattern`/`NoteCell`/`SoundLock`, `lowbias32` probability, `1V/oct` CV, 9-channel `ByteBus`, and 336-byte `#[repr(C)]` `SoundLinkPacket`.
- [x] **M4-M6**: `#![no_std]` `leviathan-dsp` — 7-machine zero-allocation rack including 4-bank `UQ16.16` variable-clock sampler (`sampler.rs`) and 4-operator FM synth (`fm.rs`) with 8 per-step `SoundLock` slots.
- [x] **M7**: Clean-room RIFF/WAV loader (`8`/`16`/`24`-bit PCM & `32`-bit float) and runtime drag-and-drop bank inspector (`wav.rs`, `sampler_ui.rs`).
- [x] **M8**: Fast keyboard tracker navigation (`Arrows`, `PgUp`/`PgDn`, `Home`/`End`, `Tab`), 2-octave piano entry (`Z-M` / `Q-P`), hex velocity (`00-7F`), and `AIR` step entry.
- [x] **M9**: 64-step `EditHistory` (`Ctrl+Z`/`Ctrl+Y`/`Ctrl+Shift+Z`), `Shift+Arrows`/`Shift+Click`/`Ctrl+A` block selection, `Ctrl+C`/`X`/`V` block copy/cut/paste, `±1`/`±12` semitone transpose (`Ctrl+Up`/`Down`), and lock-free `AudioCommand::SetPattern` sync (`tracker_edit.rs`, `tracker_keys.rs`, `main.rs`).
- [x] **T1**: Hardware `wgpu` ping-pong feedback render pipeline (`gpu_view.rs`).

## Active Backlog (Scoped for Multi-Agent Handoff)

### `T1` — Hardware `wgpu` Ping-Pong Feedback Render Pipeline
- **Status**: `DONE` (implemented 640x360 `Rgba8Unorm` ping-pong `wgpu` pipeline in `gpu_view.rs` with native `egui_wgpu` texture swap and automatic `FeedbackSurface` CPU fallback)
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, new `crates/leviathan-app/src/gpu_view.rs`, light hook in `crates/leviathan-app/src/main.rs`
- **Goal**: Upgrade the live visual preview from the 160×90 CPU reference rasterizer (`FeedbackSurface`) to a real GPU ping-pong texture pipeline running `FEEDBACK_SHADER_WGSL` via `eframe::egui_wgpu::RenderState`.
- **Scope & Constraints**:
  - Allocate two ping-pong textures (`Rgba8Unorm` or `Rgba16Float`, e.g. `640×360`), a uniform buffer updated each frame from `VisualUniforms`, and bind groups registered as an `egui::TextureId`.
  - Keep `FeedbackSurface` intact in `leviathan-gpu` as the automatic fallback when `cc.wgpu_render_state` is `None` (e.g., in headless CI or software render mode) so no existing tests or headless targets break.
  - Expose a clean, self-contained `FeedbackViewport` struct in `gpu_view.rs` consumed by `main.rs`.
  - Add unit tests verifying WGSL shader compilation/binding structure and `FeedbackViewport` creation/fallback mechanics.
