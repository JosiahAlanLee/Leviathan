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

## Wave 1 Completed Tickets (T1–T6)

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
- **Status**: `DONE` (implemented `inspector_ui.rs` with step `Prob`/`Fill`/`Gate`, pattern `Rows`/`Rate`, per-row `VisualAutomation` + `ByteBus` bit toggles, and lock-free `AudioCommand::SetAutomation`)
- **Owned Files**: new `crates/leviathan-app/src/inspector_ui.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Expose the Score v3 fields already evaluated by `leviathan-core` that aren't yet editable in the right-hand panel: `NoteCell.probability` (`0..=100%`), `NoteCell.fill` (`Always`, `WhenFill`, `WhenNotFill`), `NoteCell.gate_fraction`, lane clock rate multipliers, and per-row `VisualAutomation` (`z`, `roll`, `feedback`, `bytebus`).
- **Scope & Constraints**:
  - Extract the `Step Lock` + `Automation` inspector out of `main.rs` into `inspector_ui.rs` to keep `main.rs` concise and modular.
  - Preserve drag-coalesced undo recording (`EditHistory::record_before_edit` once per pointer drag gesture) and sync automation edits to `RtAudioWorker`.
- **Acceptance Criteria**:
  - All `NoteCell` conditional fields and row `VisualAutomation` values are editable in UI and reflected in live `CvFrame` telemetry; 0 warnings.

### `T5` — `#![no_std]` Stereo Master Bus (Delay, Soft-Clip Limiter & Peak Telemetry)
- **Status**: `DONE (merged in PR #2 — fixed-size #![no_std] stereo delay, DC blocker, linked soft-knee limiter, and L/R peak + CLIP meters)`
- **Owned Files**: new `crates/leviathan-dsp/src/master.rs`, `crates/leviathan-dsp/src/lib.rs`, transport meter readout in `crates/leviathan-app/src/main.rs`
- **Goal**: Add glue, spatial width, and overload protection to `SynthRack::render_stereo` without heap allocation.
- **Scope & Constraints**:
  - Fixed-size `#![no_std]` stereo ping-pong / Haas delay buffer (e.g. `[f32; 16384]` inside `SynthRack`) with global or lockable send amount, followed by a clean-room soft-knee limiter and DC blocker.
  - Compute stereo peak levels so the UI transport bar can render a compact L/R level + clip indicator.
- **Controls** `AudioCommand::SetMasterSend` adjusts the transport's **Delay send**. The send saves with the project and defaults to zero for older files. The meters display output peaks and hold pre-limiter overloads for one second.
- **Acceptance Criteria**:
  - `no_std` check passes (`cargo check -p leviathan-dsp --no-default-features`), unit test verifies bounded `[-1.0, 1.0]` output under hot 6-lane chord input, and 0 warnings.

### `T6` — Non-Blocking `SoundLinkPacket` Loopback UDP Bridge
- **Status**: `DONE (implemented safe little-endian 336-byte SoundLinkPacket non-blocking loopback UDP sender in link.rs with transport LINK toggle/port and loopback unit tests)`
- **Owned Files**: new `crates/leviathan-app/src/link.rs`, toggle in `crates/leviathan-app/src/main.rs`
- **Goal**: Broadcast the existing 336-byte `#[repr(C)]` `SoundLinkPacket` (`leviathan-core`) over loopback UDP for external shader/hardware receivers without touching the `cpal` thread.
- **Scope & Constraints**:
  - Never perform socket calls on the `cpal` audio thread; forward telemetry over a lock-free ring or UI-thread poller to a non-blocking `std::net::UdpSocket` sender.
- **Acceptance Criteria**:
  - Loopback UDP unit test verifying a 336-byte packet encodes, transmits, and validates magic/version headers cleanly; 0 warnings.
## Wave 2 Active Backlog (T7–T10 — Scoped for Multi-Agent Handoff)

**Recommended Parallel Split**:
- `T8` (`export.rs` / `wav.rs`) and `T10` (`leviathan-gpu` / `gpu_view.rs`) own nearly disjoint subsystems and can run in parallel across branches with minimal merge overlap.
- `T7` (`SynthRack` / `audio.rs` mute-solo) and `T9` (`midi.rs` / `Cargo.toml` / `audio.rs`) both hook into `audio.rs` and `main.rs`, so run them sequentially or rebase cleanly.

### `T7` — Lock-Free Per-Lane Mute & Solo Controls (`M` / `S` Toggles & Shortcuts)
- **Status**: `OPEN`
- **Owned Files**: `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/tracker_keys.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Add instant per-lane `Mute` (`M`) and `Solo` (`S`) controls in the 6-lane tracker header plus keyboard shortcuts (`Alt+1`..`Alt+6` mute lane, `Alt+Shift+1`..`Alt+Shift+6` solo lane, `Alt+0` clear all) without audio clicks.
- **Scope & Constraints**:
  - Compute a 6-bit `audible_mask: u8` (default `0b0011_1111`; when any lane is soloed, only soloed unmuted lanes remain audible) and send zero-allocation `AudioCommand::SetLaneAudibleMask(u8)` over `rtrb`.
  - Apply smooth per-lane gain ramping (`~3–5 ms` one-pole slew) inside `SynthRack` (`#![no_std]`) so muting/soloing a sustained bass or FM voice never produces a discontinuity click.
  - Persist `mute_mask: u8` and `solo_mask: u8` in `.leviathan.json` using `#[serde(default)]` so older projects open with all 6 lanes unmuted.
- **Acceptance Criteria**:
  - Unit tests covering mute/solo mask precedence, click-free/bounded `SynthRack` gain slew, and `#[serde(default)]` project round-trip; 0 compiler warnings.

### `T8` — Clean-Room Offline WAV Bounce & Per-Lane Stem Export
- **Status**: `OPEN`
- **Owned Files**: new `crates/leviathan-app/src/export.rs`, `crates/leviathan-app/src/wav.rs`, export dialog/button hook in `crates/leviathan-app/src/session_ui.rs` and `crates/leviathan-app/src/main.rs`
- **Goal**: Bounce the selected `Pattern` or full `PatternChain` faster-than-realtime to stereo RIFF/WAV files (`16-bit PCM` or `32-bit IEEE float`, selectable `44.1` / `48` / `96 kHz` sample rate, loop count, and delay-tail seconds), with an optional 6-lane stem export mode.
- **Scope & Constraints**:
  - Implement a clean-room RIFF/WAV encoder in `wav.rs` / `export.rs` (zero extra crate dependencies, matching the clean-room WAV decoder in `wav.rs`) with atomic temp-file writes.
  - Run offline rendering on an isolated `SongPlayer` + `SynthRack` instance (cloned from current patterns, chain, BPM, `master_send`, and `SampleBank`) on a background `std::thread` with atomic progress reporting so live `cpal` audio and `egui` frames never stall.
  - In stem mode, write individual lane files (`0-kick` through `5-acid`) plus the master bus mix.
- **Acceptance Criteria**:
  - Round-trip unit test rendering a multi-lane pattern to both `16-bit PCM` and `32-bit float` WAV bytes and decoding them back via `wav::decode_wav_bytes` with verified frame counts, sample rates, and non-silent peaks; 0 compiler warnings.

### `T9` — External MIDI Input (Live Audition, Step Record & MIDI Transport)
- **Status**: `IN REVIEW` (implementation `beac0d8`)
- **Controls**: MIDI port selector, **Refresh MIDI**, and **MIDI REC**. `AudioCommand::SetMidiTarget` follows the cursor, and `SetPlaying { playing, request }` acknowledges local play/stop changes.
- **Owned Files**: `Cargo.toml`, `crates/leviathan-app/Cargo.toml`, new `crates/leviathan-app/src/midi.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Support hardware/virtual MIDI input ports (`midir`) for live voice audition on the active lane, step-record note/velocity entry into the tracker grid, and MIDI `Start` (`0xFA`) / `Continue` (`0xFB`) / `Stop` (`0xFC`) transport sync.
- **Scope & Constraints**:
  - Keep `leviathan-core` and `leviathan-dsp` strictly `#![no_std]` with zero MIDI crate dependencies (`AGENTS.md` hexagonal boundary).
  - Never lock a `Mutex` inside the `midir` input callback: parse raw 1–3 byte MIDI messages (`NoteOn`, `NoteOff` including `NoteOn` with velocity `0`, and `0xFA`/`0xFB`/`0xFC` real-time transport) into a small `Copy` `MidiEvent` pushed over a bounded `rtrb::RingBuffer`.
  - Support live note audition on `RtAudioWorker` when stopped or playing, and degrade cleanly with `"MIDI: No ports"` when no MIDI hardware/ALSA sequencer is available (headless CI on Linux, macOS, and Windows).
- **Acceptance Criteria**:
  - Unit tests for the clean-room MIDI byte parser (`NoteOn`, `vel=0` `NoteOff`, `Start`/`Stop`, ignored SysEx/malformed bytes) and headless port-enumeration fallback; 0 compiler warnings across all 3 OS runners.

 ### `T10` — Custom User WGSL Shader Loading, Validation & Live Hot-Reload
 - **Status**: `DONE`
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, `crates/leviathan-app/src/gpu_view.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Allow users to drop or open custom `.wgsl` fragment shaders (matching Leviathan's 64-byte `VisualUniforms` + `prev_tex` + `prev_sampler` `@group(0)` ABI), automatically hot-reload when the `.wgsl` file's modification timestamp changes on disk, and keep the previous working pipeline alive if a shader edit fails validation.
- **Scope & Constraints**:
  - Route `.wgsl` file drag-and-drop events to `FeedbackViewport` while keeping `.wav` drops routed to `SamplerBankUi`.
  - Guard custom shader compilation with `wgpu` validation error scopes (`push_error_scope(wgpu::ErrorFilter::Validation)` / `pop_error_scope`) or pre-flight entry-point checks so syntax/type errors while live-coding **never panic the app**—display the shader error banner in the side panel while continuing to render the last valid shader.
  - Add `"Export Starter .wgsl"` and `"Reset Built-In Shader"` buttons, and persist optional custom shader source/path in `.leviathan.json` via `#[serde(default)]`.
- **Acceptance Criteria**:
  - Unit tests verifying starter WGSL template ABI markers, `.wgsl` vs `.wav` drop routing, fallback retention on invalid shader input, and `#[serde(default)]` project round-trip; 0 compiler warnings.
