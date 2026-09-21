# Static Seek Visualizer

Working doc. Delete when the feature ships; leave an ADR behind first.

Status: **not started** · Created: 2026-09-21 · Revised: 2026-09-21

Issue [#108](https://github.com/KenanSalar/Melodia/issues/108). No branch yet.

> Facts below were verified **2026-09-21** against this tree at `6eddcfa3`, the pinned
> `symphonia 0.6.1`, `i-slint-core 1.16.1`, `i-slint-renderer-femtovg 1.16.1` and `femtovg 0.27.0`
> sources. The decode figures were measured in-session with a throwaway probe built against the same
> `symphonia 0.6.1` this workspace links; the protocol is in the cost section.
>
> The rendering, component and geometry decisions below replace an earlier draft's, which was
> written before the renderer's own sources had been read. Where a decision was reversed the
> reason sits with it, so the reversal cannot be quietly undone.
>
> Anything marked ⚠️ **re-verify** is expected to drift or was not reachable without writing the
> code; check it on the day rather than trusting this doc.

---

## The symptom

The seek track is a flat 4 px rule. It says where you are in a track and nothing about the
track. A quiet intro, the point a live take turns into applause, the bar where a long recitation
changes pace: none of it is visible, so finding a remembered spot is a hunt of click, listen,
click again. The longer the file the worse it gets, and an hour-long recording is one press of the
track away from a three-minute one.

The app already draws the shape of its audio, well, and in the wrong place for this. The Mirrored
visualizer style says plenty, but only about the 40 ms passing right now, and only on a page the
player bar is not on. The one control that could carry a whole track's shape is the one that
carries none.

## What ships

A **style picker for the seek track**, in Settings under Playback. Its default is the track exactly
as it is drawn today, and its other entry replaces the progress slider's twin bars with a **static
Mirrored strip of the whole track**: the same centre-anchored bars the visualizer draws, frozen, one
bar every few pixels across the track's full length. An outer envelope carries each bucket's peak in
the accent, a denser band inside it carries the same bucket's RMS, the played side and the unplayed
side take the fill and track colours the row already passes down, and the thumb and its touch area
are the ones already there.

Computed once per file, cached on disk, read back in microseconds on every later play. The plain
rule is the default, so an install that never opens the picker decodes nothing and writes nothing.

Both hosts get it. `SeekRow` mounts at `layout/now-playing-bar.slint:256` and
`components/mini-player/mini-controls.slint:169`, both already behind `!Player.vm.has_station`, so a
stream has no timeline to draw and needs no special case. The miniplayer's row is shorter, so a whole
track lands there at fewer, no narrower bars; the bar count comes from the width, so nothing about
that is a second code path.

---

## The decision

Five facts about this tree decide most of the design, and three of them mean the feature is
mostly wiring.

### The drawing already exists, tuned, with one call site

`spectrum::write_bar_path(levels, strip, anchor, out)`
(`crates/melodia-playback/src/player/playback/spectrum.rs:536`) with `BarAnchor::Centre` (`:441`)
**is** this feature's renderer. It is the Mirrored style's writer, and everything a static seek strip
needs from it is already argued there:

| what it already does | where |
|---|---|
| one level per bar, clamped `0..1`, height as the **total** split half either side of the axis | `spectrum.rs:570-575` |
| whole-pixel pitch and gap, remainder to the two ends, so every bar matches every other | `band_pitch`, `:606` |
| every edge snapped to a device pixel against the window's own grid, which is what `StripGeometry` is for | `:454`, and `anti-alias: false` on the host |
| a silent stretch resting as a square mark rather than vanishing | `:566` |
| the winding femtovg reads to tell solid from hole | `:585-593` |
| coordinates normalized into a unit viewbox, so the bar count never crosses the language boundary | `GridAxis::normalize`, `:520` |

Its only production caller is `crates/melodia-views/src/ui/visualizer/frame.rs:39`; four test callers
sit in `playback/tests/spectrum_tests.rs` and `ui/visualizer/tests/visualizer_tests.rs`.

**The gap has to become a parameter, and the reason is arithmetic rather than taste.** `BAR_GAP_PX`
(`spectrum.rs:471`) is a private module constant tuned for 64 bands on a narrow strip, and
`band_pitch` clamps it to `(pitch * 0.5).floor()`, dropping it outright below a 2 px pitch
(`:606-618`). At seek-track density a 2 px gap takes half a 4 px pitch, and a strip that picked a
fixed bar count instead of a target pitch collapses toward a pitch of 2, where the gap is 1 and the
bar is 1: a hairline comb, not bars. So the bar count comes from the strip's width and a target
pitch, and the gap is threaded through `write_bar_path`. Lifting it is also what makes it reachable,
`melodia-views` being a different crate.

So the seek strip and the live strip read as siblings, drawn by one writer, which is the
coherence a second hand-rolled bar path would throw away.

### Two bytes per bucket, because the second one cannot be added later for free

The writer takes magnitudes, not min and max pairs, because a centre-anchored bar is symmetric. So a
stored bucket is **two unsigned bytes: peak and RMS**, each a magnitude, and the asymmetric min/max
band that `waveform::write_path_commands` draws is neither what this feature wants nor what gets
stored.

Peak alone reads as an outline. The band inside it is what makes a loud passage look dense rather
than merely tall, and it is the shape a whole-track strip is conventionally drawn in. The byte costs
2 KB a track and is the one thing in the format that **cannot** be added by a version bump without
re-decoding every file in the library, so it goes in from the start whether or not the first release
paints it.

Both lanes go through `write_bar_path`, called twice into two `Path` elements, so the strip still has
exactly one bar writer. The RMS lane is clamped inside the peak lane at paint, so the band can never
poke out of its own envelope.

The reducer for the pass is therefore peak magnitude and root-mean-square per bucket, not
`waveform::min_max_columns`, which is correct for the live trace's fixed window and is the wrong
shape here. `min_max_columns` still earns its place in the UI half, with one caveat: it takes
**samples** (`waveform.rs:111`, `src: &[f32]`), not columns, and there is no column-to-column fold in
the tree. So folding stored buckets to the bar count means dequantizing a lane into a flat `&[f32]`
and handing that over, taking each `Column::max`. Two lanes, two calls.

### A resize rebuilds the string, and so does a move

`write_bar_path` writes normalized coordinates but takes **pixel facts**, and three of them are
window-relative: `GridAxis::new` derives `phase = (origin * scale).rem_euclid(1.0)`
(`spectrum.rs:503`) so that every bar edge lands half a device pixel clear of a pixel centre. So the
string owes a rebuild whenever the strip's **width, height, scale or position within the window**
changes, and a sidebar drag moves it without resizing it.

That is one build of a few hundred bars per such change. The live strip rebuilds 64 bands through the
same writer every 33 ms, so the cost here is known and is nowhere near the budget.

What must not happen is a rebuild per position tick. The bars do not move; only the split does.

### The geometry cannot reach Rust through a `changed` handler

Both `SeekRow` mounts sit inside `if !Player.vm.has_station`, and `seek-row.slint:11-12` says
outright that nothing in that component carries a `changed` handler, **which is what makes the branch
safe to drop**. A `changed` tracker on a layout property inside a droppable branch is the documented
panic in `.claude/rules/slint-pitfalls.md`: the tracker outlives the branch, the surviving parent
re-dirties the property the moment it re-flows without the child, and `ChangeTracker::evaluate`
unwraps a weak that no longer upgrades. A discrete write from Rust is explicitly not the cure there.

So the geometry crosses the way the visualizer's does: a `Timer` whose `triggered` body reads
`absolute-position` and the size and hands them over as **callback arguments**
(`components/now-playing/visualizer-strip.slint:30-56` is the worked example). No tracker anywhere in
the branch, and the first fire is the mount seed, so the 1 ms mount `Timer` the `changed` idiom owes
is not needed either. Rust compares against the last geometry and rebuilds only on a change, so a
lazy interval costs a property read and a comparison.

### The played and unplayed halves are a clip, not a gradient

An earlier draft painted both sides with one hard-stop linear gradient over a single `Path`. That is
wrong twice over, and both halves are in the renderer's sources rather than in its docs.

**It does not resolve against the element.** `brush_to_paint`
(`i-slint-renderer-femtovg-1.16.1/itemrenderer.rs:1495`) takes `path_bounding_box(&self.canvas, path)`
(`:160`), the **path's own** bounding box, and anchors the run at the item origin via
`line_for_angle(angle, [path_w, path_h])` (`i-slint-core-1.16.1/graphics/brush.rs:524`, which for
`90deg` returns `(0, cy)` to `(w, cy)`). The bars' bbox is inset from the element by the pitch
remainder `write_bar_path` deliberately parks at each end, so the stop fraction is not the progress
fraction and the error moves with the remainder.

**And it is on the expensive path.** Four stops with two inset positions fail the `TwoStop` test at
`femtovg-0.27.0/src/paint.rs:229-235` and fall through to `MultiStop`, which synthesizes a 256x1
gradient texture per distinct stop set in `GradientStore::lookup_or_add` (`gradient_store.rs:30`).
The key includes the stop positions, so a moving split allocates and releases a texture on every
position tick and on every frame of a drag. That is the mechanism `slint-pitfalls.md` already records
against the aurora's inset stops, where the bill arrived as a driver-pool slab.

**So: one `commands` string, two `Path` elements, and a clip.** The unplayed copy paints in the track
colour at full width; the played copy paints in the fill colour inside a
`Rectangle { clip: true; width: root.frac * root.width }` whose radius is zero, with the inner `Path`
given the full strip width explicitly so `fit` still maps the viewbox onto the whole strip. A
radius-less clip lowers to a scissor and builds no offscreen layer. No gradient, no texture, no
per-tick string rebuild, and the boundary still cuts through whichever bar the playhead is inside,
which is the more precise of the two behaviours. With the RMS band that is four `Path` elements, two
inside the clip.

⚠️ **re-verify** on the day: the extra tessellation. FemtoVG caches no path between frames, so the
figure is built twice (four times with the band) per repaint of that region. The fallback if it ever
measures is two command strings split at a bar boundary, rebuilt only when the bar under the playhead
changes, which costs the same tessellation as one figure and quantizes the split to a bar edge.

### A whole-file pass cannot hand a reducer the whole file

The longest file in the test library is 8 hours, which at 48 kHz stereo is about 11 GB of `f32`, and
5.5 GB after the mono downmix. The pass therefore reduces in two stages: a coarse pass accumulates
peaks and sums of squares into fixed-size blocks as packets arrive, which makes resident memory flat
in track length, and a fold collapses the coarse bins to the stored bucket count. The probe measured
**8.8 MB peak RSS** on that 8-hour file, which is the shape working.

---

## Measured cost

Throwaway probe, built against the same `symphonia 0.6.1` this workspace links: full decode, mono
downmix, streaming reduction into 1000 buckets. Run on the 16-core dev machine against a real
511-file library. Not a criterion benchmark; single samples, and the wall figures include reading
the files. The probe reduced to min and max pairs, which is close in shape to the peak-and-RMS
reduction this feature needs, so the figures are indicative rather than a strict bound either way.

| | |
|---|---|
| 4:49 stereo 192 kb/s MP3 | **181 ms** |
| the same audio as FLAC | **160 ms** |
| per file, uncontended, p50 / p90 | **168 ms / 368 ms** |
| whole library, 511 files, 87.2 h of audio | **213.8 s of core time**, 21.5 s wall at 16-way |
| mean throughput | about 1470x realtime |
| longest single file (8 h) | 16.8 s |
| peak RSS of the pass | 8.8 MB |

Two things those settle. **First-play latency is not worth engineering around**: 168 ms is below
the threshold where a missing strip reads as broken, and prefetching the next queue entry removes
it entirely for sequential listening. **A whole-library backfill is affordable**: 21 s of wall
time once, for a library this size, is less than the artwork pass already costs.

The parallel run reached 950% CPU on 16 cores, so the pass goes I/O-bound before it goes
CPU-bound. A parallelism cap matters more than per-file speed.

⚠️ **re-verify** before Phase 6 ships: issue #108 asks for this to be confirmed on a library
substantially bigger than the test one before the background fill is let loose. 511 files is not that
library.

---

## Storage

At 2048 buckets of two bytes an entry is **4 KB**: about 2 MB for the test library, about 80 MB for
a 20,000-track one. 2048 is the count to use rather than a number derived from the strip: the point
is to hold far more buckets than the widest strip has bars, so a resize is a fold rather than a
re-decode, and it is where the conventional answer sits.

**It does not go in the database.** The pre-migration backup is a `VACUUM INTO` copy of the whole
file (`melodia-store/src/database/backup.rs`), so a table of derived data doubles into `backups/`
on every migration, for bytes that are recomputable in 21 seconds. It also has no business in a
file the user is told holds their library.

It goes in a fourth store directory beside `artwork/`, `artists/` and `radio-logos/`,
**content-addressed by the BLAKE3 already in `tracks.file_hash`**, which is what makes an entry
survive exactly the events it should:

| event | keyed by path | keyed by tag metadata | keyed by `file_hash` |
|---|---|---|---|
| file moved or renamed | orphaned, recomputed | survives | **survives**, the hash is what carries a move across the delete plus create |
| re-encoded, same duration and tags | **stale entry served** | **stale entry served** | invalidated |
| tags edited in place | survives | new key, old entry orphaned | recomputed, 168 ms |
| track deleted | orphaned | orphaned | swept, `tracks.file_hash` is the reference set |

The two columns that serve a stale strip are what a path key and a tag key cannot avoid, and a
silently wrong picture over the right audio is the worst failure this feature has. The hash column
and the moved-file resolution it feeds already exist here, so the correct key costs nothing to
adopt. It is also stricter than the conventional answer, which reaches for a path, an mtime or a
tag hash and accepts one of the two stale columns in exchange.

The cost of choosing it is the third row: a tag edit rewrites the file, which moves the content
hash, which discards a still-valid strip. That is one 168 ms pass on the next play and it is the
right trade. It is also why the lyrics store hashes the *path* instead
(`library/lyrics/store.rs:133`), so the two stores will look inconsistent side by side and each owes
its argument at its own definition rather than a shared note somewhere neither reads.

**The sweep is the size limit.** Issue #108 asks that the store never grow without one, and the
reference set being `tracks.file_hash` bounds it to the library rather than to history. There is no
second cap and no LRU.

**Nothing normalized is stored.** Absolute quantized magnitudes, with the perceptual curve and any
normalization applied at paint. Storing a curved or normalized strip freezes a display decision
into the cache, and changing it later means re-decoding the library.

---

## Structure

| what | where | note |
|---|---|---|
| whole-file decode entry point | `melodia-audio/src/player/source/file_decode.rs` | `decode::open` is `pub(super)` (`decode.rs:176`) and nothing in `decode` is `pub`, so no second probe is reachable. `FileDecoder` (`:94`) already iterates a whole file, so this is one `pub fn` of convenience over it, not a new capability |
| the pass: decode, downmix, coarse peaks and squares, fold, quantize | `melodia-store/src/media/ingest/peaks.rs` | ingest-shaped, and the **second** user of the one edge this crate has into the audio stack, the first being `probe_duration` at `media/ingest/metadata.rs:254` |
| the store: read, atomic write, the header, the sweep | `melodia-store/src/media/peaks/` | not `melodia-artwork`, which is the image tier and the wrong crate by name for a byte array |
| `Paths::peaks_dir` | `melodia-core/src/config.rs`, decl beside `:35`, join in the `:116-119` block, one entry in `create_dirs:127` | one doc comment saying why it is its own directory rather than a table, and why it is not in `artwork_dir`. `radio_logos_dir`'s comment at `:21-27` is the precedent |
| the entity crossing to the UI | `melodia-core/src/entities/` | `melodia-views` cannot name `melodia-store`; a payload it cannot name means the missing entity is the bug |
| the one door | `melodia-app/src/library/peaks.rs` | cache read, else compute and store. The radio and lyrics doors are the shape |
| the style key | `SeekTrackFlags` beside `VisualizerFlags` (`melodia-app/src/services/settings/playback.rs:127`), setter beside `set_visualizer_enabled` in `library/settings/visualizer.rs` | `settings.json`, not `views.json`: it is a preference, not per-view state |
| the backfill | `melodia-app/src/tasks/peaks_backfill.rs` | `one_shot::spawn` (`tasks/one_shot.rs:59`) owns the marker shape; `retroactive_hash.rs` is the template for the pass inside it, being rayon in `spawn_blocking` with one batched write. They are two different templates and this uses both |
| the sweep task | `melodia-app/src/tasks/` | `artwork_sweep.rs` is the template, over `artwork/sweep.rs`'s `collect_candidates:57` / `retire:105` / `GRACE:24` |
| the UI half: fetch, fold to bar count, build the strings | `melodia-views/src/ui/` | `ui/visualizer/mod.rs:233-239` is the precedent for assembling a `StripGeometry` from a mounted element |
| the strip | `melodia-ui/ui/components/now-playing/seek-peaks-strip.slint` | the `Path` elements, the clip, and the geometry `Timer`. Mounted by `SliderTrack`, not beside it |
| the slider | `melodia-ui/ui/components/slider-track.slint` | gains the two command-string inputs; everything else about it is untouched |

**The strip reskins the slider; it is not a sibling of it.** `SeekRow` (`seek-row.slint:17`) owns
only the two labels and the seek-pending hold. The thumb, the `TouchArea`, jump-to-click, the
no-move-grab guard, commit-on-up and `drag-value` all live in `SliderTrack` (`:22-33`, `:66-69`,
`:128-163`), so a sibling component copies about a hundred lines of interaction that is subtle in
every one of them. Instead `SliderTrack` takes `bars-path` and `rms-path`, empty by default; non-empty
swaps its twin bars for the strip and **suppresses the seam mask**, a mantle stadium punched through a
waveform reading as damage rather than as a seam. Of its four mounts (`seek-row.slint:52`,
`dialog/preamp-slider-row.slint:37`, `now-playing/volume-popup-horizontal.slint:137`,
`views/settings/playback-section.slint:133`) three pass nothing and are unaffected.

Four duplications this deliberately does not create.

**No second bar writer.** `write_bar_path` draws it, both lanes. A hand-rolled path here is how the
two strips would drift apart on pitch, snapping and the silent-bar floor, all three of which are
already solved and none of which is obvious.

**No second reducer for the fold.** Stored buckets fold to the bar count through `min_max_columns`,
one call per lane, taking each `Column`'s `max`.

**No second quantizer.** The magnitude-to-byte rule and its inverse live in one place, beside the
version byte that versions them. Two copies is the bug that reads as a rounding difference nobody
can reproduce.

**No second decode preamble.** The pass calls the new entry point rather than building its own
probe, track pick and decoder, which is the argument `decode.rs` already makes for the file and
stream paths.

---

## Phase 1 - The pass

Pure functions over samples. No storage, no UI, no settings.

- [ ] One `pub fn` in `file_decode` that opens a file and yields decoded packets to a
      caller-supplied sink, over the existing `decode::open`. Nothing else in the tree gains the
      ability to open a decoder.
- [ ] `media/ingest/peaks.rs`: mono downmix, then per coarse fixed-size block a peak magnitude and a
      sum of squares; fold to the target bucket count; quantize each lane to a byte.
- [ ] Name the block size and the bucket count as constants with the argument at each, not the
      figure. The block size is what keeps memory flat in track length; the bucket count is what
      keeps the strip from repeating buckets on a wide bar at 2x scaling.
- [ ] `AppError::scanner` on a decode failure with the typed cause preserved. No `Result<_, String>`.
- [ ] Unit tests: silence reduces to all zeroes in both lanes, a full-scale tone reaches the top of
      the peak range, RMS of a full-scale square equals its peak and RMS of a sine sits below it,
      RMS never exceeds peak in any bucket, fewer samples than buckets does not read past the end,
      one bucket is not a divide by zero, and the quantizer round-trips inside its own step.

**Gate:** `cargo test --locked --workspace` green, and the pass over the longest fixture holds
resident memory flat rather than proportional to duration.

---

## Phase 2 - The store

- [ ] `Paths::peaks_dir`, created beside the others, with a doc comment saying why it is its own
      directory rather than a table and why it is not in `artwork_dir`.
- [ ] Entry name is the track's BLAKE3 prefix at the same 16 hex characters the artwork and lyrics
      stores use, so a directory listing reads the same way in all three.
- [ ] A header carrying a magic, a version byte and the bucket count. A version change reads as a
      miss, which makes a format change a one-line edit rather than a migration.
- [ ] Write through `utils::atomic_file::write_with_sync` (`atomic_file.rs:69`), so a half-written
      entry cannot be read as a short one. The read side is a plain bounded `std::fs::read`: that
      module's readers are JSON-only and there is no binary twin to reach for.
- [ ] Read returns `Option`: an unreadable, truncated or wrong-version entry is a **miss, not an
      error**. Every answer this store gives is an optimisation and the caller's next move is the
      same for absent and for corrupt.
- [ ] Bound the read by the exact expected length before allocating, so a planted entry cannot ask
      for an arbitrary allocation. The length is fixed by the header's bucket count times two.
- [ ] Sweep over `collect_candidates` / `retire` against a `referenced_hashes` query, with the same
      grace window, retiring **only names that parse back into the scheme the module writes**. The
      existing artwork query is `referenced_filenames`, so this is a new query rather than a rename.
- [ ] Tests: round trip, truncated reads as a miss, wrong version reads as a miss, an over-long
      entry is refused before the allocation, and the sweep retires an unreferenced name while
      leaving a referenced one and a foreign filename alone.

**Gate:** the sweep's tests, and a hand-planted oversized entry refused rather than read.

---

## Phase 3 - The door

- [ ] `library::peaks::for_track` is the only way anything above reaches either the store or the
      pass. Cache read, else compute and store, else nothing.
- [ ] In-flight dedupe, so two asks for one track decode once. Two hosts can mount the bar and a
      prefetch can race the current track.
- [ ] Cancellable: a queue skipped three times must not leave three passes running to completion.
      `TaskSpawner::spawn_cancellable`, with the poll inside the coarse loop. A generation counter
      beside it discards a pass whose track has already been left.
- [ ] A parallelism cap rather than a thread priority. `std::thread` exposes no nice level and
      `libc` is reachable only from `services::platform::allocator`, so the throttle is how many
      passes run at once.
- [ ] A row whose `file_hash` is still NULL has no key, so the door returns nothing rather than
      computing an entry it cannot store or hashing the file itself. `tasks/retroactive_hash.rs`
      runs on every boot, so the gap is transient and the next play fills it; writing the hash here
      would make the door a database writer, which is a larger claim than a cache read.
- [ ] Return the entity, never a store type.

**Gate:** a test that two concurrent asks for one track produce one pass, and that a cancel
between packets stops it.

---

## Phase 4 - The UI

- [ ] `seek-peaks-strip.slint`: four `Path` elements over two `commands` strings, `anti-alias: false`,
      the unit viewbox with `fit: fill` the bars path already needs, and the played pair inside a
      radius-less `Rectangle { clip: true; width: frac * width }` with each inner `Path` given the
      full strip width. Every colour an input, the way `SliderTrack` already takes them.
- [ ] `SliderTrack` takes `bars-path` / `rms-path`, mounts the strip when either is non-empty,
      suppresses the seam mask on that arm, and leaves the thumb, touch area and both callbacks
      exactly as they are.
- [ ] The geometry reaches Rust from a `Timer` in the strip, handing `absolute-position`, size and
      the scale factor over as callback arguments. **No `changed` handler anywhere inside the
      component**, for the reason `seek-row.slint:11-12` already documents and
      `slint-pitfalls.md` argues: both `SeekRow` mounts sit in a droppable branch.
- [ ] The Rust half: on track change ask the door, fold each stored lane to the bar count, build both
      strings through `write_bar_path` with `BarAnchor::Centre`, set the properties.
- [ ] The bar count comes from the strip's own width and a target pitch, the way
      `waveform::columns_for_width` derives its column count at `LOGICAL_PX_PER_COLUMN`. The pitch
      and the gap are the two new visual constants and they are what decide whether this reads as
      bars or as a solid block.
- [ ] Thread the gap through `write_bar_path` as a parameter. One production caller and four test
      callers move with the signature.
- [ ] Apply the perceptual curve here, not in the store: a sign-preserving power law with the
      exponent as a named constant whose doc comment says what it buys.
- [ ] Rebuild on geometry changes only, which includes the strip moving without resizing. A position
      tick moves the clip's width and nothing else.
- [ ] `SeekRow` picks the strip or the plain bars off the style key, and falls back to the plain bars
      whenever the string is empty, so a track with no entry yet is the rule rather than a gap.
- [ ] Clear the properties on track change **before** the new ones arrive, or the outgoing track's
      shape sits under the incoming track's playhead.
- [ ] Prefetch the next queue entry so sequential listening never sees the fallback.

**Gate:** manual. A track shows its strip, the split tracks the position, a drag still seeks, a
resize redraws at the right pitch, a sidebar drag redraws it too, the station branch still shows a
station row, the miniplayer draws a coarser strip of the same track, and the fallback appears rather
than a gap on a track with no entry.

---

## Phase 5 - The picker

- [ ] `SeekTrackFlags { style: String }` with a `DEFAULT_SEEK_TRACK_STYLE` const, `#[serde(default)]`
      so a saved install reads the field as absent rather than failing, composed into `SettingsData`
      beside `visualizer`.
- [ ] **Matched on the key, never on an index**, so reordering the picker cannot repoint an install's
      saved choice, and an unknown key degrades to the plain rule. `visualizer-strip.slint:20-22`
      states the same rule for the visualizer's styles.
- [ ] `set_seek_track_style` beside `set_visualizer_enabled` in `library/settings/visualizer.rs`:
      three lines over `mutate_settings`, no runtime half.
- [ ] A `SettingRowStacked` holding a `ChipGroup` under Playback, mirroring the visualizer's own
      style row at `playback-section.slint:226-237`, plus a label and description pair, a `show-`
      gate, a term in that section's `has-matches`, and one more link in the divider chain.
- [ ] The style names are an inline `[@tr("…"), …]` literal in picker order. `@tr` resolves literals
      at codegen, so a `[string]` filled from Rust ships untranslated;
      `flyout-presets.slint`'s `viz-style-names` is the worked example.
- [ ] The key gates the door, not just the drawing: with the default selected nothing decodes and
      nothing is written. A style that is not chosen should cost a branch, not a background decode.
- [ ] Every new label is one new msgid in all six catalogues.
- [ ] Decide before the Settings row is written rather than after: whether the picker also appears in
      the player bar's overflow menu, where the thing it changes lives. The visualizer's flyout is
      the shape if it does.

**Gate:** with the default selected, a fresh profile plays a library through without creating the
store directory's first entry.

---

## Phase 6 - The backfill

- [ ] `tasks/peaks_backfill.rs` over `one_shot::spawn`, gated on the style key, skipping rows that
      already have an entry and rows whose `file_hash` is NULL. The marker lives in `LibraryFlags`,
      which is what `one_shot::Sweep` reads.
- [ ] Rayon inside `spawn_blocking`, capped, observing shutdown.
- [ ] `Sweep`'s `label` and `marker` stay separate, so a bug report reads the sentence rather than
      the flag name.
- [ ] Choosing the strip is what kicks it, not every boot.
- [ ] **This is the one place the feature goes past the conventional answer.** The usual shape is
      lazy-on-play plus a prefetch, with at most an opt-in, selection-driven regenerate. Issue #108
      asks for the whole-library pass, so it ships, and that is exactly why it stays behind the
      picker, capped, cancellable, and gated on the timing below.

**Gate:** the cost confirmed on a library substantially bigger than the 511-file one the figures
above come from; then a library with no entries fills without the UI stalling, and a shutdown
mid-pass exits promptly rather than waiting for the library.

---

## Phase 7 - The walks

Each of these is violable from any file and none can fail visibly.

- [ ] Nothing outside the sanctioned entry point opens a decoder over a whole file.
- [ ] One quantizer, one inverse.
- [ ] `write_bar_path` is the only writer of centre-anchored bars, and its production callers are
      exactly the visualizer frame and the seek strip's two lanes.
- [ ] The seek track's style is resolved by key and never by a persisted index.
- [ ] Nothing under the strip's own file carries a `changed` handler.
- [ ] Each walk carries a vacuity floor, collects unreadable paths and asserts them empty, strips
      comments before the needle, and holds its exemption list to an exact count.

**Gate:** each walk fails when the property is deliberately broken in a scratch edit.

---

## Phase 8 - Docs and exit

- [ ] `README.md`: the feature line, and the setting if the settings list names peers.
- [ ] Root `CLAUDE.md`: the persistence bullet gains `peaks/`, and the store list goes from three
      directories to four. The sweep's reference set is the column to name. The visualizer rule's
      strip section is where `BAR_GAP_PX` becoming a parameter is recorded.
- [ ] Delete this file.

---

## Cross-cutting

- **No `unwrap`, tests included.** `expect` only with the invariant in the message.
- **800 lines per production file.** The pass, the store, the sweep and the door are four files from
  the start rather than one that has to be split later.
- **Footprint.** The pass is flat in track length; the UI holds two command strings per track. At a
  few hundred bars that is tens of kilobytes. Nothing here is a new cache to size.
- **No new `#[allow(dead_code)]`**, and no `#[allow]` where `#[expect]` fits.
- **The visualizer's rule mostly does not apply.** No tap, no tick, no FFT, no analyzer buffers, no
  arming split. What does carry over is its geometry argument: pixel snapping, the whole pitch, and
  why anti-aliasing is off. Borrowing the rest would drag a frame timer into a feature that
  repaints on a position tick.
- **CUE and gapless get nothing.** A strip is per file, so a CUE rip would draw the whole image
  under a playhead addressing one track inside it. Out of scope, and worth saying so the gap is a
  decision rather than a surprise.

---

## Open questions

1. **Bar pitch and gap.** The two new visual constants, and what decides whether this reads as the
   Mirrored strip or as a solid block. Tighter than the live strip's, since this spans a whole track
   rather than 64 bands, and worth trying on a real window rather than picking a number. Whether the
   miniplayer's shorter row wants its own pitch is the same question asked twice.
2. **What colour the RMS band takes.** A transparentized fill colour keeps the strip to the two
   brushes the row already passes down; a third input makes it tunable and gives the row one more
   thing to pass. The played and unplayed sides both need an answer.
3. **Whether the picker also lands in the player bar's overflow menu.** Issue #108 asks for it,
   the visualizer's flyout is the shape, and it is cheaper to decide before the Settings row is
   written than after.

---

## Verification across the whole feature

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

Then, once the binary exercises it: play a track, scrub it, skip through a queue faster than a
pass completes, resize the window across the bar's whole range, drag the sidebar without resizing
the window, switch to the miniplayer and back, and move the picker off and back onto the strip with
the backfill mid-flight.
