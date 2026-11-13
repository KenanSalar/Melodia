# ReplayGain Measurement

Working doc. Delete when the feature ships.

Status: **Phases 1–4 landed**, Phase 5 built and hand-passed, Phase 6 next; Phase 4's Windows
build check pending · Created: 2026-11-12

Issue [#156](https://github.com/KenanSalar/Melodia/issues/156).

> Verified **2026-11-12** against this tree at `00b74796`, the `ebur128` 0.1.11 source, and the
> kernel's `include/linux/ioprio.h` and `block/mq-deadline.c`.

---

## The symptom

ReplayGain evens out the volume between tracks only when their files carry gain tags. A track with
none plays at its mastered level: `compute_linear_gain` returns unity when there is no gain to read
(`melodia-playback/src/player/playback/replaygain.rs:102-137`). So a library where only some
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
- A replaced file stops using its old measurement as soon as the library re-reads it, and is
  measured again. A track that leaves the library takes its measurement with it.
- Turning the switch on with a library in place measures the backlog in the background. After
  that, new tracks are measured as they arrive.
- While a pass has work, the albums of the queued tracks around the playing one that have no gain
  go first.
- A pass shows its progress under the switch and can be stopped.

**What keeps working:**

- Tagged tracks sound exactly as they do now.
- With the switch off, the default, nothing is measured and playback is today's: no pass, pool or
  thread exists and the pass does no database work (**Off costs nothing**, below). Stored
  measurements stay in the library but are not applied, so turning it back on is instant.
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
    -- set together, once the whole album was measured: the album and its member count then
    album_id INTEGER,
    album_tracks INTEGER,
    album_gain REAL,
    album_peak REAL
);
```

**Not the four tag columns.** Every rescan of a changed file, every tag edit and the tag backfill
rewrites them from the file, `NULL` included (`TAG_BACKED_SET`,
`queries/scan/mutations.rs:296-315`), so a measurement written there would be wiped by the next
re-read. They also have to keep saying what the file says, since "a file that gains tags switches
to its own" is read from them.

**Valid only while its key matches the row.** A read uses a measurement only where `file_hash`,
`cue_start` and `cue_end` equal the track's current values, compared with `IS`: the span is `NULL`
on a whole-file row, and `cue_end` on a sheet's last track. The hash is the whole-file BLAKE3
every re-read writes (`metadata.rs:24-34`), so:

- a replaced file stops matching the moment its re-read commits, with no write path having to
  remember an invalidation. A file renamed over a track's path, which a temp-file save does, is
  not re-read until the next scan (`watch_batch::handle_created` leaves a path already holding
  rows alone), so it keeps its old tags and measurement until then: a watcher gap of its own,
  outside this plan;
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
every rated track's album back to be decoded again. Melodia writes only tags into a file on two
paths, and each moves `file_hash` from the old value to the new one on rows that matched the old:

- `commit_retag`, which every tag write goes through (Edit Tags with its artwork, the rating
  write-back, the lyrics embed), inside its transaction: the old hash is read with the row ahead of
  its update and carried right after it, the statement the sheet-tags path shares;
- the sheet-tags write into a FLAC's embedded `CUESHEET`, which has no transaction of its own: its
  read-back goes through the general ingest (`library/sheet_tags.rs`, `read_back`), and that never
  carries a measurement, since a file another program replaced reaches it the same way. So it
  reads the image's hash before writing and carries the rows right after the read-back commits. A
  pass landing in between measures that album again, which costs a decode, never a wrong level. A
  `.cue` write changes no audio file.

A tag edit by another program changes the hash the same way and is indistinguishable from a
replaced file, so that track is measured again, at the same cost.

**A file that can't be measured is recorded, not retried; one out of reach waits.** Silent audio,
a file under one gating block, a shape change mid-file and a file that is there but fails to open
or to read through all write a row with `NULL` gains. The key makes it retry when the file changes,
and never on every pass, which would decode the same broken file on every wake. A failure, at the
open or part-way, on a file that isn't there afterwards (an unmounted drive, told apart with
`try_exists`, an error from it counting as not there) writes nothing: it may come back, so it is
skipped for the session and tried again at the next launch, as `embedded_cue_probe` treats a file
out of reach. Recorded, it would never be measured again, its hash unchanged.

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
exact (no histogram), its history bounded by the one file. Members are fed in Album Detail's order
(disc, then track, a track numbered 0 or none last, as a cue sheet's hidden track 0 is), so the
blocks across a join hear it as the album plays through, which is what "as one whole" means.
An album whose members differ in rate or channel count gets one album meter per shape, combined
through `loudness_global_multiple`, which accepts histogram states. The album meter carries no peak
mode: the album peak is the loudest member's sample peak, read off the track meters. Feeding two
meters runs the K-weighting filter twice over every sample; Phase 6 measures that share of the
pass's CPU against decoding.

**An album is complete only when every in-library member is untagged and measured**, its member
count matching `album_tracks`. Then:

- a member that couldn't be measured leaves the album without an album gain, and its siblings fall
  back to their track gain, as a file carrying only track tags does today;
- a silent member counts as measured, its blocks falling under the absolute gate like any quiet
  passage;
- an album with a tagged member never gets a measured album gain, and its untagged members get
  track gains, the scope ruling out re-measuring tagged files.

**The album values sit on each member's row, with the album and the count they were measured
over.** A read is then one primary-key join, and it applies the album values only while the row's
`album_id` is still the track's. Each pass opens with the settle (`queries::loudness::settle`): one
grouped read finds the albums whose held values no longer describe them (gone, a different size,
holding a tagged, unhashed or unmatched member, or a member carrying another album's values), and
only then does one write strip them, which sends a joined or departed member's album back to the
work list while the common wake never takes the writer. Re-measuring means decoding the album
again, for the energies reason above; an album gaining a track is rare next to the backlog.

The count alone misses a swap: two tracks retagged across two albums of one size each carry their
old album's values through `carry_over`, and both counts still match. The id catches it, and the
count stays beside it because a pruned album's id can be reused (`prune_orphans`). No foreign key:
an album's delete would scan this table for want of an index.

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
  Tagging tools write a sample peak by default too, a true peak being their opt-in, so a measured
  track's guard caps where a tagged one's does.
- **The channel map comes from the decoder's channel positions**, never the crate's default map,
  which drops a 7.1 file's side pair and mislabels 2.1 and 3.1. LFE is unused; side and back pairs
  take the ±110° surround weighting; front, centre, front-of-centre and height channels sit at
  unity. Side and back weigh alike because a 5.1 file labels its surround pair either way. The
  crate's ±135° and 180° positions weigh unity, BS.1770's rear, so a back pair maps to the ±110°
  positions, never to those. A buffer with no positions (`Channels::Discrete`, `Ambisonic` or
  `Custom`) takes the default map by count. A multichannel file is measured as authored, as a tag
  is; what a narrower output plays of it is the output's fold (`output/convert.rs` keeps the first
  channels), outside this plan.
- **Mono is measured as dual mono.** The output copies a mono source to both front channels at
  unity (`output/convert.rs:341-348`), so it is heard as two identical channels; measured as one, it
  would be levelled 3 dB hotter than the same audio in stereo. Tagging tools measure mono as one
  channel unless their dual-mono option is set, so a mono file they tagged with their defaults
  plays 3 dB hotter than one measured here; matching the same audio in stereo is the point.

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
  it. A packet the decoder rejects is skipped and the stream goes on (`decode.rs:345`); playback
  skips it too, so the measurement stands.
- **The channel positions**, read from the opening packet's decoded spec, the source the count in
  `Shape` already comes from (`decode.rs:332-341`), and kept as `Position` bits beside the cursor's
  `Shape`, never in it: `Shape`'s equality decides a gapless join (`output/voice.rs:506`), a
  mount's reconnect (`stream_source.rs:627`) and the mid-track end (`decode.rs:294`), and the
  output builds one with no positions to give. Not from the codec parameters, which can lack them
  or disagree with the buffer, and not as `Channels`, which isn't `Copy`.

### Play time: measured values ride the tag columns' path

- **The summary query picks the gains.** `get_track_summaries_by_ids` (`queries/track/lookup.rs:88-93`)
  is the one read of `TRACK_SUMMARY`. Its statement left-joins `track_loudness` on the key and the
  untagged condition, so the four gain fields carry the tags where the file has them and the
  measurement otherwise, and a new `replaygain_origin` column says which. Everything downstream,
  the queue window, `begin_track`, seek, crossfade and gapless preload, already carries those four
  fields into `EqSource` unchanged. The statement up to its `WHERE`, projection and join, is one
  interned string handed to `select_in_id_order` in place of today's `SELECT {cols} FROM tracks`,
  its columns qualified (`TRACK_SUMMARY.prefixed`) since the two tables share the key's column
  names.
- **A track measured while it plays keeps the level it started with** until it is next started or
  seeked: a source bakes its gains when it is built, and a refetch replaces only the record.
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

### Off costs nothing

The switch is off by default, and off means:

- no pass, pool or thread: the pool is built per pass, and the wake loop is one parked task waiting
  on the kick alone, listening to no library or scan signal;
- no database work from the pass: no settle, no work list, no count;
- the queue window publishes no upcoming ids, and nothing bumps `gains_measured`;
- measured gains play at unity, gated at the DSP, so playback is today's.

Three things run whatever the switch says: the summary read's join and `carry_over` on a tag write,
each a primary-key lookup into a table that stays empty until the switch is first turned on, and
the scan's busy count, a counter each scan moves twice. `carry_over` keeps a turned-off library's
measurements matching, so turning the switch back on is instant, and the join is what lets the DSP
gate re-level the playing track the moment the switch moves.

### The pass

**When it runs.** While ReplayGain and the switch are both on, the user hasn't stopped it this
session, and no library scan is running. ReplayGain off (by hand or by bit-perfect's reset) stops
it: measuring for a disabled feature only spends CPU. The pass reads the engine's live controls,
ReplayGain's `enabled` and the switch's `measured`, at each block beside its stop check. A
bit-perfect reset turns ReplayGain off live through `player_set_replaygain_enabled` as well as in
`settings.json` (`apply_bit_perfect_reset`), and the live controls are what the pass reads.

**What wakes it.** Event-driven, never polled:

- launch, so a backlog a quit cut short resumes;
- the kick, from the switch's and ReplayGain's handlers right after their live set. Nothing the
  pass reads is on disk, so no persist sits between;
- `library_changed`, which scans finishing, watcher batches, imports and edits already bump, so new
  tracks are measured as they arrive;
- the scan going idle;
- Resume.

While the switch is off it waits on the kick alone. Each wake re-reads the work list, which is
cheap when empty; the pass runs until it is empty or something stops it.

**The queue goes first.** While measured gains are heard, the queue window task offers the ids of
the window's tracks that no gain reaches (the slot behind, the playing one and the three ahead,
`queue/mod.rs:12-17`) on a small `watch` in `LoudnessControl`, and offers them again on a kick, so
switching measuring on mid-track puts the playing album first. At each job boundary the pass takes
the work-list albums, and albumless tracks, among them before its next keyset item, and a per-pass
set of those jobs makes the keyset skip them when it gets there. A job that lands drops its queued
copy, so one the keyset took first isn't measured twice. During a long first backlog,
what is about to play is levelled first; once the backlog is done it does nothing, new tracks
being measured as they arrive.

**It steps aside for a scan.** A scan reads the same disks and needs the one writer. The scan's
progress channel can't say whether one is running: a reconcile pass clears it between folders
(`library/scan/reconcile.rs:235`, `run.rs:107-111`). So `ScanControl` gains a busy count, a
`HoldCount` held by an RAII guard. `reconcile::run_pass` takes its guard before its task spawns,
and `scan_folder` holds one for its whole run, so its hand-off to the reconcile it starts
(`library/scan/mod.rs:109-116`) leaves no idle gap. Watcher batches and imports hold none: they are
short, and the pass's write already skips a row whose hash moved under it. When it goes busy the
pass stops mid-album, writing nothing for that album, and the busy count dropping to zero wakes it
again.

**It steps aside for playback through the scheduler.** The work runs on a dedicated pool whose
threads lower their own priority as they start, so the decode-ahead thread, the output and the UI
take the CPU first. Dedicated, because a priority set on a blocking-pool thread outlives the job and
lowers whatever runs there next, and `ScanPool` can take every core. The pool is `MEASURE_WORKERS`
wide, built at a pass's first job and dropped with the pass, so no thread idles after the backlog
and a wake whose work is all out of reach starts none, logging nothing at `info`. Its width is
settled on the big library (Phase 6).

- Linux: `rustix::process::setpriority_process` on the thread's own id, which on Linux sets that
  thread alone. `rustix` is already in the tree (`melodia-playback`), with no new `unsafe`. A thread
  with no I/O priority of its own takes best-effort at a level its nice value sets
  (`include/linux/ioprio.h`). BFQ honours that level, mq-deadline only the class, which nice leaves
  at best-effort, and `none`, NVMe's default, has no priorities. The idle class needs `ioprio_set`
  or `SCHED_IDLE`, neither of which `rustix` wraps, and `libc` is the allocator's alone, so the
  pass's reads go unthrottled outside BFQ; Phase 6's no-dropout check covers that.
- Windows: `SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)` through
  `windows-sys`, which lowers CPU and I/O priority together. A new FFI call of the kind
  `.claude/rules/unsafe-rust.md` already sanctions; `Win32_System_Threading` is already enabled.
- macOS: unchanged. The thread-level call is `pthread_set_qos_class_self_np`, reachable only
  through `libc` (the allocator's alone) or a hand-written `extern` block, which
  `.claude/rules/unsafe-rust.md` rules out; the narrow pool still leaves cores free.

**Stop holds for the session.** Stop cancels the pass, and nothing restarts it, new tracks
included, until Measure is pressed, the switch is turned off and on, or the app is next launched.
"Can be stopped" means the user decides when it runs again.

**Progress** is tracks decoded out of tracks the pass will decode, an album re-read counting all
its members, published on a `watch` the Settings card paints.

**A measurement reaches the queue.** Each committed job bumps a narrow `gains_measured` signal
that the queue window task listens to beside its three others, refetching its window with
`WindowFill::All` (`Missing` fetches nothing, the ids being in the window already), so a queued
track measured mid-pass plays measured. Not `library_changed`, which every library view re-reads
on.

**Memory.** Per worker: one decoder, one track meter, and one album meter per shape. Work lists are
keyset pages, never a library-sized `fetch_all`. Nothing is held between passes but the table, and
a pass that did work trims the allocator as it ends, as a reconcile does: its decoders free after
the startup trims have run.

---

## What this does not do

- **No file is ever written.** Writing measurements into tags would let other players use them,
  but it changes the user's files, so if it is ever offered it is an action of its own.
- **Tagged files are not measured**, a file with either gain tag included.
- **Radio has nothing to measure** and is untouched.
- **A file opened from outside the library is not measured**: the work list takes `IN_LIBRARY`.
- **No true peak and no loudness range.**
- **A file that fails to measure is not retried until it changes**; one out of reach is tried
  again at the next launch.
- **Nothing is shared with #108's seek-bar shape yet.** That issue also decodes every file once.
  The pass's per-track decode feeding a meter is the seam: if #108 lands, its shape can read the
  same decode then, rather than an abstraction being built for it now.
- **macOS gets no lowered priority, and Linux no lowered I/O priority outside BFQ.**

---

## Structure

| what | where | note |
|---|---|---|
| reference levels | `crates/melodia-core/src/entities/loudness.rs` | `REPLAYGAIN_REFERENCE_LUFS`, `R128_REFERENCE_LUFS`, `GainOrigin` with its `FromSql`, read from whether a measurement stood in, so nothing binds one. The store's R128 offset derives from the two |
| the decoder's end and positions | `crates/melodia-audio/src/player/source/{decode,file_decode,audio}.rs` | `pub(crate)`; the iterator ends as it does today; `Position` bits beside the cursor's `Shape` |
| the meter | `crates/melodia-audio/src/loudness/` | `channels.rs`: positions to an `ebur128` channel map, mono as dual mono. `meter.rs`: the track meter and the album meters per shape, fed in blocks from the decoder. `mod.rs`: `measure_album` and `measure_track`, each polling a stop check per block and answering a typed outcome per track, out of reach among them |
| the table | `migrations/<timestamp>_track_loudness.sql` | the schema above |
| store units of work | `crates/melodia-store/src/database/queries/loudness/` | `mod.rs`: `MATCHING_MEASUREMENT`, the key's one home, the job types, and `carry_over` (`pub(crate)` for `commit_retag`'s transaction; `hashes_on_record` and `carry_over_rewritten` for the sheet-tags read-back). `work.rs`: the album tally, the settle, the two keyset work lists, the queue-first lookup and the pending count. `record.rs`: the album and track writes |
| the summary read | `queries/track/lookup.rs` | the statement up to its `WHERE`, interned and handed to `select_in_id_order`; `TRACK_SUMMARY` loses the four gain names to it |
| the gate at the DSP | `crates/melodia-playback/src/player/playback/replaygain.rs` | `TrackReplayGain.origin`, `ReplayGainShared::set_measured`, the `RgControls` snapshot |
| live controls | `crates/melodia-engine/.../backend/controls.rs`, `crates/melodia-app/src/library/playback.rs` | the setter beside the other ReplayGain setters, and `levels_measured`, the one read the pass polls (`enabled` and `measured` together) |
| the setting | `crates/melodia-app/src/services/settings/playback.rs` (`ReplayGainFlags.rg_measure_untagged`), `library/settings/replaygain.rs` | `serde(default)` off |
| scan busy | `crates/melodia-app/src/state/{scan,signal}.rs`, `library/scan/{reconcile,mod}.rs` | the count, a `HoldCount` and its `Hold`, the two holders |
| pass state | `crates/melodia-app/src/state/loudness.rs` | `LoudnessControl`: progress `watch`, the queued-ids `watch`, stop and resume, the kick and `gains_measured` signals |
| the pass | `crates/melodia-app/src/tasks/loudness/` | `mod.rs`: the wake loop. `pass.rs`: settle, count, the queue-first jobs, page, dispatch, write, publish. `pool.rs`: the dedicated pool and its width |
| the queue window | `crates/melodia-app/src/tasks/queue_window.rs`, `melodia-engine`'s `window_ungained` | the `gains_measured` arm, and the window's ungained ids offered while measured gains are heard |
| lowered priority | `crates/melodia-platform/src/services/platform/background_priority.rs` | one function, `cfg_select!` per OS |
| Settings card | `crates/melodia-ui/ui/views/settings/replaygain-section.slint`, `globals/audio.slint`, `components/settings/replaygain-controls.slint` | the row and the progress row; strings on `ReplayGainText` |
| Settings wiring | `crates/melodia-views/src/ui/replaygain/` | `mod.rs`: `install_replaygain`. `measure.rs`: the switch, the progress painter, stop and resume |

`melodia-views` learns nothing about the table: it reads `LoudnessControl` through `melodia-app`.
The store learns nothing about decoding beyond the edge it already has (`melodia-audio`).

**The duplication this collapses:**

- **`ScanProgress` and the new progress row are one layout**: a bar, a status line and one action
  button. `ScanProgress` (`components/settings/scan-progress.slint`) splits into a generic
  `JobProgress` taking `value`, `status`, `action-label`, `action-icon`, `action-enabled` and
  `action()`, and a `ScanProgress` that binds it to `LibrarySettings`, mounted where it is today.
  Landed before the feature, with no behaviour change.
- **The R128 offset's literal** becomes the difference of the two references (above).
- **`rustix`** moves to a workspace dependency, its one line shared by `melodia-playback` and
  `melodia-platform`.
- **The artwork restore notice and the scan busy count are one `HoldCount`** (`state/signal.rs`),
  `RestoreNotice` giving way to its `Hold`.

---

## Phase 1: Groundwork (no behaviour change)

- [x] `entities/loudness.rs` with the two references; `metadata.rs`'s R128 offset derives from them.
- [x] `JobProgress` extracted from `ScanProgress`, `ScanProgress` a binding of it, both mounts
      (`library-section.slint:70`, `music-panel.slint:20`) unchanged.
- [x] `rustix` moved to the workspace's dependencies.

**Gate:**
- [x] The static gates.
- [x] Kenan's hand pass: a scan from Settings and from onboarding shows and cancels as before.

## Phase 2: Measuring an album (`melodia-audio`)

- [x] `ebur128` 0.1.11 in the workspace and `melodia-audio`'s manifest (`cargo search` again on the
      day). 0.1.11 makes results independent of how input is split on non-x86 targets, which a
      block-fed meter relies on. `dasp_frame` is the one crate it adds to the lock file.
- [x] `Cursor` records how it ended; `FileDecoder` answers that and the channel positions, read
      from the opening packet's decoded spec into `Position` bits beside `Shape`.
- [x] `loudness/`: the channel map, the meters and `measure_album` / `measure_track`, with the
      outcomes:
  - [x] `Measured(Gain)` per track, from the reference and a finite loudness;
  - [x] no result for silence, a short file, a shape change, and a file that is there but fails
        to open or to read through;
  - [x] out of reach for a failure, at the open or part-way, on a file that isn't there
        afterwards, kept apart from no result;
  - [x] an album gain only with every member read to its end and some block above the gate;
  - [x] a stop returns `Stopped`, no partial result.
- [x] The crate's doc comments carry the dual mono, sample peak and album meter arguments.

**Gate:**
- [x] The static gates. Nothing calls it yet, so the run gates wait for Phase 4.

## Phase 3: Stored measurements play like tags

- [x] The migration.
- [x] `GainOrigin`, `TrackSummary.replaygain_origin`, the summary statement.
- [x] `TrackReplayGain.origin`, `ReplayGainShared::set_measured`, the controls snapshot in
      `compute_linear_gain` and its caller (`equalizer.rs:429-440`).
- [x] The engine and app setters and reads; `hydrate_audio_dsp` (`state/mod.rs:321-353`) seeds it.
- [x] `ReplayGainFlags.rg_measure_untagged` and its setter.
- [x] `carry_over` called from `commit_retag` (`queries/scan/retag.rs`) and after the sheet-tags
      embedded write's read-back (`library/sheet_tags.rs`), never from the general ingest.
- [x] Docs: `.claude/rules/library-data.md` gains "a path where Melodia writes only tags into a file
      carries its loudness row (`queries::loudness::carry_over`), or the next pass decodes its
      album again, and the general ingest never does", violable from any new self-write path.

**Gate:**
- [x] The static gates, and the existing tests fixed where the new field or column breaks them.
- [x] In the dev data folder, with a backup: rows inserted by hand with `/usr/bin/sqlite3` for one
      untagged album, matching each track's hash and span, then:
  - [x] with `rg_measure_untagged` set in `settings.json`, the album plays levelled in Track and
        Album mode, and the signal path still shows ReplayGain on;
  - [x] with it unset, the album plays at unity and tagged albums are unchanged either way;
  - [x] an Edit Tags save and a rating write-back on one of those tracks leave its row matching;
  - [x] an Edit Tags save moving one of them to another album stops it using the album gain at
        once;
  - [x] a file replaced on disk by a copy over it stops matching once the watcher has re-read it.

## Phase 4: The pass

- [x] `LoudnessControl` and its signals on `AppState`.
- [x] The scan busy count and guard, taken by `run_pass` before its task spawns and by
      `scan_folder` for its whole run.
- [x] `queries/loudness/`: the settle, the album and track work lists (keyset, `IN_LIBRARY`,
      untagged, hashed), the queue-first lookup, the count and the two writes, each write
      skipping rows whose hash moved or which gained tags. Query plans read on a copy of the dev
      database: the album page walks `idx_tracks_album_id` and stops at its limit.
- [x] `background_priority.rs`; the Windows call joins `unsafe-rust.md`'s table and counts.
- [x] `tasks/loudness/`: the wake loop, the pass with its queue-first jobs and session skip set,
      and the pool. Threads named `loudness-{i}`; the file naming them joins `packaging.rs`'s
      `RUNTIME_NAMED`, with their width argued.
- [x] Spawned with the other tasks (`crates/melodia/src/boot/tasks.rs`), running a pass at launch;
      ReplayGain's handler kicks it after its live set. The switch's handler, with its resume, is
      Phase 5's, the switch having no row before it.
- [x] The queue window's `gains_measured` arm and its queued ids (`tasks/queue_window.rs`).
- [x] Logging: a pass's start, end, counts and duration at `info`; each album's path, loudness and
      gain at `debug`, for the gate's comparison.

**Gate:**
- [x] The static gates.
- [ ] Windows: `background_priority.rs`'s `SetThreadPriority` call builds (no Windows target here).
- [x] In the dev data folder, the switch on through `settings.json` (2026-11-12, release build,
      the 525-track dev library plus scratch fixtures in two added folders, on a null sink;
      restored after):
  - [x] the backlog is measured and the pass ends; a relaunch finds nothing to do. 535 tracks in
        95 albums in 176 s with two workers; the relaunch logged no pass and built no pool;
  - [x] measured gains agree with an independent reference, `ffmpeg`'s `ebur128` filter with
        `peak=sample`, and `dualmono=true` for a mono file, over a handful of files across
        codecs, an Opus file, a cue sheet track and a multichannel file, where the library holds
        one, among them. A back centre (6.1) differs by design: ffmpeg weighs it 1.41
        (`BACK_MASK` in the installed 8.1's `f_ebur128.c`, `WEIGHT_1_41_MASK` on master),
        `channels.rs` at unity, BS.1770's weight at 180°. 8.1 also weighs the top back channels
        1.41, which master and `channels.rs` keep at unity as height channels. FLAC, Opus, M4A,
        raw AAC, MP3 stereo and mono, Vorbis, WAV, three cue spans and 5.1 in both its side and
        back layouts (fixtures, the library holding none) all read 0.002 to 0.006 LU above
        ffmpeg's last-frame running `I`, peaks equal to 1e-5;
  - [x] tagged copies with their tags stripped (copies only) measure to what their tags said; the
        differences are read and explained before the phase closes. No file on disk carries gain
        tags, so `rsgain` 3.6 (libebur128 1.2.6) scanned copies of the same files in scan-only
        mode, the values its `-s i` would write: within ±0.004 LU, its two decimals, except one
        MP3 at +0.011 LU, the two MP3 decoders. That MP3's peak reads 1.327 here and in ffmpeg
        and 1.000 in `rsgain`, which caps it; the decoded floats overshoot full scale and the
        clipping guard has to cap against what plays;
  - [x] a mono file and a stereo copy of it with both channels equal measure to the same gain
        (-15.421 LUFS each);
  - [x] a scan started mid-pass stops the pass, which resumes once the scan ends. A renamed
        directory's reconcile stopped it at 71.5 s, and the next pass took exactly the rest;
  - [x] bit-perfect turned on mid-pass stops it. Make Bit-Perfect, switching to exclusive, stopped
        it at 82.3 s, its committed jobs kept, and nothing restarted it;
  - [x] a new file dropped into a watched folder is measured on its own, by the pass its library
        change woke;
  - [x] with a backlog running, an untagged album the pass hasn't reached, once queued, is measured
        next, and only once. Album 88, opened from the command line while the keyset was on 45,
        committed right after it;
  - [x] a file on an unmounted drive writes nothing, and is measured at the next launch once the
        drive is back. A library folder whose root was renamed away stood in for the drive: the
        scan skips a missing root and keeps its rows;
  - [x] with the switch off, a launch and a library change run no pass: no pass line, no
        `loudness-` thread, no row;
  - [x] RSS sampled from `/proc` during the pass, and idle RSS after it, against a run with the
        switch off. RssAnon peaked at 38.4 MiB during the pass and idled at 29.7 MiB after it, as
        the switch-off run did. Before the pass trimmed the allocator at its end it idled 3.6 MiB
        higher, its decoders' frees landing after the startup trims; VmRSS idles 2.9 MiB higher,
        the page cache.

## Phase 5: The switch, progress and Stop in Settings

- [x] `ReplayGain` global: `measure-untagged` and its callback, the progress state (running,
      waiting for a scan, stopped with work left, idle), done and total, `stop-measuring()` and
      `resume-measuring()`.
- [x] `replaygain-section.slint`: the switch row while ReplayGain is on, and a `JobProgress` under
      it while a pass runs or stopped work remains. The card only: the overflow menu's dialog stays
      the live listening controls.
- [x] The card's description loses its claim that only tagged tracks are affected; its pointer to
      tagging tools stays. The overflow menu's dialog reads the same `enable-desc`
      (`overflow-menu.slint:216-217`), where the switch isn't, so the new wording reads right in
      both.
- [x] `ui/replaygain/`: the switch's handler is `toggle_binding` composed with `resume`, which
      kicks, since `toggle_binding` persists fire-and-forget and the pass reads only live state.
      Moved either way it clears a Stop, so off reads as idle. ReplayGain's own toggle keeps its
      kick. The progress painter on the `watch`, Stop and Measure.
- [x] Strings through `@tr`, the counts as plurals; `scripts/update-translations.py` and every
      catalog finished.
- [x] `README.md`'s ReplayGain lines (`:17`, `:128`).

**Gate:**
- [x] The static gates.
- [x] Kenan's hand pass, run by Claude on his go (2026-11-13, debug build, the 525-track dev library
      on a null sink, restored after; clicks through `ydotool`):
  - [x] the switch on with a library in place shows progress: at launch "39 / 525 tracks
        measured" with Stop, and on a click mid-session "26 / 516"; the row leaves when the pass
        ends (93 albums, 525 tracks);
  - [x] Stop holds through a new file arriving: "Stopped · 39 / 516" with Measure, and an untagged
        FLAC dropped into a watched folder was imported with no pass and no row;
  - [x] Measure resumes: a pass owing 478, the arrival among them;
  - [x] the switch off stops the pass ("stopped as measuring was switched off") and the row goes;
        the playing track, of an album measured at -8.05 dB with the preamp at 0, stepped -8.15 dB
        at the switch on and +7.72 dB at the switch off in the null sink's recording;
  - [x] the settings search finds the card by "measure" and, through the new row alone, by
        "untagged";
  - [x] the overflow dialog's description reads right, its controls unchanged;
  - [x] "Waiting for a library scan · 0 / 525 tracks measured" with Stop, while a launch scan
        parsed six reflink copies of the music folder and held the pass for 6.5 s; the pass came
        back as "0 / 3525" once it let go. The library's own scans hold it for milliseconds.

## Phase 6: The big library

- [ ] A whole-library pass on the big test library, measured and recorded here:
  - [ ] wall time, CPU, peak RSS sampled from `/proc` (never heaptrack on a pass this size), idle
        RSS after, and the album meter's share of the CPU against decoding;
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
  the doc comments of `loudness/`, `queries/loudness/`, `tasks/loudness/` and the migration's
  comments as each lands.
- **800 lines:** `equalizer.rs` (662), `metadata.rs` (597), `library/playback.rs` (542) and
  `lookup.rs` (507) gain lines, not bodies; everything new is its own module.
- **Memory:** `TrackSummary` gains one enum, and it lives only in the player's window; the
  queued-ids `watch` holds at most the window's ids. The pass's cost is bounded per worker and
  gone when the pass ends; Phase 4 and Phase 6 measure both.
- **Lock order** (ADR 0035) is untouched: the pass takes no player lock, its measurements reach the
  player through the queue window's existing fetch, and the queue-first ids come from that same
  fetch.
- **The live public release:** the setting defaults off, the new JSON field is `serde(default)`, and
  the migration only adds a table.

## Open questions

- `MEASURE_WORKERS`, settled in Phase 6.
- Whether Stop should hold across launches. This plan holds it for the session.
