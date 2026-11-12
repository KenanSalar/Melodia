# ReplayGain Measurement

Working doc. Delete when the feature ships.

Status: **proposed**, no phase started · Created: 2026-11-12

Issue [#156](https://github.com/KenanSalar/Melodia/issues/156).

> Verified **2026-11-12** against this tree at `dc4c913a`, and the `ebur128` 0.1.10 source in the
> registry with 0.1.11's changelog. Anything marked ⚠️ **re-verify** drifts or rests on an outside
> fact; check it again on the day.

---

## The symptom

ReplayGain evens out the volume between tracks only when their files carry gain tags. A track with
none plays at its mastered level: `compute_linear_gain` returns unity when there is no gain to read
(`melodia-playback/src/player/playback/replaygain.rs:102-124`). So a library where only some
albums were tagged still jumps in loudness from one track to the next, which is exactly what
ReplayGain is switched on to stop. In Album mode a single untagged track inside a tagged album
stands out the same way.

The Settings card says as much: its description tells the user that only tagged tracks are
affected and points them at an outside tool to tag the rest.

## What ships

- A switch in the ReplayGain card, **Measure untagged tracks**, off by default.
- With it on, Melodia measures every library track that carries no gain tags, in the background,
  and keeps the result in the library. The files are never written.
- Each measured track gets a track gain and peak. An album gets an album gain and peak once all of
  its tracks are measured, measured as one whole the way the standard intends, never an average of
  its tracks.
- A measured track plays exactly as it would if the file carried those tags, in Track and Album
  mode alike, with the same preamp and clipping guard.
- A file that carries tags keeps using them. A file that gains tags later switches to its own on
  the rescan that reads them.
- A replaced file stops using its old measurement at once and is measured again. A track that
  leaves the library takes its measurement with it.
- Turning the switch on with a library in place measures the backlog in the background. After
  that, new tracks are measured as they arrive.
- A pass shows its progress under the switch and can be stopped.

**What keeps working:**

- Tagged tracks sound exactly as they do now.
- With the switch off, nothing is measured and playback is today's: stored measurements stay in the
  library but are not applied, so turning it back on is instant.
- Playback stays smooth while a pass runs, and a pass waits for a library scan.
- Memory doesn't grow with the library: a track is measured as it is decoded, never held whole, and
  the pass's threads exist only while it runs.

---

## Decisions

### What counts as untagged

A track is untagged when **both** `replaygain_track_gain` and `replaygain_album_gain` are `NULL`.
An R128-tagged Opus file already fills those columns at scan time
(`media/ingest/metadata.rs:200-203`, `:418-427`), so it counts as tagged. A file with either gain
keeps exactly today's behaviour, including today's fallback from one pair to the other.

### A measurement lives in its own table, keyed to the audio it heard

```sql
CREATE TABLE IF NOT EXISTS track_loudness (
    track_id INTEGER PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    -- what was measured: the file's whole-file hash and the row's cue span at the time
    file_hash TEXT NOT NULL,
    cue_start INTEGER,
    cue_end INTEGER,
    -- NULL: measured, nothing to level (silent, too short, unreadable)
    track_gain REAL,
    track_peak REAL,
    -- set together, once the whole album was measured; album_tracks is its member count then
    album_gain REAL,
    album_peak REAL,
    album_tracks INTEGER
);
```

**Not the four tag columns.** Every rescan of a changed file, every tag edit and the tag backfill
rewrites them from the file, `NULL` included (`TAG_BACKED_SET`,
`queries/scan/mutations.rs:296-315`), so a measurement written there would be wiped by the next
re-read. They also have to keep saying what the file says, since "a file that gains tags switches
to its own" is read from them.

**Valid only while its key matches the row.** A read uses a measurement only where `file_hash`,
`cue_start` and `cue_end` equal the track's current values. The hash is the whole-file BLAKE3
every re-read writes (`metadata.rs:24-34`), so:

- a replaced file stops matching the moment its re-read commits, with no write path having to
  remember an invalidation;
- a moved file keeps its hash and its id (`update_track_location`), so it keeps its measurement;
- a cue sheet edit that moves a track's timing keeps the row's id but not its span, so it stops
  matching;
- a row whose hash is still `NULL` (rows from before hashing, `retroactive_hash` fills them) is
  not measured until it has one.

**`ON DELETE CASCADE`** answers "a track that leaves the library takes its measurement" through
every delete path at once: the orphan purge, a folder's cascade, a duplicate removal, a sheet
letting go of its rows.

**Melodia's own tag writes carry the measurement forward.** A tag write changes the file's bytes,
and so its hash, without touching its audio. Left alone, the rating write-back alone would send
every rated track's album back to be decoded again. The two paths where Melodia writes only tags
into a file (`commit_retag` for Edit Tags and the rating write-back, and the sheet-tags write into a
FLAC's embedded `CUESHEET`) move `file_hash` from the old value to the new one, in the same
transaction, on rows that matched the old. A tag edit by another program changes the hash the same
way and is indistinguishable from a replaced file, so that track is measured again. That costs a
decode, never a wrong level.

**A file that can't be measured is recorded, not retried.** Silent audio, a file under one gating
block, a decode that fails part-way and a shape change mid-file all write a row with `NULL` gains.
The key makes it retry when the file changes, and never on every pass, which would decode the same
broken file on every wake.

### The album is measured as one whole

**The album is the library's album**, `tracks.album_id`: the album name and album artist, as the
album page groups them (`resolve_track_context`, `queries/scan/mod.rs:96`). A track with no album
name has no album (`upsert_album` answers `None` for an empty name), and gets a track gain only.
Album mode then levels exactly what it plays as one album.

**One running album meter, not every track's state kept alive.** The standard's relative gate runs
over the whole album's blocks. `ebur128` exposes no per-block energies a later pass could
recombine (`gating_block_count_and_energy` is after each track's own gate, so summing those is the
average the issue rules out). Two shapes give the right answer:

- keep every member's meter alive and combine them with `loudness_global_multiple`;
- feed every member's audio into one album meter as well as the track's own.

The second wins on memory. A meter holds a 400 ms buffer sized by rate and channels for its whole
life, so keeping every member's alive grows with the album's length, all of it per worker; a box set
at a high rate holds the lot. The album meter runs in `Mode::HISTOGRAM`, whose history is fixed
whatever the length, so a worker holds one track meter and one album meter. A track meter runs
exact (no histogram), its history bounded by the one file. Members are fed in disc and track order,
so the blocks across a join hear it as an album played through, which is what "as one whole" means.
An album whose members differ in rate or channel count gets one album meter per shape, combined
through `loudness_global_multiple`, which accepts histogram states.

**An album is complete only when every in-library member is untagged and measured**, its member
count matching `album_tracks`. Then:

- a member that couldn't be measured leaves the album without an album gain, and its siblings fall
  back to their track gain, as a file carrying only track tags does today;
- a silent member counts as measured, its blocks falling under the absolute gate like any quiet
  passage;
- an album with a tagged member never gets a measured album gain, and its untagged members get
  track gains, the scope ruling out re-measuring tagged files.

**The album values sit on each member's row, with the count they were measured over.** A read is
then one primary-key join. Each pass opens by stripping album values from rows whose album no
longer has that many members, or now has a tagged or unmeasured one, which sends a joined or
departed member's album back to the work list. Re-measuring means decoding the album again, for
the energies reason above; an album gaining a track is rare next to the backlog.

**Written as a unit.** An album's rows commit in one transaction or not at all, so a stop or a
failure mid-album leaves no half-measured album. The write skips a row whose hash moved or which
gained tags since its job was read.

### Reference, gain, peak and channels

- **Gain is `−18 LUFS − integrated loudness`**, the ReplayGain 2.0 reference, so a measured track
  and one tagged to the same standard end up equally loud. The reference gets one home in
  `melodia-core`, beside R128's −23 LUFS; the store's `R128_TO_REPLAYGAIN_DB`
  (`metadata.rs:192-195`) becomes the difference of the two instead of its own literal.
- **A non-finite loudness is no result.** Silence and audio under one gating block answer `-inf`,
  and NaN input answers NaN. Checked before any arithmetic, so no `+inf` or sentinel-derived gain
  reaches the table.
- **The peak is a sample peak**, the loudest sample over all channels, linear with 1.0 at full
  scale, as the tag columns store it. The clipping guard caps a boost at `1 / peak`, which is what
  keeps the samples Melodia produces under full scale, and a sample peak bounds exactly that; the
  soft-knee limiter (`equalizer.rs:50-55`) still runs behind any gain that isn't unity. A true peak
  would run an oversampling filter over every sample of every channel for the pass's whole length.
  ⚠️ **re-verify** which peak common tagging tools write by default, so a measured track's guard caps
  where a tagged one's does.
- **The channel map comes from the decoder's channel positions**, never the crate's default map,
  which drops a 7.1 file's side pair and mislabels 2.1 and 3.1: LFE unused, side and rear channels
  at their surround weighting, front and centre at unity. A file whose positions are unknown takes
  the default map by count.
- **Mono is measured as dual mono.** A mono file plays on both speakers, so it is heard as two
  identical channels; measured as one, it would be levelled 3 dB hotter than the same audio in
  stereo. ⚠️ **re-verify** that the output maps a mono source to both channels, and compare one
  mono file's measurement against the same file tagged by a tagging tool.

### Measured through the playback decoder

The pass decodes through `FileDecoder::open_track` (`melodia-audio/.../file_decode.rs:186-195`),
the decoder a deck plays from, so it measures exactly the audio that plays:

- AAC priming and padding, Matroska discard padding and Opus pre-skip come off (`install_trim`);
- a cue sheet track decodes its span alone (`enter_span`);
- an Opus file's header gain is applied (`opus.rs:222-226`), which is what R128 tags are relative
  to, so a measured Opus file and an R128-tagged one agree.

Two things the iterator keeps to itself today, both added `pub(crate)` without changing playback:

- **Why it ended.** `Cursor::next_sample` treats a failed read, a clean end and a shape change as
  one end (`decode.rs:286-297`), which is right for a deck. A measurement must not store a
  truncated decode as complete, so the cursor records which end it reached and the decoder answers
  it.
- **The channel positions**, kept from the codec parameters beside the count `Shape` already
  carries (`decode.rs:333`).

### Play time: measured values ride the tag columns' path

- **The summary query picks the gains.** `get_track_summaries_by_ids` (`queries/track/lookup.rs:88-93`)
  is the one read of `TRACK_SUMMARY`. Its statement left-joins `track_loudness` on the key and the
  untagged condition, so the four gain fields carry the tags where the file has them and the
  measurement otherwise, and a new `replaygain_origin` column says which. Everything downstream,
  the queue window, `begin_track`, seek, crossfade and gapless preload, already carries those four
  fields into `EqSource` unchanged.
- **`TrackSummary` gains `replaygain_origin: GainOrigin { Tagged, Measured }`**, an enum in
  `melodia-core` with its `FromSql`, rather than a bare flag at the boundary.
  `TrackReplayGain::from(&TrackSummary)` carries it.
- **The switch gates measured gains at the DSP, live.** `ReplayGainShared` gains a `measured`
  atomic beside `enabled`, set at boot and by the switch, bumping the generation like the other
  controls. A measured origin with it off is unity, today's untagged behaviour, so turning the
  switch off re-levels the playing track at the next frame boundary and the rows stay for turning it
  back on. `compute_linear_gain` already takes four parameters, so it takes a snapshot of the
  controls (`mode`, `preamp`, `prevent_clipping`, `measured`) instead of a fifth.
- **The full `Track` entity stays tag-only**, so Edit Tags and the track's properties show what the
  file says.

### The pass

**When it runs.** While ReplayGain and the switch are both on, the user hasn't stopped it this
session, and no library scan is running. ReplayGain off (by hand or by bit-perfect's reset) stops
it: measuring for a disabled feature only spends CPU.

**What wakes it.** Event-driven, never polled:

- the settings kick, after the switch's or ReplayGain's persist commits;
- `library_changed`, which scans finishing, watcher batches, imports and edits already bump, so new
  tracks are measured as they arrive;
- the scan going idle;
- Resume.

Each wake re-reads the work list, which is cheap when empty; the pass runs until it is empty or
something stops it.

**It steps aside for a scan.** A scan reads the same disks and needs the one writer. The scan's
progress channel can't say whether one is running: a reconcile pass clears it between folders
(`library/scan/reconcile.rs:235`, `run.rs:107-111`). So `ScanControl` gains a busy count, held by an
RAII guard that `reconcile::run_pass` and `scan_folder` take for their whole run, published on a
`watch`. When it goes busy the pass stops mid-album, writing nothing for that album, and the busy
count dropping to zero wakes it again.

**It steps aside for playback through the scheduler.** The work runs on a dedicated pool whose
threads lower their own priority as they start, so the decode-ahead thread, the output and the UI
take the CPU first, and on Linux the I/O priority follows the nice value under the CFQ and BFQ
schedulers (⚠️ **re-verify** against the kernel's ioprio documentation). Dedicated, because a
priority set on a blocking-pool thread outlives the job and lowers whatever runs there next, and
`ScanPool` takes every core. The pool is `MEASURE_WORKERS` wide, built per pass and dropped with it,
so no thread idles after the backlog. Its width is settled on the big library (Phase 6).

- Linux: `rustix::process::setpriority_process` on the thread's own id, which on Linux sets that
  thread alone. `rustix` is already in the tree (`melodia-playback`), with no new `unsafe`.
- Windows: `SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)` through
  `windows-sys`, which lowers CPU and I/O priority together. A new FFI call of the kind
  `.claude/rules/unsafe-rust.md` already sanctions.
- macOS: unchanged. A thread-level call there needs FFI the platform crate doesn't carry; the
  narrow pool still leaves cores free.

**Stop holds for the session.** Stop cancels the pass, and nothing restarts it, new tracks
included, until Measure is pressed, the switch is turned off and on, or the app is next launched.
"Can be stopped" means the user decides when it runs again.

**Progress** is tracks decoded out of tracks the pass will decode, an album re-read counting all
its members, published on a `watch` the Settings card paints.

**A measurement reaches the queue.** Each committed job bumps a narrow `gains_measured` signal
that the queue window task listens to beside its three others, refetching its window
(`WindowFill::All`), so a queued track measured mid-pass plays measured. Not `library_changed`,
which thirteen subscribers re-read on.

**Memory.** Per worker: one decoder, one track meter, and one album meter per shape. Work lists are
keyset pages, never a library-sized `fetch_all`. Nothing is held between passes but the table.

---

## What this does not do

- **No file is ever written.** Writing measurements into tags would let other players use them,
  but it changes the user's files, so if it is ever offered it is an action of its own.
- **Tagged files are not measured**, a file with either gain tag included.
- **Radio has nothing to measure** and is untouched.
- **A file opened from outside the library is not measured**: the work list takes `IN_LIBRARY`.
- **No true peak and no loudness range.**
- **A file that fails to measure is not retried until it changes.**
- **Nothing is shared with #108's seek-bar shape yet.** That issue also decodes every file once.
  The pass's per-track decode feeding a meter is the seam: if #108 lands, its shape can read the
  same decode then, rather than an abstraction being built for it now.
- **macOS gets no lowered priority.**

---

## Structure

| what | where | note |
|---|---|---|
| reference levels | `crates/melodia-core/src/entities/loudness.rs` | `REPLAYGAIN_REFERENCE_LUFS`, `R128_REFERENCE_LUFS`, `GainOrigin` with its `FromSql` and a `param_from!` line in `sql.rs`. The store's R128 offset derives from the two |
| the decoder's end and positions | `crates/melodia-audio/src/player/source/{decode,file_decode}.rs` | `pub(crate)`; the iterator ends as it does today |
| the meter | `crates/melodia-audio/src/loudness/` | `channels.rs`: positions to an `ebur128` channel map, mono as dual mono. `meter.rs`: the track meter and the album meters per shape, fed in blocks from the decoder. `mod.rs`: `measure_album` and `measure_track`, each polling a stop check per block and answering a typed outcome per track |
| the table | `migrations/<timestamp>_track_loudness.sql` | the schema above |
| store units of work | `crates/melodia-store/src/database/queries/loudness.rs` | the album settle, the two keyset work lists, the pending count, the album and track writes, and `carry_over` (`pub(crate)` on the self-write paths' transactions) |
| the summary read | `queries/track/lookup.rs` | one interned statement; `TRACK_SUMMARY` loses the four gain names to it |
| the gate at the DSP | `crates/melodia-playback/src/player/playback/replaygain.rs` | `TrackReplayGain.origin`, `ReplayGainShared::set_measured`, the controls snapshot |
| live setter | `crates/melodia-engine/.../backend/controls.rs`, `crates/melodia-app/src/library/playback.rs` | beside the other ReplayGain setters |
| the setting | `crates/melodia-app/src/services/settings/playback.rs` (`ReplayGainFlags.rg_measure_untagged`), `library/settings/replaygain.rs` | `serde(default)` off |
| scan busy | `crates/melodia-app/src/state/scan.rs`, `library/scan/{reconcile,mod}.rs` | the count, its guard, the two holders |
| pass state | `crates/melodia-app/src/state/loudness.rs` | `LoudnessControl`: progress `watch`, stop and resume, the kick and `gains_measured` signals |
| the pass | `crates/melodia-app/src/tasks/loudness/` | `mod.rs`: the wake loop. `pass.rs`: settle, count, page, dispatch, write, publish. `pool.rs`: the dedicated pool and its width |
| lowered priority | `crates/melodia-platform/src/services/platform/background_priority.rs` | one function, `cfg_select!` per OS |
| Settings card | `crates/melodia-ui/ui/views/settings/replaygain-section.slint`, `globals/audio.slint`, `components/settings/replaygain-controls.slint` | the row and the progress row; strings on `ReplayGainText` |
| Settings wiring | `crates/melodia-views/src/ui/replaygain/` | `mod.rs`: today's `install_replaygain`. `measure.rs`: the switch, the progress painter, stop and resume |

`melodia-views` learns nothing about the table: it reads `LoudnessControl` through `melodia-app`.
The store learns nothing about decoding beyond the edge it already has (`melodia-audio`).

**The duplication this collapses:**

- **`ScanProgress` and the new progress row are one layout**: a bar, a status line and one action
  button. `ScanProgress` (`components/settings/scan-progress.slint`) splits into a generic
  `JobProgress` taking `value`, `status`, `action-label`, `action-enabled` and `action()`, and a
  `ScanProgress` that binds it to `LibrarySettings`, mounted where it is today. Landed before the
  feature, with no behaviour change.
- **The R128 offset's literal** becomes the difference of the two references (above).
- **`rustix`** moves to a workspace dependency, its one line shared by `melodia-playback` and
  `melodia-platform`.

---

## Phase 1: Groundwork (no behaviour change)

- [ ] `entities/loudness.rs` with the two references; `metadata.rs`'s R128 offset derives from them.
- [ ] `JobProgress` extracted from `ScanProgress`, `ScanProgress` a binding of it, both mounts
      (`library-section.slint:70`, `music-panel.slint:20`) unchanged.
- [ ] `rustix` moved to the workspace's dependencies.

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass: a scan from Settings and from onboarding shows and cancels as before.

## Phase 2: Measuring an album (`melodia-audio`)

- [ ] `ebur128` 0.1.11 in the workspace and `melodia-audio`'s manifest (`cargo search` again on the
      day). 0.1.11 makes results independent of how input is split on non-x86 targets, which a
      block-fed meter relies on. `dasp_frame` is the one crate it adds to the lock file.
- [ ] `Cursor` records how it ended; `FileDecoder` answers that and the channel positions.
- [ ] `loudness/`: the channel map, the meters and `measure_album` / `measure_track`, with the
      outcomes:
  - [ ] `Measured { gain, peak }` per track, from the reference and a finite loudness;
  - [ ] no result for silence, a short file, a failed read, a shape change;
  - [ ] an album gain only with every member read to its end and some block above the gate;
  - [ ] a stop returns `Stopped`, no partial result.
- [ ] The crate's doc comments carry the dual mono, sample peak and album meter arguments.

**Gate:** the static gates. Nothing calls it yet, so the run gates wait for Phase 4.

## Phase 3: Stored measurements play like tags

- [ ] The migration.
- [ ] `GainOrigin`, `TrackSummary.replaygain_origin`, the summary statement.
- [ ] `TrackReplayGain.origin`, `ReplayGainShared::set_measured`, the controls snapshot in
      `compute_linear_gain` and its caller (`equalizer.rs:429-440`).
- [ ] The engine and app setters; `hydrate_audio_dsp` (`state/mod.rs:321-337`) seeds it.
- [ ] `ReplayGainFlags.rg_measure_untagged` and its setter.
- [ ] `carry_over` called from `commit_retag` (`queries/scan/retag.rs`) and the sheet-tags embedded
      write (`library/sheet_tags.rs`).
- [ ] Docs: `.claude/rules/library-data.md` gains "a path where Melodia writes only tags into a file
      carries its loudness row (`queries::loudness::carry_over`), or the next pass decodes its
      album again", violable from any new self-write path.

**Gate:**
- [ ] The static gates, and the existing tests fixed where the new field or column breaks them.
- [ ] In the dev data folder, with a backup: rows inserted by hand with `/usr/bin/sqlite3` for one
      untagged album, matching each track's hash and span, then:
  - [ ] with `rg_measure_untagged` set in `settings.json`, the album plays levelled in Track and
        Album mode, and the signal path still shows ReplayGain on;
  - [ ] with it unset, the album plays at unity and tagged albums are unchanged either way;
  - [ ] an Edit Tags save and a rating write-back on one of those tracks leave its row matching;
  - [ ] a file replaced on disk stops matching once the watcher has re-read it.

## Phase 4: The pass

- [ ] `LoudnessControl` and its signals on `AppState`.
- [ ] The scan busy count and guard, taken by `run_pass` and `scan_folder`.
- [ ] `queries/loudness.rs`: the settle, the album and track work lists (keyset, `IN_LIBRARY`,
      untagged, hashed), the count and the two writes, each write skipping rows whose hash moved
      or which gained tags.
- [ ] `background_priority.rs`; the Windows call joins `unsafe-rust.md`'s table and counts.
- [ ] `tasks/loudness/`: the wake loop, the pass and the pool. Threads named `loudness-{i}`, listed
      in `packaging.rs`'s `RUNTIME_NAMED` with their width argued.
- [ ] Spawned with the other tasks (`boot/tasks.rs`); the switch's and ReplayGain's persists kick
      it.
- [ ] The queue window's `gains_measured` arm (`tasks/queue_window.rs`).
- [ ] Logging: a pass's start, end, counts and duration at `info`; each album's path, loudness and
      gain at `debug`, for the gate's comparison.

**Gate:**
- [ ] The static gates.
- [ ] In the dev data folder, the switch on through `settings.json`:
  - [ ] the backlog is measured and the pass ends; a relaunch finds nothing to do;
  - [ ] measured gains agree with an independent reference, `ffmpeg`'s `ebur128` filter with
        sample peaks, over a handful of files across codecs, an Opus file and a cue sheet track
        among them;
  - [ ] tagged copies with their tags stripped (copies only) measure to what their tags said; the
        differences are read and explained before the phase closes;
  - [ ] the mono check from Decisions;
  - [ ] a scan started mid-pass stops the pass, which resumes once the scan ends;
  - [ ] a new file dropped into a watched folder is measured on its own;
  - [ ] RSS sampled from `/proc` during the pass, and idle RSS after it, against a run with the
        switch off.

## Phase 5: The switch, progress and Stop in Settings

- [ ] `ReplayGain` global: `measure-untagged` and its callback, the progress state (running,
      waiting for a scan, stopped with work left, idle), done and total, `stop-measuring()` and
      `resume-measuring()`.
- [ ] `replaygain-section.slint`: the switch row while ReplayGain is on, and a `JobProgress` under
      it while a pass runs or stopped work remains. The card only: the overflow menu's dialog stays
      the live listening controls.
- [ ] The card's description loses its claim that only tagged tracks are affected; its pointer to
      tagging tools stays.
- [ ] `ui/replaygain/`: `toggle_binding` for the switch (setter, then persist, then the kick on
      `Ok`), the progress painter on the `watch`, Stop and Measure.
- [ ] Strings through `@tr`, the counts as plurals; `scripts/update-translations.py` and every
      catalog finished.
- [ ] `README.md`'s ReplayGain lines (`:17`, `:128`).

**Gate:**
- [ ] The static gates.
- [ ] Kenan's hand pass: the switch on with a library in place shows progress; Stop holds through a
      new file arriving; Measure resumes; the switch off stops the pass and returns to tag-only
      levels; the settings search finds the row.

## Phase 6: The big library

- [ ] A whole-library pass on the big test library, measured and recorded here:
  - [ ] wall time, CPU, peak RSS sampled from `/proc` (never heaptrack on a pass this size), idle
        RSS after;
  - [ ] `MEASURE_WORKERS` decided from those figures, its doc saying what it trades;
  - [ ] whether the pass evicts the database's pages from the page cache (the UI slower after a
        pass than before). If it does, each file's pages are dropped once it is measured
        (`posix_fadvise(DONTNEED)` through `rustix` on Linux), and the check runs again.
- [ ] The issue's hand list:
  - [ ] a library mixing tagged and untagged albums, in Album mode;
  - [ ] an album with a single untagged track;
  - [ ] a file replaced while a pass runs;
  - [ ] music playing through a large pass, gapless and crossfaded, with no dropout.

## Phase 7: Tests

Tests are added where the landed change earns them, on Kenan's go, after the hand passes above.
Existing tests a phase breaks are fixed in that phase.

---

## Cross-cutting

- **Docs land in the phase that stales them**, held to the anchor rule: the arguments above move into
  the doc comments of `loudness/`, `queries/loudness.rs`, `tasks/loudness/` and the migration's
  comments as each lands.
- **800 lines:** `metadata.rs` (597) and `lookup.rs` (507) gain lines, not bodies; everything new is
  its own module.
- **Memory:** `TrackSummary` gains one enum, and it lives only in the player's window. The pass's
  cost is bounded per worker and gone when the pass ends; Phase 4 and Phase 6 measure both.
- **Lock order** (ADR 0035) is untouched: the pass takes no player lock, and its measurements reach
  the player through the queue window's existing fetch.
- **The live public release:** the setting defaults off, the new JSON field is `serde(default)`, and
  the migration only adds a table.

## Open questions

- `MEASURE_WORKERS`, settled in Phase 6.
- Whether Stop should hold across launches. This plan holds it for the session.
