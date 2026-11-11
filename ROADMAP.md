# Leviathan Roadmap & Multi-Agent Task Queue

Read [AGENTS.md](AGENTS.md) before picking up a ticket for `#![no_std]`, lock-free audio, and `BSD-3-Clause` clean-room invariants.

## Completed Baseline (M1–M9 — Commit `27f1057`)

- [x] **M1–M2**: Cargo workspace, `BSD-3-Clause` [CLEANROOM.md](CLEANROOM.md), and Linux/macOS/Windows GitHub Actions CI (0 warnings).
- [x] **M3**: `#![no_std]` `leviathan-core` — Score v3 `Pattern`/`NoteCell`/`SoundLock`, `lowbias32` probability, `1V/oct` CV, 9-channel `ByteBus`, and 336-byte `#[repr(C)]` `SoundLinkPacket`.
- [x] **M4–M6**: `#![no_std]` `leviathan-dsp` — 7-machine zero-allocation rack including 4-bank `UQ16.16` variable-clock sampler (`sampler.rs`) and 4-operator FM synth (`fm.rs`) with 8 per-step `SoundLock` slots.
- [x] **M7**: Clean-room RIFF/WAV loader (`8`/`16`/`24`-bit PCM & `32`-bit float) and runtime drag-and-drop bank inspector (`wav.rs`, `sampler_ui.rs`).
- [x] **M8**: Fast keyboard tracker navigation (`Arrows`, `PgUp`/`PgDn`, `Home`/`End`, `Tab`), 2-octave piano entry (`Z–M` / `Q–P`), hex velocity (`00–7F`), and `AIR` step entry.
- [x] **M9**: 64-step `EditHistory` (`Ctrl+Z`/`Ctrl+Y`/`Ctrl+Shift+Z`), `Shift+Arrows`/`Shift+Click`/`Ctrl+A` block selection, `Ctrl+C`/`X`/`V` block copy/cut/paste, `±1`/`±12` semitone transpose (`Ctrl+Up`/`Down`), and lock-free `AudioCommand::SetPattern` sync (`tracker_edit.rs`, `tracker_keys.rs`, `main.rs`).

## Active Backlog (Scoped for Multi-Agent Handoff)

### `T1` — Hardware `wgpu` Ping-Pong Feedback Render Pipeline
- **Status**: `OPEN`
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, new `crates/leviathan-app/src/gpu_view.rs`, light hook in `crates/leviathan-app/src/main.rs`
- **Goal**: Upgrade the live visual preview from the 160×90 CPU reference rasterizer (`FeedbackSurface`) to a real GPU ping-pong texture pipeline running `FEEDBACK_SHADER_WGSL` via `eframe::egui_wgpu::RenderState`.
- **Scope & Constraints**:
  - Allocate two ping-pong textures (`Rgba8Unorm` or `Rgba16Float`, e.g. `640×360`), a uniform buffer updated each frame from `VisualUniforms`, and bind groups registered as an `egui::TextureId`.
  - Keep `FeedbackSurface` intact in `leviathan-gpu` as the automatic fallback when `cc.wgpu_render_state` is `None` (headless CI / software rasterizer) so no dead-code warnings are introduced.
- **Acceptance Criteria**:
  - Live feedback shader runs on GPU when `wgpu` is active and falls back cleanly when unavailable.
  - Add a shader/uniform layout validation test; 0 compiler warnings across all 3 OS targets.

### `T2` — Versioned Project Save / Load (`.leviathan.json`)
- **Status**: `OPEN`
- **Owned Files**: `crates/leviathan-core/Cargo.toml`, `crates/leviathan-core/src/lib.rs`, new `crates/leviathan-app/src/project.rs`, toolbar/shortcut hook in `crates/leviathan-app/src/main.rs`
- **Goal**: Persist and reload full tracker sessions on disk using workspace `serde` + `serde_json` (already declared in root `Cargo.toml`).
- **Scope & Constraints**:
  - Gate `#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]` on `Pattern`, `NoteCell`, `SoundLock`, `FillCondition`, and `VisualAutomation` in `leviathan-core` so `cargo check -p leviathan-core --no-default-features` stays strictly `#![no_std]` and dependency-free.
  - Enable `leviathan-core = { workspace = true, features = ["serde"] }` inside `crates/leviathan-app/Cargo.toml`.
  - Define a versioned `ProjectFile { version: u32, bpm: f64, pattern: Pattern, sample_paths: [Option<String>; 4] }` in `project.rs` with atomic temp-file write, `Ctrl+S` / `Ctrl+O` shortcuts, and WAV bank reload from stored paths when present.
  - Push `AudioCommand::SetBpm` and `AudioCommand::SetPattern` upon load.
- **Acceptance Criteria**:
  - Round-trip `serde_json` unit test verifying notes, `AIR` flags, `FillCondition`, `SoundLock` masks/values, and `VisualAutomation` survive save/load bit-for-bit; 0 warnings.

### `T3` — Multi-Pattern Bank & Quantized Live Song Chain
- **Status**: `OPEN`
- **Owned Files**: `crates/leviathan-core/src/lib.rs`, `crates/leviathan-app/src/tracker_edit.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Expand from a single 64-row `Pattern` to a multi-pattern bank (e.g., 16 patterns `00`–`0F`) with live quantized pattern queuing and a compact song-chain bar.
- **Scope & Constraints**:
  - Support immediate pattern editing in the UI (`Prev` / `Next` / `Clone` / pattern selector) while allowing the audio engine to either follow immediately when stopped or switch seamlessly on row `00` wraparound (`QueuedPattern`) while playing.
  - Keep `TrackerEvaluator` `#![no_std]` and zero-allocation; pass pattern snapshots via UI-retained `Arc<Pattern>` (`EditHistory::make_rt_snapshot`).
- **Acceptance Criteria**:
  - Unit test in `leviathan-core` verifying row-`00` quantized pattern transition without phase drift; 0 warnings.

### `T4` — Step Conditions (`Prob`, `Fill`, `Gate`, `Rate`) & Visual Automation Inspector
- **Status**: `OPEN`
- **Owned Files**: new `crates/leviathan-app/src/inspector_ui.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Expose the Score v3 fields already evaluated by `leviathan-core` that aren't yet editable in the right-hand panel: `NoteCell.probability` (`0..=100%`), `NoteCell.fill` (`Always`, `WhenFill`, `WhenNotFill`), `NoteCell.gate_fraction`, lane clock rate multipliers, and per-row `VisualAutomation` (`z`, `roll`, `feedback`, `bytebus`).
- **Scope & Constraints**:
  - Extract the `Step Lock` + `Automation` inspector out of `main.rs` into `inspector_ui.rs` to keep `main.rs` concise and modular.
  - Preserve drag-coalesced undo recording (`EditHistory::record_before_edit` once per pointer drag gesture) and sync automation edits to `RtAudioWorker`.
- **Acceptance Criteria**:
  - All `NoteCell` conditional fields and row `VisualAutomation` values are editable in UI and reflected in live `CvFrame` telemetry; 0 warnings.

### `T5` — `#![no_std]` Stereo Master Bus (Delay, Soft-Clip Limiter & Peak Telemetry)
- **Status**: `OPEN`
- **Owned Files**: new `crates/leviathan-dsp/src/master.rs`, `crates/leviathan-dsp/src/lib.rs`, transport meter readout in `crates/leviathan-app/src/main.rs`
- **Goal**: Add glue, spatial width, and overload protection to `SynthRack::render_stereo` without heap allocation.
- **Scope & Constraints**:
  - Fixed-size `#![no_std]` stereo ping-pong / Haas delay buffer (e.g. `[f32; 16384]` inside `SynthRack`) with global or lockable send amount, followed by a clean-room soft-knee limiter and DC blocker.
  - Compute stereo peak levels so the UI transport bar can render a compact L/R level + clip indicator.
- **Acceptance Criteria**:
  - `no_std` check passes (`cargo check -p leviathan-dsp --no-default-features`), unit test verifies bounded `[-1.0, 1.0]` output under hot 6-lane chord input, and 0 warnings.

### `T6` — Non-Blocking `SoundLinkPacket` Loopback UDP Bridge
- **Status**: `OPEN`
- **Owned Files**: new `crates/leviathan-app/src/link.rs`, toggle in `crates/leviathan-app/src/main.rs`
- **Goal**: Broadcast the existing 336-byte `#[repr(C)]` `SoundLinkPacket` (`leviathan-core`) over loopback UDP for external shader/hardware receivers without touching the `cpal` thread.
- **Scope & Constraints**:
  - Never perform socket calls on the `cpal` audio thread; forward telemetry over a lock-free ring or UI-thread poller to a non-blocking `std::net::UdpSocket` sender.
- **Acceptance Criteria**:
  - Loopback UDP unit test verifying a 336-byte packet encodes, transmits, and validates magic/version headers cleanly; 0 warnings.
