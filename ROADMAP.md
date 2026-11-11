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

### `T2` — Versioned Project Save / Load (`.leviathan.json`)
- **Status**: `IN REVIEW` (implementation `d9b8721`)
- **Owned Files**: `crates/leviathan-app/src/project.rs`, `session_ui.rs`, project hooks in `main.rs`, and serde support in `leviathan-core`.
- **Scope**:
  - Version 1 stores BPM, stable pattern slots, the chain, the edit selection, and the decoded PCM and playback rate for all four sampler banks. Embedding the bank data also preserves WAVs dropped as bytes.
  - `Ctrl/Cmd+S` saves, `Ctrl/Cmd+Shift+S` saves to a new path, and `Ctrl/Cmd+O` opens. Saves use a temporary file in the destination directory. A validated project replaces the audio state through one `ReplaceProject` command and opens paused.
- **Acceptance**: The round-trip and tracker shortcut tests cover patterned sessions, sound locks, conditions, automation, and sample data. Invalid projects are rejected before replacing the session.

### `T3` — Multi-Pattern Bank & Quantized Live Song Chain
- **Status**: `IN REVIEW` (implementation `d9b8721`)
- **Owned Files**: `crates/leviathan-core/src/song.rs`, `crates/leviathan-app/src/audio.rs`, `session_ui.rs`, and the tracker and transport hooks in `main.rs`.
- **Scope**:
  - The editor supports 16 pattern slots, per-pattern rates and undo history, and a chain of up to 64 entries. While playing, the user can edit one pattern and queue another pattern or the chain.
  - Launches land on row `00` of the incoming pattern on the first sample at the boundary. Each pattern has a local phase and the master clock continues through rate changes and live BPM edits.
- **Acceptance**: Core and callback tests cover mixed-rate launches, repeated chain entries, tempo continuity, and the first note and automation value of the incoming pattern.

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
