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
| Pause / resume transport | `Space` |
| Play edited pattern from the cursor row | `Shift+Space` |
| Context deck | `F1` Grid · `F2` Voice / Trig · `F3` Mixer · `F4` Routing · `F5` Samples · `F6` Scene · `F7` Song |
| Save / Save As / Open project | `Ctrl+S` / `Ctrl+Shift+S` / `Ctrl+O` (`Cmd` on macOS) |
| Navigate cursor | `Arrows`, `PgUp`/`PgDn` (±16 rows), `Home`/`End`, `Tab` / `Shift+Tab` |
| 2+ octave piano note entry | `Z S X D C V G B H N J M` (lower) & `Q 2 W 3 E R 5 T 6 Y 7 U I 9 0 O P` (upper) |
| Base octave / edit step | `-`/`=` (octave), `[`/`]` (step `0..=16`) |
| Hex velocity (`00`..`7F`) & `AIR` | `0`..`9` / `A`..`F` on `VEL` column; `` ` `` inserts `AIR` trigger |
| Rectangular block selection | `Shift + Arrows`, `Shift + PgUp/PgDn/Home/End`, `Shift + Click`, `Ctrl+A` (lane -> pattern -> view), `Esc` |
| Undo / Redo (64 steps) | `Ctrl+Z` / `Ctrl+Y` or `Ctrl+Shift+Z` (`Cmd` on macOS) |
| Copy / Cut / Paste block | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` |
| Clear / Wipe locks | `Delete` (clear notes) / `Shift+Delete` (clear notes + `SoundLock`s) |
| Semitone / Octave transpose | `Ctrl + Up/Down` (±1 semitone) / `Ctrl + Shift + Up/Down` (±12 semitones) |
| Per-lane Mute / Solo / Clear all | `Alt + 1..6` (mute), `Alt + Shift + 1..6` (solo), `Alt + Shift + A` (unmute/unsolo all) |

## Projects, Pattern Chains & Offline WAV Export

- **Save** (`Ctrl/Cmd+S`) stores the pattern bank, chain, tempo, mixer, sound locks, visual automation, custom WGSL shader, and sampler banks with their trim and loop settings in a single `.leviathan.json` file. **Save As** (`Ctrl/Cmd+Shift+S`) chooses a new path, and **Open** (`Ctrl/Cmd+O`) restores the session with playback paused.
- **New Pattern** and **Clone** build a bank of up to 16 patterns with independent playback rate multipliers (`1/4x` to `8x`). In **Song** (F7), use **Add** to append the edited pattern to the 64-entry chain. **Cue Chain** selects the chain while paused. **Queue Chain** launches it at the next pattern boundary while playing. **Follow playback** is off by default.
- **Swing** delays odd rows from 50% to 75% of each two-row pair while preserving pattern length. In **Voice / Trig** (F2), choose **Step Lock**. **Nudge** moves a hit by up to 24 ticks either way (48 ticks per row), and **Retrig** divides the step into up to four evenly spaced hits. Nudges stay within the pattern, with later hits taking over overlapping notes on the same lane.
- Select **PERC**, **WEST**, or **OTIS** in **Lane Patch** to play the electro percussion, West Coast LPG, or transwave scanner voice. Each also supports per-step machine and parameter locks.
- Select **OT2** in **Lane Patch** or **Step Lock** for counter-scanning transwaves. **TwinSpread** starts with one reader and blends in detuned unison, a fifth, or an octave as you turn it up. **FilterF2** adds resonance below center and raises the high-pass floor above it.
- Select **FS77** for formant FM. Lock **FormantHz** on individual steps to sequence vowels. Turn up **WaveRcm** to feed an OTIS waveform into the FM, or **SkirtUnv** to widen the formants and add breath. **ResoDac** moves from a resonant low-pass with provisional CZ coloration to a vocal band-pass with YM3012 coloration.
- Select **K26** for three-stage VAST synthesis. **SourceKey** selects detuned saws, hard sync, drawbars, or OTIS wavetables. **PreTilt** filters the source before **TripleAlg** shapes it. **PostReso** adds resonance below center or phaser notches above it.
- **Voice calibration** enables the revised voice gains and detune response for the current project. New sessions start with it on and the calibrated factory patches. Existing sessions open with their original voicing unless calibration was saved on. Selecting a machine or using **Reset** in **Lane Patch** loads the factory patch for the active setting. The calibrated SMPL patch selects bank B1 and a full window. Switching calibration restarts active voices. Calibrated kicks also restart their oscillator on every hit, so each hit has the same attack and polarity.
- Select **BBUS** for the bit-stream FIR and R-2R voice. **Fixed ByteBus word** in the **Scene** deck accepts all 256 values, including `00`. With it off, playback uses the lane gate mask. Auditioning while stopped uses the standalone generator.
- Open **Mixer** (F3) for each lane's level, constant-power pan, and delay and BEND sends. The master delay send raises all lanes' delay send together. **M** and **S** in the lane headers control mute and solo. The shared **BEND** panel controls frequency shift, feedback, decay, and Freeze. Freeze holds the current feedback tail while muting new input to the bus.
- **Delay mode** is saved with the project and used by live playback and WAV export. **Slapback** uses the original 17 / 21 ms taps. **Long Ping-Pong** uses fixed 125 / 166.7 ms taps through 96 kHz. Switching modes clears the delay tail.
- In **Samples** (F5) under **VarSampler Banks**, **Start** and **End** mark the active sample region. **Sustain Loop** repeats that region while the lane gate is high and finishes the current pass on release. With sustain looping off, the step's loop and scan locks work within the trim.
- In **Voice / Trig** (F2), **Use note gates** gives KICK, FM4, SUB, AIR, ACID and CHRD a short release at gate-off. Consecutive 100% gate steps get separate attacks. Select **Lane … one-shot** for the full decay on that lane. New projects enable note gates. Existing projects open with their saved behavior and can enable the control in F2.
- **Export WAV...** bounces the selected pattern or full chain with the same timing and mixer as live playback. Optional aligned stereo stems include each lane's delay contribution and a shared `bend.wav` return when a lane has a BEND send. These taps precede master DC blocking and limiting. Export starts with an empty tank. With Freeze enabled, the first BEND send starts a 25 ms capture ramp and the tank then holds that sound.

## Capture, Sample Processing & ByteBus

- In **Samples** (F5), open **Capture sample** to record a lane after mute, level and pan, or the BEND return before the master limiter. Choose a duration and click **Arm capture**. Recording starts on a transport beat and fills one stereo project slot of up to 192,000 frames (four seconds at 48 kHz).
- In **Routing** (F4), set the selected lane to **Capture sample**. **SMPL** plays the take at its original pitch and rate on C4. **Start**, **End**, and **Sustain loop** set the playback region. A sounding note retains its take and region when a new capture becomes available.
- **OTIS** and **OT2** treat the trimmed region as fixed-length frames. Set **samples per frame**, then scan them with **FramePos** or enable **Clock OTIS frames from ByteBus**. **VAST** takes a gated capture sample or a continuous signal from another lane's dry mixer tap. These processors fold stereo input to mono.
- **Register → Lookup → BBUS** clocks the tracker word through a shared register at ByteBus clock edges. Set the reset seed and choose Identity, Invert, Square or Fold. The clocked output can also select OTIS frames independently of BBUS. Captures and routing save with the project and render in WAV and stem exports.

In **Voice / Trig** (F2), **Lane Patch** shows timbre options for the lane's base machine. **Step Lock** shows the options for the machine the selected step plays. Project and lane controls show their scope next to the name.

## BBUS cycle lock

- New sessions restart the **BBUS** register every note cycle (**BBUS cycle lock**, in **Voice / Trig** when the lane plays BBUS). Its sixteen steps are then one period, as when ByteBusLab Parallel Sum's cycle is the pitch, so the voice is pitched and harmonic.
- **Lane grit** lets one lane's register run free across note cycles: the original sound, whose feedback repeats only every 15–255 cycles and reads as a noisy, digital comb around the pitch. Projects saved before the option load with the lock off, so they sound as before.

## Oversampled FM and Acid

- **Oversample FM and Acid** (in **Voice / Trig** on FS77, 4-Op FM and Acid lanes) runs FS77 and the 4-Op FM operators at four times the output rate through a 96-tap decimator and band-limits Acid's saw (polyBLEP), so high notes and deep modulation no longer fold back as low, inharmonic tones. **Lane grit** keeps a lane at the original single rate. New sessions turn it on; projects saved earlier keep the original rendering. Six oversampled FS77 lanes take about a fifth of one core.

## Sample-rate independence

- Voices tuned with per-sample filter coefficients, decays, delays and noise at 48 kHz convert them for the device rate. OTIS and OTIS Twin scale the whole grit update period and carry the fractional remainder to keep its average rate. Settings that update every sample use the host clock.

## OTIS warm tables

- **OTIS warm tables** (in **Voice / Trig** when the lane plays OTIS) adds a darker twin of each transwave table. **TableSel** then alternates bright and warm: the lower half of each quarter plays the original table, the upper half its twin (formant swept lower, partials rolled off above the fourth harmonic, peak-matched). New sessions turn it on; projects saved earlier keep the four bright tables.

## Wave 8 Echo, OtisKit and REWORK

- In **Mixer** (F3), set **Echo mix** to send through Echo before Space. Set **Space mix** to zero for Echo alone. **Transport** follows quarter-notes and **ByteBus** measures clock intervals. At 120 BPM, 1× is 500 ms on Transport and 125 ms on the default ByteBus. The panel shows the effective delay and any ESP limit. **LOOP** accumulates the selected Hz shift across repeats. **POST** shifts each outgoing repeat once. Existing projects open with Echo and ESP off.
- Select **OKIT** in **Lane Patch** or in a **Step Lock**. Go to **Samples** (F5) and open **OtisKit**. The **Factory kit** button fills all eight slots. You can also load a WAV, load a folder in filename order, or arm a capture directly into the selected slot. Capture from the **Capture and Banks** tab still writes to the stereo take.
- **Tracker** addressing maps MIDI 60–67 (C-4 through G-4) to slots 1–8 at unity pitch. Each additional group of eight notes transposes an octave. **ByteBus** addressing takes bits 0–2 from Register/Lookup and uses the tracker note for pitch. An optional slot offset wraps around the bank. Kit controls and step offsets are latched on each hit.
- Each lane picks a **velocity curve**. **Chip** (the default, as on the hardware) drops 0.375 dB per step, so velocity 94 is −18.75 dB and 52 is about −44 dB. **Soft** drops 0.125 dB per step, about 24 dB in all, so ordinary tracker dynamics stay audible. The step inspector and velocity tooltips show the gain on the lane's curve. Projects saved earlier load on Chip.
- **Raw output** and **Squared filter taper** (per kit lane). Clean output uses a 32-tap band-limited converter between the chip and output rates. It suppresses images when upsampling and filters before downsampling. Raw output keeps the original stair-step grit. The squared taper rolls off higher frequencies sooner than the linear K1 taper. Kits saved before these options load with both on.
- **REWORK** previews the capture's trim region or an OtisKit slot from Start to its end. Set the seed, fineness and amount, listen to A and B, and commit the result. A replacement or project reload invalidates a stale preview. Export includes committed audio. Preview audition plays from the monitor and stays outside the recorded and exported signal.
- Projects save as version 2 in the same `.leviathan.json` file. The new version stores the stereo take and mono kit slots as base64 little-endian signed 16-bit PCM with explicit rate and frame count. Version 1 files with numeric PCM still load.

## Clocked shader return

1. Put **OT2** on a lane as its patch or a step lock. In **Scene** (F6), select that lane under **Shader return to OT2**, then play. The return controls **FramePos** for both the built-in tables and captured frames.
2. Turn **Probes** to rotate eight numbered points on the feedback surface, and adjust **Threshold** until their bits respond. The filled markers show the last latched byte. **Routing** (F4) shows the input and held return for the destination lane. Custom WGSL uses the existing `byte_mask` uniform, and the CPU fallback uses the built-in field.
3. On each ByteBus edge, the tracker word goes through the **Lookup** selected in F4. Enable **Return → Lookup** to feed the held byte back through that lookup. The route starts from the lookup reset word in feedback mode. **Clear field** clears the visual history and releases the held frame address on the next edge.
4. **Record return from start** restarts the currently cued pattern or chain and records the word the audio engine holds on each pulse. **Keep take** or **Pause** finishes the capture. Save the project, then choose the corresponding pattern or chain in **WAV export**. A recorded take applies to its original destination lane, and the export holds its last setting past the recorded pulses.

The audio clock latches a probe result only on the next ByteBus edge, including when the transport follows MIDI clock. A late or missing result holds the last word, with the missed count shown in Scene. An on-time result takes one sixteenth note at the default four pulses per beat, or 125 ms at 120 BPM. The GPU copies eight texels asynchronously from its feedback target. WAV and stem export use the recorded clocked values.

### Replay the return in Rack

After keeping a take, choose its pattern or chain in **Export WAV…** and enable **Include shader ByteBus CV**. Enter a new folder path. The folder contains `master.wav`, `shader-bytebus.wav`, and any requested audio stems. The CV file uses ten discrete 32-bit float channels at the same sample rate and frame count as the master.

| WAV channel | Signal |
| --- | --- |
| 1–8 | Held word, least significant bit first |
| 9 | One-sample clock strobe on each pulse with a valid word, including `00` and repeats |
| 10 | One-sample reset strobe at the start and when the held word clears |

Play the file at its native sample rate with unity gain in a host that exposes its individual channels. For a software Rack patch, map `0.0` and `1.0` to `0 V` and `10 V`, combine channels 1–9 in order, and send the result to a clocked ByteBus receiver. Route channel 10 separately to the receiver's reset logic. If both strobes appear on the first sample, apply reset before latching the word. The clock continues across chain wraps and stays low during the tail while the data bits hold. [Rack ByteBus cable contract](https://github.com/JosiahAlanLee/Gorgon/blob/22b8d4f0d61e5955deae42f726d57a763b0d8968/spec/bytebus-output.md)

### Send the return live

Set `LEVIATHAN_SHADER_CV_OUT` to the exact name of an output device with at least twelve 32-bit float channels. From a macOS or Linux shell, use the loopback device's name in the launch command.

```bash
LEVIATHAN_SHADER_CV_OUT="My 16-channel loopback" cargo run -p leviathan-app --release
```

The transport strip shows the selected device, sample rate, and **Shader CV 3–12** when the stream opens. If the name is unavailable, the strip lists a few of the output names Leviathan found. Select a return lane in **Scene** (F6) and play.

The CV stream opens at the device's default rate. Set `LEVIATHAN_SHADER_CV_RATE` (in Hz) to open it at the rate the audio server actually runs, so the bits and one-sample strobes are not resampled. If the device cannot run at that rate, the strip says so and no stream opens.

On Linux, ALSA does not list PipeWire sinks by name and repeats many hardware names, so use `PipeWire Sound Server` as the device. It plays to the current default sink, so first make your multichannel loopback (or a twelve-channel null sink) the default output, never a physical interface you do not mean to drive. PipeWire's ALSA device reports 44.1 kHz; read the graph rate with `pw-metadata -n settings | grep clock.rate` and pass it:

```bash
pactl load-module module-null-sink sink_name=leviathan-cv channels=12 format=float32le rate=48000
pactl set-default-sink leviathan-cv
LEVIATHAN_SHADER_CV_OUT="PipeWire Sound Server" LEVIATHAN_SHADER_CV_RATE=48000 cargo run -p leviathan-app --release
```

| Device channel | Signal |
| --- | --- |
| 1–2 | Stereo master |
| 3–10 | Held shader return, least significant bit first |
| 11 | One-sample publication clock |
| 12 | Separate one-sample reset |

For a Rack loopback, keep the device and Rack at the same sample rate and preserve channel order at unity gain. Combine device channels 3–11 in order into a clocked ByteBus input with rails of `0 V` and `10 V`. Route channel 12 separately. A pause holds the byte with the clock low. Reset or a route change clears the held word. **Clear field** releases it on the next clock edge and emits reset if a word was held. Physical CV playback depends on the interface's DC response and received pulse width.

## Custom WGSL Shaders & SoundLink UDP

- Drop any `.wgsl` file onto the window (or use **Load .wgsl** in **Scene** (F6)) to run custom ping-pong fragment shaders driven by Leviathan's 64-byte `VisualUniforms` and `prev_tex` feedback buffer.
- **Export Starter .wgsl** writes a documented template matching the `@group(0)` ABI, and saving edits on disk automatically hot-reloads the shader while keeping the last valid GPU pipeline running if validation fails (**Reset Built-In Shader** restores the default Möbius/PDE shader).
- Toggle **LINK** in the telemetry strip to stream non-blocking `336`-byte `SoundLinkPacket` frames (`127.0.0.1:9477` default) to external visualizers or modular hosts.

## MIDI Input

Select an input in the **MIDI** menu (**Refresh MIDI** rescans ports; **Off** disconnects). Holding MIDI notes auditions the active lane's instrument and cursor sound locks in real time, **MIDI REC** step-records incoming notes and velocities at the tracker cursor with full `Ctrl+Z` undo support, and MIDI `Start` / `Continue` / `Stop` messages synchronize transport playback.
- Incoming MIDI plays on its exact sample, one audio block after it arrives: a steady latency instead of up to a block of jitter. Leviathan asks the device for 256-frame blocks (about 5 ms at 48 kHz) and falls back to the device default.
- **Follow MIDI clock** (on by default) takes tempo from an incoming clock and stays phase-locked to the ticks counted since Start or Continue, so the transport does not drift against outboard gear. While a clock is running, Start and Continue wait for the next tick. The menu shows the clock's tempo.

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

- [UI specification](docs/UI_SPEC.md) documents the desktop workflow and follow-up prototypes.
- [ROADMAP.md](ROADMAP.md) — completed milestones (`M1`..`M9`) and completed Wave 1–2 workstreams (`T1`..`T10`) with owned files and acceptance criteria.
- [AGENTS.md](AGENTS.md) — architectural invariants, `#![no_std]` / lock-free RT audio rules, and pinned crate API notes (`eframe 0.31`, `cpal 0.17`, `midir 0.11`) for coordinating work across coding assistants.

## Clean-Room Provenance

See [CLEANROOM.md](CLEANROOM.md). Leviathan implements the public `BSD-3-Clause` normative specifications from `Gorgon/spec` without incorporating `AGPL-3.0` or `GPL-3.0` source code.
