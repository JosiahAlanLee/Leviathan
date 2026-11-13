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

## Wave 3 Completed Tickets (T11-T14)

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
## Wave 4 Active Backlog (T15–T18 — Dynamic Lane Patching & Clean-Room Synth Pool)

> **Coordination & Sequencing Note**: Land **`T15` first** so `MachineSpec` and per-lane `LanePatch` routing are in place on `main`. Once `T15` merges, **`T16` (`cz.rs`), `T17` (`pvx.rs`), and `T18` (`vast.rs`)** live in separate `#![no_std]` submodules under `crates/leviathan-dsp/src/` and can be picked up in parallel by GPT and Gemini without UI file conflicts. Strictly uphold `CLEANROOM.md` (`BSD-3-Clause`): implement DSP from mathematical/behavioral specifications only — never copy or translate GPL-3.0 VCV Rack / community plugin code or AGPL wrapper modules (`Machinemodule` / `Monomodule`).

### `T15` – Dynamic Per-Lane Instrument Assignment (`LanePatch`) & `#![no_std]` `MachineSpec` Registry
  - **Status**: `DONE` (`17daad7`, `0b04362`, `16e5383`, `83fc175`, `0dfbc12`, `86ea8d0`, `8c45040`, `fb83cca`, `c424aed`). `#![no_std]` `MachineSpec` registry, per-lane `LanePatch` `.leviathan.json` persistence, lock-free `AudioCommand::SetLanePatch`, table-driven UI patch inspector & tracker badges, and dynamic `{lane}-{slug}.wav` stem export.
  - **Owned Files**: `crates/leviathan-core/src/lib.rs`, new `crates/leviathan-dsp/src/spec.rs`, `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/locks_ui.rs`, `crates/leviathan-app/src/mixer_ui.rs`, `crates/leviathan-app/src/export.rs`, `crates/leviathan-app/src/main.rs`
  - **Goal**: Decouple the 6 tracker lanes (`NUM_LANES = 6`) from fixed default machines (`0..5`) so any lane can host any `MachineKind` in the synth pool with its own lane-level base patch (`LanePatch`), while per-step `SoundLock`s layer masked parameter locks or step-level machine swaps on top.
  - **Scope & Constraints**:
    - Add a `#![no_std]` `MachineSpec` table in `crates/leviathan-dsp/src/spec.rs` defining each `MachineKind`'s stable wire slug/ID, 4-char tracker badge, display name, and 8 macro parameter descriptors (`name` + normalized `default` in `0.0..=1.0`) so downstream `T16`–`T18` voices require zero hardcoded UI edits.
    - Introduce `LanePatch { pub machine: MachineKind, pub params: [f32; SOUND_PARAMS] }` per lane, defaulting lanes `0..5` to `[KickDrum, FourOpFm, SubBass, NoiseWash, VarSampler, AcidLead]` via `#[serde(default)]` so all pre-`T15` `.leviathan.json` projects load seamlessly.
    - Resolve voice parameters in `SynthRack` by starting from `lane_patch.params`, overriding slots where `(lock.mask & (1 << i)) != 0`, and selecting `MachineKind` from `lock.machine >= 0` override else `lane_patch.machine`; add lock-free `AudioCommand::SetLanePatch` that resets transient filter/envelope state cleanly on machine change without RT allocations.
    - Add per-lane machine dropdown + 8 base-patch macro sliders in the UI, drive tracker column badges and `export.rs` stem filenames (`stem_{lane}_{slug}.wav`) from `MachineSpec`, and sync `ReplaceProject` / offline WAV export.
  - **Acceptance Criteria**:
    - Unit tests verifying two lanes assigned to the same `MachineKind` (e.g. dual `FourOpFm` or `ChordStab`) render independent voice states and distinct base patches in both real-time `SynthRack` and offline `export.rs`, masked step `SoundLock` overrides preserve unlocked base knobs, pre-`T15` `.leviathan.json` files deserialize cleanly, `#![no_std]` checks pass on `leviathan-core` and `leviathan-dsp`, and workspace builds with `0` compiler warnings.

### `T16` – Clean-Room `CzPhaseDist` Synth Voice (`CZishLab` Phase-Distortion Archetype)
  - **Status**: `IMPLEMENTED` (`494c5fb`). Select `CZPD` in Lane Patch or Step Lock. Lane changes use `SetLanePatch`.
  - **Owned Files**: new `crates/leviathan-dsp/src/cz.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Expand the assignable instrument pool with `MachineKind::CzPhaseDist` (`"CZPD"`), a `#![no_std]` Casio CZ / `uPD933`-style dual-line phase-distortion voice inspired by the clean-room `CZishLab` specification.
  - **Scope & Constraints**:
    - Implement pure mathematical `#![no_std]` phase-warp transfer functions (`Saw`, `Square`, `Pulse`, `DoubleSine`, `SawPulse`, and windowed `Reso1..3` sync-cosine waveforms), dual-line mix/detune + optional ring/noise modulation, and dedicated DCW + DCA envelopes in `cz.rs` with zero external or GPL code.
    - Register 8 `MachineSpec` macros: `Wave A`, `Wave B / Mix`, `DCW Depth`, `DCW Decay`, `Amp Attack`, `Amp Release`, `Ring / Noise`, `Detune`.
    - Zero heap allocations and compact inline per-voice state (`<= 256` bytes per lane).
  - **Acceptance Criteria**:
    - Unit tests in `cz.rs` verifying bounded finite output across all waveform indices and MIDI pitches `0..=127`, harmonic enrichment as `DCW Depth` sweeps `0.0 -> 1.0`, clean behavior under `T11` `retrig` ratchets, `MachineSpec` registration, `--no-default-features` `#![no_std]` compilation, and `0` compiler warnings.

### `T17` – Clean-Room `PolivoksSub` Synth Voice (`PVXLab` Duophonic / Filter-Drive Archetype)
  - **Status**: `IMPLEMENTED` (`494c5fb`). Select `PVX2` in Lane Patch or Step Lock. Lane changes use `SetLanePatch`.
  - **Owned Files**: new `crates/leviathan-dsp/src/pvx.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Add `MachineKind::PolivoksSub` (`"PVX2"`), a `#![no_std]` dual-oscillator subtractive/cross-FM voice with switchable aggressive lowpass/bandpass op-amp filter saturation inspired by the clean-room `PVXLab` specification.
  - **Scope & Constraints**:
    - Implement pure `#![no_std]` PolyBLEP dual oscillators (saw/pulse/triangle + sub/noise blend), Osc2->Osc1 cross-FM & hard-sync amount, nonlinear LP/BP op-amp slew/feedback filter that remains unconditionally finite at full resonance, and punchy filter/amp envelopes in `pvx.rs` with zero GPL code.
    - Register 8 `MachineSpec` macros: `Osc Wave / Mix`, `Detune / Interval`, `Cross-FM / Sync`, `Cutoff`, `Resonance`, `LP/BP & Drive`, `Env Mod`, `Decay`.
    - Zero heap allocations and compact inline per-voice state (`<= 256` bytes per lane).
  - **Acceptance Criteria**:
    - Unit tests in `pvx.rs` verifying unconditional `is_finite()` stability at maximum resonance + cross-FM across MIDI notes `12..=120`, measurable spectral shift between LP and BP modes, `MachineSpec` registration, `--no-default-features` `#![no_std]` compilation, and `0` compiler warnings.

### `T18` – Clean-Room `VastFunSynth` Voice (`Vastish` 3-Block Pipeline & `FUN` Combinator Archetype)
  - **Status**: `IMPLEMENTED` (`494c5fb`). Select `VAST` in Lane Patch or Step Lock. Lane changes use `SetLanePatch`.
  - **Owned Files**: new `crates/leviathan-dsp/src/vast.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Add `MachineKind::VastFunSynth` (`"VAST"`), a `#![no_std]` Kurzweil VAST-inspired 3-block algorithm voice with selectable 2-input `FUN(a, b)` nonlinear modulation combinators drawn from the clean-room `Vastish` behavioral model.
  - **Scope & Constraints**:
    - Implement pure `#![no_std]` 3-stage DSP algorithms chaining Oscillator -> Nonlinear Shaper (`Wrap`, `Fold`, `Quantize`, `AllpassComb` with bounded `<= 256`-sample buffer) -> Multimode SVF, driven by selectable `FUN(env, lfo_or_vel)` equations (`(a+b)/2`, `a*b`, `sin(pi*a)*b`, diode/logistic map, stepped S&H) in `vast.rs` — strictly excluding any AGPL/GPL external code.
    - Register 8 `MachineSpec` macros: `Algorithm`, `Warp / Drive`, `FUN Equation`, `FUN Depth`, `LFO Rate`, `Cutoff`, `Resonance`, `Amp Decay`.
    - Add a compile/unit-test stack-budget assertion `core::mem::size_of::<SynthRack>() < 262_144` so 6 multi-engine `VoiceState` lanes + 64 KB `SampleBank` stay comfortably under Windows' 1 MB thread stack.
  - **Acceptance Criteria**:
    - Unit tests in `vast.rs` verifying distinct bounded non-silent output across algorithms and `FUN` equations, `is_finite()` stability under high fold/comb settings, `size_of::<SynthRack>() < 262_144` assertion passing, `MachineSpec` registration, `--no-default-features` `#![no_std]` compilation, and `0` compiler warnings.

## Wave 5 (T19–T24 — Electro Percussion, West Coast LPG, ByteBus R-2R, Transwave & Spatial Bus)

> **Coordination & Sequencing Note**: Wave 4 (`T15`–`T18`) is merged on `main`. [`crates/leviathan-dsp/src/spec.rs`](crates/leviathan-dsp/src/spec.rs) drives lane dropdowns, tracker header badges, macro sliders, `.leviathan.json` slugs, and WAV stem names automatically. **`T19` (`perc.rs`), `T20` (`west.rs`), `T21` (`bbus.rs`), and `T22` (`otis.rs`)** each live in an isolated `#![no_std]` submodule under `crates/leviathan-dsp/src/`, while **`T23` (`bender.rs` + `master.rs` + mixer/master UI)** expands the shared stereo send bus without touching per-voice synthesis files. Strictly uphold `CLEANROOM.md` (`BSD-3-Clause`): implement all DSP from mathematical/behavioral specifications only — never copy or translate `GPL-3.0` VCV Rack / community plugin code or `AGPL-3.0` wrapper modules (`Machinemodule` / `Monomodule`). Keep `core::mem::size_of::<SynthRack>() < 262_144` across all tickets so Windows CI stays well under its 1 MB thread stack.

| Ticket | Slug / Badge | Module | 8-Macro `MachineSpec` (`P0`..`P7`) | `#![no_std]` Budget |
| :--- | :--- | :--- | :--- | :--- |
| **`T19`** | `perc` / `PERC` | `crates/leviathan-dsp/src/perc.rs` | `P0 Decay`, `P1 Mode`, `P2 BodyTune`, `P3 Snappy`, `P4 MetalSpread`, `P5 BandTone`, `P6 Drive`, `P7 PitchEnv` | `<= 128 B` / lane |
| **`T20`** | `west` / `WEST` | `crates/leviathan-dsp/src/west.rs` | `P0 VactrolDecay`, `P1 FoldDrive`, `P2 Symmetry`, `P3 TzfmDepth`, `P4 ModRatio`, `P5 RingMix`, `P6 LpgMode`, `P7 Strike` | `<= 160 B` / lane |
| **`T21`** | `bbus` / `BBUS` | `crates/leviathan-dsp/src/bbus.rs`, `crates/leviathan-dsp/src/dac.rs` | `P0 Decay`, `P1 BusMask`, `P2 FirTilt`, `P3 R2rSkew`, `P4 ClockHold`, `P5 Cutoff`, `P6 SlewGlitch`, `P7 SubBit` | `<= 96 B` / lane |
| **`T22`** | `otis` / `OTIS` | `crates/leviathan-dsp/src/otis.rs` | `P0 Decay`, `P1 TableSel`, `P2 FramePos`, `P3 ScanDepth`, `P4 K1Cutoff`, `P5 K2Reso`, `P6 FixedGrit`, `P7 LoopMode` | `<= 128 B` / lane (+ static ROM) |
| **`T23`** | *(Bus FX)* | `crates/leviathan-dsp/src/bender.rs` | Shared `SpoonBenderBus`: `ShiftHz`, `Feedback`, `TankDecay`, `Freeze` + per-lane `fx_send` | `<= 64 KiB` shared on `MasterBus` |
| **`T24`** | `otis2` / `OT2` | `crates/leviathan-dsp/src/otis_twin.rs` | `P0 Decay`, `P1 TableSel`, `P2 FramePos`, `P3 ScanDepth`, `P4 FilterF1`, `P5 FilterF2`, `P6 TwinSpread`, `P7 ModeGrit` | `<= 96 B` / lane (+ shared ROM) |

### `T19` – Clean-Room `ElectroPerc` Drum & 6-Oscillator Metallic Percussion Voice (`PERC`)
  - **Status**: `IMPLEMENTED` (`c125d5b`). Select `PERC` in Lane Patch or Step Lock for snare, clap, rimshot, and metallic hat modes.
  - **Owned Files**: new `crates/leviathan-dsp/src/perc.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Add `MachineKind::ElectroPerc` (`slug: "perc"`, `badge: "PERC"`), a dedicated `#![no_std]` analog/electro percussion synthesizer covering snappy snares, multi-tap claps, rimshots, and 6-ratio metallic hi-hats/cymbals without requiring external `.wav` samples.
  - **Scope & Constraints**:
    - Implement four continuous or stepped percussion topologies selected via `P1 Mode`: (0) dual-bridged-T / pitch-swept snare membrane + filtered noise snare wires, (1) 4-burst micro-envelope handclap (`~10 ms` retrigger bursts into resonant bandpass tail), (2) high-Q dual-oscillator rimshot/clave transient, and (3) 6-square-oscillator inharmonic ring/XOR cymbal-hat matrix routed through a steep highpass/bandpass SVF.
    - Register 8 `MachineSpec` macros (`0.0..=1.0`): `P0 Decay` (`0.32`), `P1 Mode` (`0.10`), `P2 BodyTune` (`0.45`), `P3 Snappy` (`0.55`), `P4 MetalSpread` (`0.50`), `P5 BandTone` (`0.58`), `P6 Drive` (`0.30`), `P7 PitchEnv` (`0.40`).
    - Zero heap allocations, zero external crates, and `<= 128` bytes per-voice state; retrigger cleanly on `T11` `retrig` ratchets (`1..=4`).
  - **Acceptance Criteria**:
    - Unit tests in `perc.rs` verifying distinct non-silent bounded waveforms across all 4 `Mode` regions (`Snare`, `Clap` multi-burst envelope peaks, `Rimshot`, `Metallic Hat` high-frequency zero-crossing density), unconditional `is_finite()` stability across MIDI notes `0..=127`, `MachineSpec` slug/badge registration, `--no-default-features` `#![no_std]` compilation, and `0` workspace compiler warnings.

### `T20` – Clean-Room `WestCoastLpg` Complex TZFM + Wavefolder + Vactrol LPG Voice (`WEST`)
  - **Status**: `IMPLEMENTED` (`c125d5b`). Select `WEST` in Lane Patch or Step Lock for through-zero FM, folding, and the vactrol LPG.
  - **Owned Files**: new `crates/leviathan-dsp/src/west.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Add `MachineKind::WestCoastLpg` (`slug: "west"`, `badge: "WEST"`), a `#![no_std]` West Coast dual-oscillator through-zero linear FM, 5-stage wavefolder, ring/AM modulator, and asymmetric vactrol Low-Pass Gate voice.
  - **Scope & Constraints**:
    - Implement modulator + carrier oscillators supporting signed through-zero phase/frequency modulation (`P3 TzfmDepth`, `P4 ModRatio` spanning harmonic ratios and fine detune) and `1005`-style balanced ring/AM blend (`P5 RingMix`), feeding a DC-biased (`P2 Symmetry`) 5-stage sinusoidal/Lockhart-inspired wavefolder (`P1 FoldDrive`).
    - Model a coupled vactrol Low-Pass Gate (`P6 LpgMode`: `Lowpass`, `Combo LP+VCA`, `VCA-only`) with fast strike transient (`P7 Strike` ping) and nonlinear decelerating vactrol photoresistor release tail (`P0 VactrolDecay`) that simultaneously darkens cutoff and attenuates gain without zipper or DC drift.
    - Register 8 `MachineSpec` macros (`0.0..=1.0`): `P0 VactrolDecay` (`0.40`), `P1 FoldDrive` (`0.35`), `P2 Symmetry` (`0.50`), `P3 TzfmDepth` (`0.25`), `P4 ModRatio` (`0.50`), `P5 RingMix` (`0.00`), `P6 LpgMode` (`0.50`), `P7 Strike` (`0.60`).
    - Zero heap allocations and `<= 160` bytes per-voice state.
  - **Acceptance Criteria**:
    - Unit tests in `west.rs` verifying zero-mean/bounded output (`[-1.2, 1.2]`) under extreme `FoldDrive = 1.0` + `TzfmDepth = 1.0`, monotonic harmonic overtone increase as `FoldDrive` sweeps `0.0 -> 1.0`, coupled high-frequency rolloff during vactrol tail decay in `Combo` mode, `--no-default-features` `#![no_std]` compilation, and `0` compiler warnings.

### `T21` – Clean-Room ByteBus FIR and R-2R Voice (`BBUS`)

- **Status** — Implemented (`78c99f1`). Select `BBUS` in Lane Patch or Step Lock. **Fixed ByteBus word** in the row inspector allows an explicit `0x00`.
- **Source and clock** — `ByteBusReceiver` distinguishes standalone, current tracker data, and clocked input. The tracker supplies its current byte at row and gate timing, including changes within one transport clock pulse. A clocked input acquires on rising edges and holds through a stopped clock. Any published byte, including `0x00`, is valid. The deterministic standalone generator takes over when no word is available.
- **Persistence** — `AutomationRow::byte_override` defaults to false for old projects. Legacy zero rows follow the lane gate mask and legacy nonzero rows retain their fixed byte. Live playback and offline WAV use the same routing.
- **Voice** — A 32-bit phase accumulator drives an eight-stage XOR/XNOR register and an eight-tap bipolar FIR. Process every crossed slot, including wrap, at sixteen slots per note cycle. The offset-binary ladder models resistor skew, sample-and-hold, slew, and a decaying major-carry transient. The sub path blends one and two octaves down. All voice state stays within 96 bytes.
- **Macros** — `Decay` 0.38, `BusMask` 0.45, `FirTilt` 0.30, `R2rSkew` 0.20, `ClockHold` 0.15, `Cutoff` 0.60, `SlewGlitch` 0.30, `SubBit` 0.35.
- **Acceptance** — Verify a clocked zero word, held data through clock gaps, tracker changes at 8× speed, multi-slot phase advances, bounded DAC output over all 256 codes, carry decay, legacy project loading, and sample-for-sample live/WAV parity.
- **Converter follow-up** — See T25 for the independent converter profiles. Existing voices retain their current signal paths.

### `T22` – Clean-Room `OtisTranswave` Fixed-Point Wavetable Scanner (`OTIS`, `OtisLab` Archetype)
  - **Status**: `IMPLEMENTED` (`c125d5b`). Select `OTIS` in Lane Patch or Step Lock for four generated transwave banks and three scan modes.
  - **Owned Files**: new `crates/leviathan-dsp/src/otis.rs`, plus enum/table registration in `crates/leviathan-dsp/src/spec.rs` & `crates/leviathan-dsp/src/lib.rs`
  - **Goal**: Add `MachineKind::OtisTranswave` (`slug: "otis"`, `badge: "OTIS"`), a `#![no_std]` `ES5505` / Ensoniq-inspired fixed-point (`Q20.12`) transwave frame-scanning synthesizer with dual-pole `K1`/`K2` digital filtering drawn from the `BSD-3-Clause`-clean `OtisLab` behavioral model.
  - **Scope & Constraints**:
    - Generate or store compact procedural/constexpr single-cycle transwave banks (4 tables × 16 frames × 128 `i16` samples = `16 KiB` static/shared ROM, **not** duplicated per `VoiceState`) covering formant sweep, metallic bell partials, pulse-width cascade, and digital sync harmonics.
    - Drive phase accumulation using `u32` `Q20.12` fixed-point fractional addressing with configurable sample-rate grit (`P6 FixedGrit`), smooth bilinear interpolation across adjacent wavetable frames (`P2 FramePos` modulated by envelope/LFO `P3 ScanDepth` in `Fwd`, `Bidir`, or `OneShot` `P7 LoopMode`), and cascade `ES5505`-inspired `K1`/`K2` digital filter poles (`P4 K1Cutoff`, `P5 K2Reso`).
    - Register 8 `MachineSpec` macros (`0.0..=1.0`): `P0 Decay` (`0.48`), `P1 TableSel` (`0.00`), `P2 FramePos` (`0.20`), `P3 ScanDepth` (`0.55`), `P4 K1Cutoff` (`0.65`), `P5 K2Reso` (`0.30`), `P6 FixedGrit` (`0.25`), `P7 LoopMode` (`0.50`).
  - **Acceptance Criteria**:
    - Unit tests in `otis.rs` verifying smooth artifact-free frame interpolation across all 4 wavetable banks, `Q20.12` phase wraparound accuracy over 100,000+ samples without drift or overflow, `core::mem::size_of::<SynthRack>() < 262_144`, `--no-default-features` `#![no_std]` compilation, and `0` compiler warnings.

### `T23` – Shared `SpoonBenderBus` Stereo Frequency-Shifter, Freeze & Diffused FDN Send (`BEND`)
  - **Status**: `IMPLEMENTED` (`c125d5b`). `SetBenderParams` updates the shared BEND bus. Stem export writes its return to `bend.wav` when a lane has a send. Offline renders start with an empty tank.
  - **Owned Files**: new `crates/leviathan-dsp/src/bender.rs`, `crates/leviathan-dsp/src/master.rs`, `crates/leviathan-dsp/src/lib.rs`, `crates/leviathan-app/src/lane_mix.rs`, `crates/leviathan-app/src/mixer_ui.rs`, `crates/leviathan-app/src/master_ui.rs`, `crates/leviathan-app/src/project.rs`, `crates/leviathan-app/src/audio.rs`, `crates/leviathan-app/src/export.rs`, `crates/leviathan-app/src/main.rs`
  - **Goal**: Bring the spatial character of `Spoon Bender` and `Séance` into Leviathan as a shared `#![no_std]` stereo auxiliary send bus (`fx_send` per lane -> Hilbert dome frequency shifter + diffused 4-line Hadamard FDN tank with infinite `Freeze` toggle) on `MasterBus` so all lanes share one compact fixed-point/float delay pool.
  - **Scope & Constraints**:
    - Implement a `#![no_std]` stereo dome allpass Hilbert network (6th-order IIR 90-degree quadrature pair + quadrature sine/cosine ring oscillator for clean single-sideband up/down Hz shifting `-200 Hz..+200 Hz`) feeding a 4-line Hadamard feedback delay network (`<= 64 KiB` total static delay memory inside `MasterBus`, keeping `size_of::<SynthRack>() < 262_144`) with loop damping and clickless `Freeze` recirculation (`gain = 1.0`, input muted).
    - Extend `LaneChannel` with `#[serde(default)] pub fx_send: f32` (`0.0` default so pre-`T23` `.leviathan.json` files load unchanged) and add `BenderParams { shift_norm: f32, feedback: f32, decay: f32, freeze: bool }` persisted in `Project` (`#[serde(default)]`), synced lock-free via `AudioCommand::SetBenderParams`, rendered identically in offline `export.rs` (including wet stem option), and exposed in `mixer_ui.rs` + `master_ui.rs`.
  - **Acceptance Criteria**:
    - Unit tests verifying single-sideband Hilbert frequency shift shifts a `440 Hz` sine upward/downward cleanly, `Freeze` sustains bounded finite energy (`[-1.0, 1.0]`) over 5+ seconds without runaway blowup, pre-`T23` `.leviathan.json` projects deserialize with `fx_send == 0.0` and neutral bus defaults, `size_of::<SynthRack>() < 262_144`, `--no-default-features` `#![no_std]` check passes, and workspace builds with `0` warnings.

### `T24` – OTIS Twin Transwaves and Series Filters

- **Status** — `IMPLEMENTED` (`bc0c65c`). Select `OT2` in Lane Patch or Step Lock. Existing `otis` patches and numeric step locks at ID `12` retain the T22 sound and macro meanings.
- **Twin transwaves** — Use two independent phase accumulators with Q20.12 addressing and bilinear frame/phase interpolation. Oscillator B counter-scans the selected bank and offers detuned unison, fifth, and octave ranges. Provide a true single-reader setting and smooth the blend. Generate a shared 16 KiB ROM with formant, bell, PWM/sync, and sub-fold banks. Use generated waveforms and behavioral references.
- **Filters** — Use F1 for the low-pass ceiling with asymmetric coefficient slew. F2 below center introduces a Fizmo-inspired resonant four-pole low-pass, center uses a neutral three-pole low-pass, and above center adds a high-pass floor. The prototype's three-pole LP plus two-pole HP is a Leviathan voicing. The ASR-10 manual documents four-pole splits of 3+1 or 2+2 for hardware comparisons.
- **Macros** — `Decay` 0.48, `TableSel` 0.00, `FramePos` 0.20, `ScanDepth` 0.55, `FilterF1` 0.68, `FilterF2` 0.35, `TwinSpread` 0.42, `ModeGrit` 0.45. `ModeGrit` selects OneShot, Bidir, or FwdWrap with a grit range within each zone.
- **Acceptance** — Verify that the two frame readers travel in opposite directions in Bidir mode and preserve phase accuracy over 128,000 samples. Measure the resonant and bass-cut responses. Check wrap and mode transitions for discontinuities, keep per-lane state within 96 bytes and `SynthRack` under 262,144 bytes, and verify existing OTIS project and WAV behavior.
- **Behavioral references** — [FIZMO User’s Guide](https://www.vintagesynth.com/sites/default/files/2017-05/FIZMO_Users_Guide.pdf) and [ASR-10 Musician’s Manual](https://ensoniq.manymanuals.com/digital-pianos/asr-10/instruction-manual-11191/208).

## Wave 6 (T25–T26 — Converter Profiles and Formant FM)

### `T25` – Converter Word Formats and DAC Coloration

- **Status** — `IMPLEMENTED` (`ff633b4`). The shared `leviathan_dsp::dac` stage has no tracker controls or new audio commands.
- **Word formats** — The YM3012 serial codec uses a 10-bit offset-binary mantissa and exponent codes 1–7. It reconstructs signed integer PCM for audio processing. The linear profile truncates signed PCM to 4–16 bits. The 8-bit R-2R profile reuses the ladder transfer. BBUS keeps its existing ladder timing, slew, and carry transient.
- **Provisional CZ** — The 40 kHz profile has selectable instantaneous or syllabic range tracking and an optional range-gain mismatch. The companding law and gain mismatch are modeling choices pending hardware measurements.
- **Clock and filters** — The YM profile defaults to the YM2151/OPM example rate of approximately 55.93 kHz. At lower host rates it updates once per host sample. A one-pole input prefilter attenuates some content that would fold at a reduced latch rate. An independent output filter smooths the held steps.
- **Acceptance** — Verify serial fields at exponent boundaries, signed round trips across all 65,536 PCM values, linear bit-depth vectors, CZ range tracking, and independent input and output filter behavior. Keep the inline state within 32 bytes.
- **Behavioral references** — [YM3012 datasheet](https://bitsavers.org/components/yamaha/YM3012_199204.pdf), [YM2151 application manual](https://map.grauw.nl/resources/sound/yamaha_ym2151_synthesis.pdf), and [DAC Stage behavioral specification](https://github.com/JosiahAlanLee/DSP_Emulation/blob/main/docs/DAC_STAGE_SPEC.md).
