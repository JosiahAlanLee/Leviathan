# Leviathan Roadmap & Multi-Agent Task Queue

Read [AGENTS.md](AGENTS.md) before picking up a ticket for `#![no_std]`, lock-free audio, and `BSD-3-Clause` clean-room invariants.

## Completed Baseline (M1–M9 — Commit `27f1057`)

- [x] **M1–M2**: Cargo workspace, `BSD-3-Clause` [CLEANROOM.md](CLEANROOM.md), and Linux/macOS/Windows GitHub Actions CI (`0` warnings).
- [x] **M3**: `#![no_std]` `leviathan-core` — Score v3 `Pattern` / `NoteCell` / `SoundLock`, `lowbias32` probability, `1V/oct` CV, 9-channel `ByteBus`, and 336-byte `#[repr(C)]` `SoundLinkPacket`.
- [x] **M4–M6**: `#![no_std]` `leviathan-dsp` — 7-machine zero-allocation rack including 4-bank `UQ16.16` variable-clock sampler (`sampler.rs`) and 4-operator FM synth (`fm.rs`) with 8 per-step `SoundLock` slots.
- [x] **M7**: Clean-room RIFF/WAV loader (`8`/`16`/`24`-bit PCM & `32`-bit float) and runtime drag-and-drop bank inspector (`wav.rs`, `sampler_ui.rs`).
- [x] **M8**: Fast keyboard tracker navigation (`Arrows`, `PgUp`/`PgDn`, `Home`/`End`, `Tab`), 2-octave piano entry (`Z`–`M` / `Q`–`P`), hex velocity (`00`–`7F`), and `AIR` step entry.
- [x] **M9**: 64-step `EditHistory` (`Ctrl+Z`/`Ctrl+Y`/`Ctrl+Shift+Z`), `Shift+Arrows`/`Shift+Click`/`Ctrl+A` block selection, `Ctrl+C`/`X`/`V` block copy/cut/paste, `±1`/`±12` semitone transpose (`Ctrl+Up`/`Down`), and lock-free `AudioCommand::SetPattern` sync (`tracker_edit.rs`, `tracker_keys.rs`, `main.rs`).

## Wave 1 Completed Tickets (T1–T6)

### `T1` — Hardware `wgpu` Ping-Pong Feedback Render Pipeline
- **Status**: `DONE` (640×360 `Rgba8Unorm` ping-pong `wgpu` pipeline in `gpu_view.rs` with `FeedbackSurface` CPU fallback)
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, `crates/leviathan-app/src/gpu_view.rs`, `crates/leviathan-app/src/main.rs`

### `T2` — Versioned Project Save / Load (`.leviathan.json`)
- **Status**: `DONE` (merged in PR #1 — atomic `.leviathan.json` save/open with embedded `SampleBank` PCM and strict validation)
- **Owned Files**: `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/session_ui.rs`, `crates/leviathan-app/src/main.rs`

### `T3` — Multi-Pattern Bank & Quantized Live Song Chain
- **Status**: `DONE` (merged in PR #1 — 16-pattern bank, per-pattern `0.25x`..`4x` playback rates, 64-entry `PatternChain`, and quantized `SongPlayer` launches)
- **Owned Files**: `crates/leviathan-core/src/song.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/session_ui.rs`

### `T4` — Step Conditions (`Prob`, `Fill`, `Gate`, `Rate`) & Visual Automation Inspector
- **Status**: `DONE` (`inspector_ui.rs` step conditions, pattern length/rate controls, row automation sliders, and drag-coalesced undo)
- **Owned Files**: `crates/leviathan-app/src/inspector_ui.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/main.rs`

### `T5` — `#![no_std]` Stereo Master Bus (Delay, Soft-Clip Limiter & Peak Telemetry)
- **Status**: `DONE` (merged in PR #2 — `#![no_std]` stereo ping-pong delay, DC blocker, linked soft-knee limiter, and atomic peak meters)
- **Owned Files**: `crates/leviathan-dsp/src/master.rs`, `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-app/src/master_ui.rs`, `crates/leviathan-app/src/main.rs`

### `T6` — Non-Blocking `SoundLinkPacket` Loopback UDP Bridge
- **Status**: `DONE` (non-blocking 336-byte little-endian `SoundLinkPacket` UDP loopback sender in `link.rs` with top-bar toggle)
- **Owned Files**: `crates/leviathan-app/src/link.rs`, `crates/leviathan-app/src/main.rs`

## Wave 2 Completed Tickets (T7–T10)

### `T7` — Lock-Free Per-Lane Mute & Solo Controls (`M` / `S` Toggles & Shortcuts)
- **Status**: `DONE` (merged in PR #3 — 6-bit `audible_mask`, click-free `SynthRack` gain slew, `Alt+1..6` / `Alt+Shift+1..6` / `Alt+0` shortcuts, and `#[serde(default)]` persistence)
- **Owned Files**: `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-app/src/lane_mix.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/tracker_keys.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`

### `T8` — Clean-Room Offline WAV Bounce & Per-Lane Stem Export
- **Status**: `DONE` (merged in PR #3 — background worker `16-bit PCM` & `32-bit IEEE float` stereo WAV master and 6-lane stem export with atomic staging writes)
- **Owned Files**: `crates/leviathan-app/src/export.rs`, `crates/leviathan-app/src/export_ui.rs`, `crates/leviathan-app/src/wav.rs`, `crates/leviathan-app/src/session_ui.rs`, `crates/leviathan-app/src/main.rs`

### `T9` — External MIDI Input (Live Audition, Step Record & MIDI Transport)
- **Status**: `DONE` (merged in PR #4 — lock-free `midir` input, multi-message chord/running-status/realtime packet framer, live audition, undo-backed `MIDI REC`, and `Start`/`Continue`/`Stop` sync)
- **Owned Files**: `Cargo.toml`, `crates/leviathan-app/Cargo.toml`, `crates/leviathan-app/src/midi.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/main.rs`

### `T10` — Custom User WGSL Shader Loading, Validation & Live Hot-Reload
- **Status**: `DONE` (`STARTER_SHADER_TEMPLATE_WGSL`, `wgpu` validation error scopes, `.wgsl` drag-and-drop, `0.5s` mtime hot-reload with last-valid pipeline retention, and `#[serde(default)]` project persistence)
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, `crates/leviathan-app/src/gpu_view.rs`, `crates/leviathan-app/src/sampler_ui.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`

## Wave 3 Active Backlog (T11–T14 — Scoped for Multi-Agent Handoff)

**Recommended Parallel Split**:
- **Zero-Overlap Pair (`T13` + `T14`)**: `T13` (`leviathan-gpu` / `gpu_view.rs` `@group(1)` audio waveform & band texture) and `T14` (`tracker_edit.rs` / `smf.rs` / `gen_ui.rs` generative pattern tools & `.mid` import/export) touch disjoint subsystems with neither modifying `project.rs`, `leviathan-core`, or `leviathan-dsp`.
- **Core vs. DSP Pair (`T11` + `T12`)**: `T11` (`leviathan-core` / `inspector_ui.rs` swing, microtiming & ratchets) and `T12` (`leviathan-dsp` / `sampler.rs` stereo lane mixer & sampler loop/trim) own separate crates (`leviathan-core` vs. `leviathan-dsp`); either pairs cleanly in parallel with `T13` or `T14`, while `T11` and `T12` both add `#[serde(default)]` fields to `project.rs` and should rebase cleanly when combined.

### `T11` — Pattern Swing, Step Microtiming (`Nudge`) & Ratchet Retriggers
- **Status**: `DONE`
- **Controls**: Pattern **Swing** and step **Nudge** / **Retrig** use the existing `SetPattern` and `SetCell` commands and edit history.
- **Owned Files**: `crates/leviathan-core/src/lib.rs`, `crates/leviathan-core/src/song.rs`, `crates/leviathan-app/src/inspector_ui.rs`, `crates/leviathan-app/src/session_ui.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Add hardware-tracker groove and sub-step articulation—per-pattern **Swing** (`50%`..`75%` delaying odd rows without changing total bar length), per-step **Microtiming Nudge** (`micro_offset: i8` in `-24..=24` sub-ticks, `-50%`..`+50%` of a row), and per-step **Ratchet / Retrigger Count** (`retrig: u8` in `1..=4` evenly spaced gate & voice re-fires across the step).
- **Scope & Constraints**:
  - Preserve `#![no_std]` and zero heap allocations in `leviathan-core`.
  - Default `Pattern.swing` (`0.5`), `NoteCell.micro_offset` (`0`), and `NoteCell.retrig` (`1`) via `#[serde(default)]` so all pre-`T11` `.leviathan.json` files load unchanged, and validate bounds strictly in `project::ProjectFile::into_project`.
  - Evaluate swing, micro-offsets, and intra-step ratchet re-gates inside `SongPlayer` / `PatternEvaluator` so live `cpal` audio, `CvFrame` shader triggers, `SoundLinkPacket` UDP output, and offline `export.rs` WAV bounces all inherit groove and ratchets automatically.
  - Expose pattern `Swing` in `session_ui.rs` and step `Nudge` / `Retrig` sliders in `inspector_ui.rs` with drag-coalesced `EditHistory` undo support.
- **Acceptance Criteria**:
  - `#![no_std]` unit tests in `leviathan-core` verifying bar-invariant swing timing, `1..=4` ratchet gate re-fires within a single step, bounded negative/positive microtiming offsets across row boundaries, and `#[serde(default)]` `.leviathan.json` round-trip in `project.rs`; `0` workspace compiler warnings.

### `T12` — 6-Lane Stereo Pan / Send Mixer & `VarSampler` Trim / Loop Controls
- **Status**: `DONE`
- **Controls**: **Lane Mixer** and sampler **Start**, **End**, and **Sustain Loop** use `AudioCommand::SetLaneChannels` and `SetSampleRegion`.
- **Owned Files**: `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-dsp/src/sampler.rs`, `crates/leviathan-app/src/lane_mix.rs`, new `crates/leviathan-app/src/mixer_ui.rs`, `crates/leviathan-app/src/sampler_ui.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/export.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Upgrade the 6-lane audio rack from fixed mono-summed center gain to a real per-lane stereo mixer (**Level** `0.0..=1.25`, constant-power **Pan** `-1.0..=+1.0`, and per-lane **Delay Send** `0.0..=1.0`), and add per-bank **Start / End Trim** (`0.0..=1.0`) plus **Forward Sustain Loop** to the 4-bank `UQ16.16` `VarSampler`.
- **Scope & Constraints**:
  - Keep `leviathan-dsp` strictly `#![no_std]` and allocation-free, slewing per-lane level/pan/send with one-pole smoothing (`~3–5 ms`) so live mixer tweaks never click.
  - Wrap `VarSampler` `UQ16.16` phase between `trim_start..trim_end` frames when `loop_enabled` is true while the lane gate remains active, clamping safely when `trim_end <= trim_start`.
  - Mirror lane mixer parameters and bank trim/loop metadata across `AudioCommand` (`rtrb`), `AudioProject`, and `export.rs` (so master & stem WAV bounces reflect lane level/pan/send and sampler trim/loops identically), and persist via `#[serde(default)]` in `.leviathan.json` with `is_finite()` range validation.
  - Render trim start/end markers on the 64-bin waveform preview in `sampler_ui.rs`.
- **Acceptance Criteria**:
  - `#![no_std]` unit tests verifying constant-power pan law (`-1.0`, `0.0`, `+1.0`), `VarSampler` trim bounds and sustain loop wrap without buffer overrun, offline `export.rs` stereo pan parity, and legacy `.leviathan.json` `#[serde(default)]` loading; `0` workspace compiler warnings.

### `T13` — Real-Time `@group(1)` Audio Waveform & Band-Envelope Texture (`audio_tex`) for WGSL Shaders
- **Status**: `DONE`
- **Owned Files**: `crates/leviathan-gpu/src/lib.rs`, `crates/leviathan-app/src/gpu_view.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Feed live real-time stereo audio waveform samples and smoothed frequency-band energy envelopes from `RtAudioWorker` into a `256×2` `Rgba8Unorm` GPU texture (`audio_tex`) at `@group(1) @binding(0)` so built-in and custom `.wgsl` shaders can draw true audio oscilloscopes and spectrum-reactive visuals, plus a compact waveform scope toggle in the side panel.
- **Scope & Constraints**:
  - Zero heap allocations on the `cpal` callback: accumulate a fixed `Copy` `256`-sample decimated stereo waveform + 4-band SVF/one-pole magnitude row inside `RtAudioWorker` and ship snapshots over the existing `rtrb` `AudioTelemetry` ring.
  - Bind `audio_tex` (`256×2` `Rgba8Unorm` updated per frame via `queue.write_texture`) at **`@group(1) @binding(0)`** (`PipelineLayout` `[&bgl_group0, &bgl_group1]`) so **100% of existing `T10` `@group(0)`-only custom `.wgsl` shaders continue to compile and run unmodified**, while shaders declaring `@group(1) @binding(0) var audio_tex: texture_2d<f32>;` gain live waveform (`uv.y = 0.25`) and band-spectrum (`uv.y = 0.75`) sampling.
  - Update `STARTER_SHADER_TEMPLATE_WGSL` comments/optional example and keep headless CPU `FeedbackSurface` fallback working when `cc.wgpu_render_state` is `None`.
- **Acceptance Criteria**:
  - Unit tests verifying deterministic `256×2` waveform/band texel packing from synthetic stereo frames, `validate_wgsl_contract` acceptance of both `@group(0)`-only (`T10`) and `@group(0)` + `@group(1)` shaders, and headless `FeedbackViewport` rendering; `0` workspace compiler warnings.

### `T14` — Generative Tracker Tools (Euclidean Fill, Ramp Interpolation, Scale Lock) & Clean-Room `.mid` SMF Import / Export
- **Status**: `DONE` (`9c1e840`, `b8e9ba2`). Euclidean fill, `Ctrl/Cmd+I` velocity/automation ramp, live & selection `ScaleLock`, and clean-room Type-0/Type-1 `.mid` SMF import/export with `0xF8..=0xFE` real-time skip.
- **Owned Files**: `crates/leviathan-app/src/tracker_edit.rs`, `crates/leviathan-app/src/tracker_keys.rs`, new `crates/leviathan-app/src/smf.rs`, new `crates/leviathan-app/src/gen_ui.rs`, `crates/leviathan-app/src/main.rs`
- **Goal**: Add fast tracker composition power-tools (**Euclidean step generator** `E(pulses, steps, rotation)`, **Linear Ramp Interpolation** `Ctrl/Cmd+I` across selected `VEL` or visual automation `Z`/`Roll`/`Fbk` ranges, and **Scale-Lock pitch quantizer**) alongside clean-room Standard MIDI File (`.mid`, SMF Type-0 / Type-1) pattern & chain import/export for DAW interoperability.
- **Scope & Constraints**:
  - Implement SMF (`MThd` / `MTrk` + variable-length quantity `VLQ` delta-times, `NoteOn`/`NoteOff`, tempo meta-event `0xFF 0x51`) in pure clean-room Rust in `smf.rs` with **zero new Cargo dependencies** and bounded input-file size (`<= 2 MiB`) + malformed-header safety.
  - Every generative edit (Euclidean fill, velocity/automation linear interpolation, scale quantize, and `.mid` pattern import) must push a single `EditHistory` snapshot beforehand so `Ctrl+Z` / `Ctrl+Y` cleanly undoes/redoes the entire operation and syncs `AudioCommand::SetPattern`.
- **Acceptance Criteria**:
  - Unit tests verifying Bjorklund Euclidean rhythm distribution (`E(3,8)`, `E(5,16)` with rotation), linear velocity/automation ramp endpoints, scale-quantization mapping, `EditHistory` single-step undo, and lossless SMF `.mid` export -> import round-trip (note pitches, velocities, row positions, and BPM); `0` workspace compiler warnings.
