# Visual sampler editor: from break to kit

**Status:** Proposed. Android tablet first; shared editor and project format on desktop.

## The workflow

Get four hits out of a break and onto the tracker without leaving the tablet:
**Samples → Open WAV → Trim → mark three splits → Split to Kit → Grid.**
Keep Leviathan's existing sampler sound and sequencer. The upgrade is making it
easy to see, hear, and place a hit without entering percentages or switching apps.

| Stage | What the musician can do |
| --- | --- |
| Open WAV | On Android, pick a WAV from Files; on desktop, open or drop a WAV. Show its name, duration and sample rate, then draw the whole file. |
| Trim | Drag start/end handles; zoom around the selected handle and pan without moving a marker. Show a close-up, time/frame readout, and **−1 / +1 frame** buttons for precise edits. **Play selection / Stop** auditions the exact trimmed interval at source pitch, independently of tracker transport. |
| Slice | Place, move or remove manual split markers within the trim. Select any numbered slice to audition it. Keep zoom and marker gestures distinct: one-finger drag on a handle edits it; drag the empty waveform to pan; explicit zoom controls work without multitouch. Pinch can follow once scrolling has been tested on the tablet. |
| Make a kit | **Split to Kit** previews which consecutive empty slots will receive each hit. Publish all slices in one operation, with no overwrite in v1. An occupied destination, insufficient slots, or an over-budget kit blocks the whole operation and explains why. |
| Sequence | Hear a rendered kit slot with the existing kit audition. On a compatible note-addressed OtisKit lane, offer a direct **Enter hit** control for the selected slot at the active row. Use the existing slot mapping and undoable step command; do not silently repatch a lane, change its addressing mode, or rewrite another cell. |

On portrait tablets this is a full-width Samples deck, not another narrow
sidebar. Keep transport above the waveform and put audition/slice actions within
thumb reach, inside Android's system bars. Keep the selected pattern, row and
lane when returning to Grid. On desktop use the same editor/model with mouse
and keyboard, not a separate sampler implementation.

## Rules that keep edits predictable

- The source is immutable mono PCM at the original sample rate. Editing uses
  integer frame indices and half-open intervals: trim is `[start, end)`, and a
  split is the end of one slice and the start of the next. A slice must contain at
  least two frames. Handles cannot cross, and a trim that would strand markers
  is blocked until those markers are removed or moved. No destructive crop or
  resampling while dragging. Markers are a draft until the user commits.
- Use signed min/max waveform buckets cached by source and resolution; draw
  only visible buckets. At high zoom, show individual samples. Calculate peaks,
  decode PCM and copy slices off the audio callback. Publish only bounded,
  validated commands and immutable snapshots through the existing audio path.
- Reuse the current [audition player](../crates/leviathan-app/src/audition.rs)
  and its short edge ramp. If needed add a source-PCM input to that player
  instead of opening a separate audio stream. Preview is neutral/source-rate
  playback; once mapped, kit audition is the check for instrument coloration
  or slot settings. Cancel a preview if its source is replaced.
- Use the existing [WAV decoder](../crates/leviathan-app/src/wav.rs) and its
  mono conversion, but factor a bounded byte/stream decode for this workflow.
  Do **not** round-trip slice sources through the VarSampler bank import: that
  path can resize PCM to fit the bank. Enforce the kit import byte and frame
  limits up front, including for providers without a reported size. Reject
  invalid input before mutating the current edit.
- V1 destination is the existing [OtisKit](../crates/leviathan-app/src/kit.rs):
  eight slots and a shared 1,048,320-frame budget. Build and validate a candidate
  kit first; then send one retained snapshot and make it one undo operation.
  Use deterministic slice names and default full-range, forward, non-looping
  settings for newly populated slots; leave existing slots and lane settings
  unchanged. Never approximate slot addressing with a UI-only formula. Share
  or test against the engine mapping (including note offset and slot shift);
  slot triggers must not be changed by melodic scale lock.
- Committed hits are ordinary kit PCM embedded in the existing project format.
  Keep the original WAV untouched; the resulting project must reopen without
  the WAV or Android URI. Draft markers only need to survive deck switches in
  v1; persistence of editable source/marker sets can follow. Keep the project
  size limit; if the result cannot be saved, report that before committing.

## Implementation order

1. **Shared trim and audition.** Add frame-based editor state, cached waveform,
   drag handles, zoom/pan, nudge buttons and selection audition. Build against
   a desktop fixture first. Keep the existing VarSampler bank controls working.
2. **Android Files adapter.** Use the system
   [document picker](https://developer.android.com/training/data-storage/shared/documents-files)
   with `ACTION_OPEN_DOCUMENT` and read the selected content URI through
   `ContentResolver`; no broad storage permission or invented filesystem path.
   Prove the activity-result bridge for the current native shell (use a thin
   Android host adapter if needed) before building around it. On return, keep
   the editor and transport state; cancellation is a no-op. Read/decode on a
   worker, discard stale results after cancellation or a newer import, and use
   the existing portrait safe area. Test local and provider-backed Files.
3. **Manual slices to kit and tracker.** Add marker authoring, atomic kit
   assignment and the note-addressed entry path. Run the end-to-end test with
   Android project **Save a copy** (system document export is a sibling task;
   reuse the same picker boundary and the existing project codec).

## Acceptance

- A known WAV with four separated transients produces four distinct,
  source-rate hits. On a Galaxy Tab A9+ in portrait, import it, zoom and trim
  a transient, audition each slice, place four hits in Grid, and hear them on
  playback. Check the clock and navigation bars stay clear. Repeat with
  headphones and after returning from the Files picker.
- Test exact first/last frames, adjacent non-overlapping slices, a one-frame
  invalid interval, invalid/oversize WAVs, cancelled/stale opens, occupied
  slots, and aggregate budget overflow. A rejected assignment leaves the kit
  and pattern untouched. Undo restores the prior kit in one operation.
- Test actual rendered output for audition and all four slot mappings,
  including a non-default note offset or slot shift. Check that importing,
  scrolling or moving markers does not interrupt audio. Save a sample-bearing
  project on Android, open it on desktop without the source file, and compare
  slice PCM/rates, grid triggers and a deterministic offline bounce.

**Not v1:** auto-transient detection, destructive edits, time stretch, stereo
kit slots, new DSP voices, persistent slice sets, general project migration,
or Play Store packaging. Keep the scope to the edit-to-pattern loop.
