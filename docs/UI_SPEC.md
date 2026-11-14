# Leviathan UI Specification

**Status** · v0.2, November 14, 2026

**Provenance** · Claude drafted v0.1 from Josiah and Gemini's design session. This revision checks the proposal against the Wave 7 implementation and records the first desktop workflow pass. [Wave 7 baseline](https://github.com/JosiahAlanLee/Leviathan/commit/1641232e44256feed0bbd6bd20cde543163b2aad)

## Desktop workflow

Leviathan targets one laptop screen with integrated graphics for songwriting and performance, with an optional external projector. The audio callback owns transport while audio is available. The UI and feedback surface consume telemetry without blocking the callback. Leviathan runs independently of Rack.

The tracker remains visible while working in a context deck. The deck and the Voice / Trig subtab stay selected when the cursor moves. The top controls stay in view while either the grid or a deck scrolls.

| Key | Deck | Controls in this pass |
|---|---|---|
| F1 | Grid | Closes the side pane |
| F2 | Voice / Trig | **Lane Patch** edits the lane's base machine and macros. **Step Lock** shows the effective machine, inherited values and explicit overrides, probability, gate, FILL, nudge and retrigs |
| F3 | Mixer | Six lane levels, pans and sends, master delay send and mode, BEND shift, feedback, decay and Freeze |
| F4 | Routing | Input for the selected lane, take trim and looping, transwave frame size and ByteBus clocking, Register and Lookup |
| F5 | Samples | Beat-synced lane or BEND capture, four VarSampler banks and their waveform, WAV load and trim |
| F6 | Scene | Live feedback, existing WGSL load and hot reload, audio scope and row automation for Z, Roll, Feedback and ByteBus |
| F7 | Song | Edit, cue or queue the existing chain. Shows the active entry, each pattern's duration, and its starting beat in the chain |

The telemetry strip shows the edit pattern, transport pattern, row and beat, active chain entry and queued launch, stereo peaks and clipping, the tracker ByteBus word and clock, the held Register/Lookup word when available, the SoundLink controls, capture status and the feedback thumbnail. Clicking the thumbnail opens Scene. Compile jobs, bridge offset and MIDI controller feedback can appear here once the corresponding features exist.

| Control | Behavior |
|---|---|
| **Space** | Pause or continue at the transport position. Cursor and pattern selection can be edited during a pause |
| **Play from cursor** / **Shift+Space** | Audition the edit pattern from the cursor's swing-adjusted row. Rows beyond the active length start at its final row. Starts a fresh pass with the current FILL state and clears any queued launch. Chain entries stay saved, and the transport leaves chain mode. The normal evaluator includes any gate active at the target instant, including overlapping nudged notes |
| **Cue Pattern** / **Cue Chain** | Select a playback target while paused. Press Play to start it |
| **Queue Pattern** / **Queue Chain** | Schedule a launch at the next pattern boundary while running |
| **Follow playback** | Off by default. When enabled, shows the playing pattern and keeps the playhead visible. The lane and row of the edit cursor and the open deck stay put. Manual pattern selection turns follow off. Follow yields during cursor input, pointer edits, popups and text entry |
| **-** / **=**, **[** / **]** | Octave and edit step. Deck keys replace the old F1–F4 aliases |

The app uses six lanes, up to 64 rows per pattern, a 16-pattern bank, and a 64-entry chain. Each entry references a pattern. A chain entry's duration is its active rows divided by its rate in rows per beat, including with swing. Feedback submits at most one pass per 60 Hz slot and skips missed slots. This caps feedback updates independently of redraw frequency. Display presentation still depends on the window backend.

## Wave 8 controls

The [Wave 8 plan](https://github.com/JosiahAlanLee/Leviathan/pull/22) owns the DSP, sample storage and real-time contracts for T32–T35. Confirm the UI against working Echo and OtisKit sessions after those controls exist.

| Deck | Proposed workflow | Acceptance |
|---|---|---|
| Mixer | Show `Input → Echo → Space → Return`. Put source, ratio and effective echo time together, including when a time cap applies. Use **LOOP** for shifting inside feedback and **POST** for shifting each outgoing echo once. Keep Space Freeze, ESP mode and rates close to the stages they affect | Compare `1×` on transport and default ByteBus at 120 BPM (500 ms and 125 ms). Check the clamp, Freeze, BEND capture and exported return |
| Routing | Show the existing single-source selection per destination as a patch matrix once it fits the pane. Use separate sections for audio and clocked ByteBus. Mark unavailable routes and explain a rejected cycle | Trace a take into SMPL, OTIS, OT2 and VAST, plus a lane into VAST. Trace the raw tracker word through Register and Lookup to BBUS or the frame clock. Add pins as their signal paths land |
| Samples | Group banks, the captured take and kit slots. Show source, current target, trim/loop, transwave frame size and the take or slot currently assigned. Use an eight-row kit editor for Tune, Decay, Level, Filter, Start and Reverse. Preview REWORK dry/wet before applying | Capture to a named target, audition and replace it, and observe armed, recording and processing states. Check that a stale REWORK result leaves the newer sample intact and that save/reopen restores the committed PCM |

Prototype a “Slice take to Kit” action after slot addressing and region ownership are defined. Derive waveforms from peaks cached per sample generation, and add a frame-grid overlay for OTIS/OT2 where it helps select transwave regions. Keep the full-resolution PCM on the host side of the publication boundary.

Before the Wave 8 sample editors depend on project storage, follow the roadmap's compact, versioned PCM representation and decoded-frame budgets for a full kit plus a stereo take. Keep loading v1 numeric PCM with source rates, trims, routing, chain, lane and step patches and custom WGSL. The v1 file cap is 8 MiB. Measure the largest supported combination by serializing, loading and exporting it. A 128-pattern × 128-row expansion of the *Trace Current* fixture alone takes about 21 MiB as expanded v1 JSON. Set a storage and migration budget before expanding the grid.

## Grid and arrangement prototypes

Test the current desktop workflow at 1280×840 and 1024×720 using *Trace Current* and *Water Table*. Run from opening a project through selecting and playing the chain, tweaking a machine lock, arming a take, checking its route and exporting stems. Use the same sequence with OtisKit, REWORK and BEND Echo after Wave 8. Record which tasks lose focus, force scrolling or obscure the current pattern or sample target. Evaluate keyboard focus on Linux, macOS and Windows.

| Proposal | Decision to validate |
|---|---|
| Optional **Cond** and **Fx** columns | Show existing probability, FILL, nudge, ratchet, machine and lock information in the grid first. Keep all six Note/Velocity pairs visible at laptop width by hiding wider columns. Skip hidden columns in keyboard traversal and define how column-specific copy and paste works. Use a documented priority for stacked tokens, or persist an ordering if last-edited display is wanted |
| Bar gutter and folded rows | Define meter, displayed row grouping and true musical bar boundaries. At `2×`, 16 rows are two quarter-note beats. Preserve measured beat durations across playback, MIDI export and WAV export |
| Additional conditions and seek | Currently `NoteCell` carries probability, FILL, nudge and 1–4 retrigs. Specify pass counting and reset for `A:B` and `1ST`, and event ordering for `NEI` across lanes, nudges, retrigs and seeks, before assigning tokens and keys |
| Pattern-wide automation column | `Bxx` can represent both explicit `B00` and absent override, using the existing field plus flag. Preserve legacy nonzero words even without that flag. Future scene slots and `V0..V3` can live alongside the three legacy visual fields. Keep old shader uniforms valid |
| Unit-aware macros | Keep normalized patch and lock values and stable parameter IDs. Map display units to actual DSP behavior, including ranges, scaling and discrete zones. FS77 `FormantHz` selects linked formant frequencies. Show the mapped range and vowel rather than only knob position |
| Expanded Song view and performance mode (F8) | Prototype repeats, scene cues, loop markers and entry-start mute baselines. Define how live overrides are cleared and what seek and export replay. Prototype scene pads and lane mutes in performance mode with note entry disabled |
| Undo | Current undo is per-pattern and covers the grid and row edits. Define coalescing and the undo boundaries for mixer, routing, chain, sample swaps and other project edits before moving to a project-wide history |

Use logical shortcuts for app commands and validate physical key positions for piano entry separately. Check current solo shortcuts against Windows language switching. Verify modifier down/up and window focus behavior before choosing a momentary FILL key. MIDI Learn can follow a saved mapping design and CC / Program Change parsing. Note audition, step recording and MIDI Start / Continue / Stop are available now.

## Scene engine and projector

Keep loading and hot-reloading custom WGSL. Prototype the scene library with a small operator subset selected by inspecting the user's existing scene files. The proposed `.scn` format has optional named parameters and a `clear` flag. `Sxx` should switch to a previously compiled pipeline. By default a scene switch preserves the existing feedback buffers. A scene may explicitly clear them.

```text
clear: false
params:
  amount  0.0..1.0 = 0.35
  spin    -1.0..1.0 = 0.1
  density 1..32 = 8

osc(density, 0.1, 0.8)
  .rotate(spin)
  .modulate(noise(3), amount)
  .blend(src(o0), 0.92)
  .out(o0)
```

Use the project's [clean-room policy](../CLEANROOM.md) for synthesis and video. Treat Hydra and hydrapy as behavioral references. Separate the author of the public operator spec and conformance fixtures from the DSL-to-WGSL implementer. Freeze UV conventions, defaults, compositing and feedback order in the fixtures before writing the compiler.

An optional F9 projector viewport should reuse the output texture where the backend supports it. Measure a single display first, then interactive editing on a 120 Hz laptop while a 60 Hz output is active. Record present modes, missed frames, GPU pass time and audio underruns on the target OS and adapters. In wgpu 24, `AutoNoVsync` can fall back to FIFO, `Mailbox` requires surface support, and native pipeline caching is Vulkan-specific. Four 960×540 `Rgba16Float` ping-pong outputs consume about 31.6 MiB for the eight textures alone. Measure redraw, compile and texture budgets on the integrated GPU, including waveform upload and projector output.

A future bundle can hold score JSON, scene files, WAVs and image assets. Specify it once fixtures cover v1 migration, PCM budgets, relative assets and a complete save/load. Compiled artifacts are disposable and backend-specific.

## Rack bridge parity

Leviathan uses a sample-anchored beat clock and default 4 PPQN CV/ByteBus pulses. Gorgon's 1920 PPQ is score/wire resolution. Its Rack side also evaluates score events, so compatible transport snapshots alone cannot establish equivalent output. For example, the same probability hash `1_936_593_078` for seed `0x1234`, pattern 0, lane 1, row 12 and first pass reduces to 45 in Leviathan and 78 in Rack. A 50% step fires in only one.

Build shared fixtures around gate timing, probability and FILL, swing, nudge, retrigs, seeks and chain repeats, then check ByteBus overlays, parameter projections, multichannel capture and offset behavior. Verify with an existing Rack build using the pinned public contract. Keep loopback SoundLink UDP on port 9477 available through that comparison. [GorgonBridge contract](https://github.com/JosiahAlanLee/Gorgon/blob/22b8d4f0d61e5955deae42f726d57a763b0d8968/spec/bridge-api.md) · [Bus I/O](https://github.com/JosiahAlanLee/Gorgon/blob/22b8d4f0d61e5955deae42f726d57a763b0d8968/spec/bus-io.md)

Treat audio-device recovery as its own transport task. Today the desktop falls back to a UI clock when startup has no device. Test handoff during active playback, a recording, and a change of output sample rate before treating it as hot-swap support.
