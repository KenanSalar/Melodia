# Memory Efficiency

An audit of where Melodia holds memory, done on 2026-10-05 against the release build of v0.18.0
(`c592b8c`) and `scripts/measure-linux-footprint.sh`. Melodia is already lean in the states the
README measures. What is left is mostly artwork held larger or in more copies than it is drawn,
threads and buffers that outlive their job, and memory that grows with library size faster than
the distinct data does. This plan holds the changes that save memory without a visible, audible or
latency regression, one phase per change.

**Order:** phase 0 goes first, since the states most phases act on were not in the 2026-10-05
measurement. Phases 1 to 7 are independent of each other. The optional phases come last and start
only after a discussion: each one reverses a documented choice or carries a real behaviour risk.

**Measurement:** the 2026-10-05 run below is the baseline. It is measured again once phases 1 to 7
are all in, and again each time one or more of the optional phases lands (see Checkpoints).

**Figures:** anything not marked *measured* is an upper bound computed from the constants named
beside it, or an estimate from field types. Phase 0 and each phase's own re-measurement replace
them.

## What we see

- **A grid page holds its covers larger than it draws them.** On a 2560×1440 window at 1× a card's
  artwork is drawn at 184 px, 191.4 px with the hover zoom, and decoded at 224 px.
- **Opening the queue sheet decodes every visible cover a second time**, at the same size the track
  lists already hold it.
- **After a library scan, 16 worker threads stay alive for the rest of the session.** The installed
  build showed 19 `melodia-bg` threads on 2026-10-05: two runtime workers, the scan's 16 and one
  blocking thread.
- **A first scan of a large library holds every file's tags in memory before it writes any**, and
  the full re-read the tag backfill triggers does the same.
- **The Songs list costs more per track than its text does.** Each row allocates its own artist,
  album, genre, artwork path and duration, though an album's tracks share them, and an empty field
  still allocates.
- **Small holders outlive their use.** The visualizer's rings exist from launch whether or not it is
  ever opened, both analyzers are built whichever style is shown, Now Playing keeps a cover pair
  after it closes, and the tag editor decodes a picked image whole for a 160 px preview.

## Baseline (measured 2026-10-05)

Release build of `c592b8c` (v0.18.0, built 2026-10-05 08:46), measured with
`scripts/measure-linux-footprint.sh` attached by `--pid` to a fresh launch: 30 s settle, 10 s after
each switch, 60 one-second samples per scenario, memory the median sample and CPU and GPU the total
over the window.

| Scenario | Anonymous | PSS | USS | RSS | Peak RSS | GPU memory | CPU, one core | GPU | Threads |
|---|---|---|---|---|---|---|---|---|---|
| Idle | 34.3 MiB | 89.8 MiB | 85.0 MiB | 162.0 MiB | 176.2 MiB | 26.0 MiB | 0.15% | 0.00% | 41 |
| Playing, list view | 35.5 MiB | 92.2 MiB | 87.2 MiB | 165.3 MiB | 178.5 MiB | 26.0 MiB | 0.65% | 0.00% | 41 |
| Playing, visualizer live | 35.2 MiB | 91.4 MiB | 87.5 MiB | 162.5 MiB | 178.5 MiB | 30.0 MiB | 3.47% | 2.19% | 42 |

No swap in any window; 91 to 94 file descriptors and 559 to 563 memory maps.

The setup a checkpoint reproduces, or names where it differs:

- **Machine:** Ryzen 7 9800X3D with an RTX 3080, Fedora 44, kernel 7.2.8 (per-page mapcounts, so PSS
  and USS are exact), KDE Plasma 6.7.5 on Wayland.
- **Screen:** the window on the 144 Hz 2560×1440 screen at scale 1 (DP-6). The report's Display line
  confirms it.
- **App:** the `Melodia-dev` data root, the 512-track library, the native titlebar, the Mirrored
  visualizer style, and Last.fm, ListenBrainz and Discord presence switched off for the run.
- **Queue:** the Synthwave playlist plus the test song, which is what plays in both playing
  scenarios.
- **Launches:** Idle came from a first launch with a different song selected, the two playing
  windows from a second launch of the same build.

None of these scenarios has a grid page up, the queue sheet open or a scan running, which is where
most of this plan acts. Phase 0 adds those states to the baseline.

## Checkpoints

Each checkpoint is a full run of the baseline protocol, the phase 0 states included, written into
the table below beside the baseline. A phase's own re-measurement (Every phase) is a spot check of
the state it targets, taken to catch a regression early; it does not replace a checkpoint.

1. **After phases 1 to 7.** Once every phase in the main list is in. This is the number the plan is
   judged on.
2. **After the optional phases.** Each time one or more of phases A to D lands, a new block of rows
   named for exactly the phases it includes, for example *After 1–7 + B*.

| Run | Scenario | Anonymous | PSS | USS | RSS | Peak RSS | GPU memory | CPU, one core | GPU | Threads |
|---|---|---|---|---|---|---|---|---|---|---|
| Baseline, 2026-10-05 | Idle | 34.3 MiB | 89.8 MiB | 85.0 MiB | 162.0 MiB | 176.2 MiB | 26.0 MiB | 0.15% | 0.00% | 41 |
| Baseline, 2026-10-05 | Playing, list view | 35.5 MiB | 92.2 MiB | 87.2 MiB | 165.3 MiB | 178.5 MiB | 26.0 MiB | 0.65% | 0.00% | 41 |
| Baseline, 2026-10-05 | Playing, visualizer live | 35.2 MiB | 91.4 MiB | 87.5 MiB | 162.5 MiB | 178.5 MiB | 30.0 MiB | 3.47% | 2.19% | 42 |
| Baseline, phase 0 | Grid page (Albums) | | | | | | | | | |
| Baseline, phase 0 | Queue sheet open | | | | | | | | | |
| Baseline, phase 0 | Now Playing opened, then closed | | | | | | | | | |
| Baseline, phase 0 | Minute after a full rescan | | | | | | | | | |
| After 1–7 | Idle | | | | | | | | | |
| After 1–7 | Playing, list view | | | | | | | | | |
| After 1–7 | Playing, visualizer live | | | | | | | | | |
| After 1–7 | Grid page (Albums) | | | | | | | | | |
| After 1–7 | Queue sheet open | | | | | | | | | |
| After 1–7 | Now Playing opened, then closed | | | | | | | | | |
| After 1–7 | Minute after a full rescan | | | | | | | | | |

The scan phases are judged on the large library instead, where the footprint script's windows do
not reach. Peak `RssAnon` comes from `MELODIA_RSS_SAMPLE=1` during a first scan into an empty
database.

| Run | First scan of the large library | Peak RssAnon | Wall time | Threads after |
|---|---|---|---|---|
| Baseline, phase 0 | | | | |
| After 1–7 | | | | |

Production files the phases touch, in lines (`wc -l`): `cover_thumbs.rs` 622, `folders.rs` 484,
`now_playing/mod.rs` 472, `visualizer.rs` 441, `track_list_cache.rs` 389, `grid_prewarm.rs` 358,
`mbid_backfill.rs` 358, `ui/visualizer/mod.rs` 337, `queries/track/lookup.rs` 254, `tracks/mod.rs`
168, `queue_sheet/mod.rs` 167, `scanner.rs` 118, `callbacks/tags/artwork.rs` 98, `ui/util.rs` 78.

## Every phase

- [ ] Re-measure what the phase targets before touching code, and rescope if it moved.
- [ ] Spot-check the state it targets before and after, and confirm CPU and latency did not move:
      the footprint script for a steady state, heaptrack for a peak. A phase that saves memory by
      costing frames, decode time or scan time is not done. The full comparison waits for the
      checkpoint.
- [ ] Gate: `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings`,
      `cargo test --locked --workspace`. Existing tests the change breaks are updated in the same
      phase. New tests a phase lists wait for the manual check and an explicit go.
- [ ] Update the doc comment, the `.claude/rules/` entry or the CLAUDE.md line that describes what
      moved, in the same phase.
- [ ] No production Rust file past 800 lines.

## Phase 0: baseline and one correction

- [ ] Measure the states this plan acts on, on the same build and setup as the baseline: a grid page
      (Albums) scrolled once, the queue sheet open on a long queue, Now Playing opened and closed,
      and the minute after a forced full rescan. Fill the *Baseline, phase 0* rows, and keep a
      heaptrack profile of each beside them.
- [ ] Build a large library outside the repo for phases 4 to 6: ffmpeg-generated short files with
      tags spread over a few thousand albums, 50k tracks or more. Fill the baseline row of the scan
      table from a first scan into an empty database.
- [ ] `playlists/callbacks/dialog.rs:68` quotes "~603 KiB" for the cover buffer, a figure left from
      the old 448 px tier. Drop the number and keep what the comment argues.

## Phase 1: decode grid covers at the size the tile draws

Where: `crates/melodia-views/src/ui/grid_prewarm.rs` (`widest_card`, `cover_size`),
`crates/melodia-ui/ui/components/grid/entity-card.slint` (`tile-size`, `cover-scale-hover`).

`cover_size` sizes the tier off the widest card a column band draws, but the artwork sits inside
the card's `pad-sm` inset and grows by the hover zoom at most. On a 2560×1440 window at 1× that is a
200 px card, a 184 px tile drawn at up to 191.4 px, and a 224 px decode after the 32 px step.

- [ ] Size off `(widest_card - 2 × inset) × hover zoom × scale`, then step: 192 px instead of 224 px
      in the case above, and 384 px instead of 416 px for the same card at 2×.
- [ ] The inset and the zoom become Rust constants beside `MIN_CARD_W` and `MAX_CARD_W`, pinned
      against `entity-card.slint` the way `the_card_constants_are_the_ones_the_component_declares`
      pins `grid-geometry.slint`, so the two spellings cannot drift. The zoom is the largest any grid
      with artwork uses; the Genres grid raises it only on cards that decode no cover.
- [ ] Radio's logo tier shares the sizing and publishes `logo-decode-size`, which
      `station-card.slint` divides by to place a logo, so the derivation follows on its own. Check
      that a small logo still insets the same.
- [ ] Update `cover_size`'s doc comment and `ui-patterns.md`'s "Decode size via
      `grid_prewarm::cover_size_for_window`" entry, both of which say the tier is derived from the
      card.

Memory: 150,528 B to 110,592 B per cover at 1× (−26.5%), up to −3.5 MiB of heap at the 91-cover cap
that window gets, and the same cut in each mounted card's texture. 519,168 B to 442,368 B per cover
at 2× (−14.8%). The 64 px proxies a section leave keeps are unchanged.
Risk: none while the size stays at or above what is drawn, FemtoVG minifying without mipmaps.
Tests, when asked: sweep `grid_prewarm_tests` over widths and scales, asserting the decode never
falls under the drawn tile.

## Phase 2: let the queue sheet share the row tier's buffers

Where: `crates/melodia-views/src/ui/queue_sheet/mod.rs` (the private `CoverThumbs` and its
`request-cover` handler), `crates/melodia-artwork/src/media/image/cover_thumbs.rs`.

The sheet's tier is private so that closing it drops every buffer without taking covers the track
lists still need. It decodes at the same `row_cover_size` as the shared row tier, so every cover the
two have in common is held twice while the sheet is open. Cached buffers are refcounted
`SharedPixelBuffer`s.

- [ ] Give `CoverThumbs` a lookup that returns a cached buffer without promoting it in the LRU, and
      only when it was decoded at the asked size.
- [ ] On a private miss the sheet takes the shared tier's buffer by reference and caches that,
      decoding only what the shared tier lacks. Closing still clears the private tier, which now
      drops references rather than pixels where the two overlapped. The reason the tier is private
      holds unchanged.
- [ ] Update the comment at `queue_sheet/mod.rs:74` and `ui-patterns.md`'s `QueueRow` entry.

Memory: up to 512 × 48² × 3 = 3,538,944 B at 1× and 512 × 72² × 3 = 7,962,624 B on HiDPI while the
sheet is open, plus the decode work for every overlapping cover. Nothing changes while it is closed.
Risk: low. A shared buffer the shared tier later evicts stays alive through the sheet's reference
until the sheet closes, which is what the sheet holds today anyway.

## Phase 3: small holders that outlive their use

- [ ] **Visualizer rings** (`crates/melodia-playback/src/player/playback/visualizer.rs`, built at
      `engine/backend/mod.rs:142`): two decks × 16,384 × 4 B = 131,072 B, written in full when the
      engine is constructed, whether or not the visualizer is ever shown. Allocate them on the first
      enable, on the UI thread and never on the audio callback, keeping `valid_from` and `DeckRun`
      as they behave today.
- [ ] **Analyzers** (`crates/melodia-views/src/ui/visualizer/mod.rs:69`): the spectrum and the
      waveform analyzer are both built whichever style is shown. Build the shown one, and the other
      on a style switch. The audit put the two at roughly 110 to 170 KiB while the strip is up;
      measure before and after. The waveform window is sized to `RING_CAP` and the comment at `:73`
      argues it, so leave it unless the measurement says otherwise.
- [ ] **Now Playing's crossfade pair** (`Player.np-cover-a/b`, `blur-img-a/b`): once no surface
      renders the artwork, the slot not shown still holds a 384 px cover and its blur, about
      0.5 MiB. Clear that slot then, never mid-fade. The two-slot rule in `slint-pitfalls.md` names
      `ui::now_playing::source_change` as the one site allowed to clear a pair; a second site argues
      itself the same way, and the rule entry is updated to say so.
- [ ] **Tag editor preview** (`crates/melodia-views/src/ui/callbacks/tags/artwork.rs:91`):
      `decode_cover_preview` decodes the picked file whole through `decode_capped` and then resizes.
      Decode through `decode_capped_to` against `COVER_SIZE`, under `large_decode_guard`. Peak only:
      a 3000 × 3000 JPEG no longer passes through a full-size buffer. `crates/melodia/tests/bounded_decode.rs`
      stays green, both helpers living in the file it treats as the owner.

Risk: low for each. None changes what is drawn or heard.

## Phase 4: run scan work on a pool that ends with the scan

Where: the global-pool `par_iter`s at `library/settings/folders.rs:274`,
`media/ingest/scanner.rs:53`, `database/queries/ingest.rs:335`,
`tasks/file_event_processor/reconcile.rs:63`, `tasks/rating_import.rs:124` and
`tasks/retroactive_hash.rs:41`.

They all run on rayon's global pool, which is built on first use with one thread per logical CPU
and never exits: 16 threads here, alive from the boot reconcile to quit. Each keeps its stack, and
any that resized a cover keeps up to `RESIZER_SCRATCH_CAP` (2 MiB) of scratch in a thread-local
(`image_decode.rs:258`).

- [ ] One helper owns the scoped pool: built at today's width, used through `install`, dropped at
      the end of the job. It lives in `melodia-store`, the lowest crate that calls it, which
      `melodia-app` already names; nothing re-exports it. `library/mbid.rs:133` builds its own pool
      per call and folds into the helper.
- [ ] Name its threads within 15 bytes (`scan-{i}`), the limit `crates/melodia/tests/packaging.rs`
      walks for.
- [ ] Small watcher batches stay sequential rather than paying for a pool.
- [ ] `cover_thumbs.rs:439` falls back to `rayon::spawn` when its decode pool fails to build. Leave
      it: after this phase it is the one use of the global pool left, and only on that failure.
- [ ] Update the doc comments that say "Rayon worker" where the scratch cap is argued.

Memory: 16 fewer idle threads on this machine, their stacks, and up to 32 MiB of resizer scratch
after a scan that stored covers. Measure the thread count after boot and after a rescan.
Cost: one pool build per scan. Time a forced full rescan on the phase 0 library before and after.

## Phase 5: parse and ingest a scan in chunks

Where: `crates/melodia-app/src/library/settings/folders.rs` (`scan_folder_internal`,
`TX_CHUNK_FILES`), and the full re-read `tasks/tag_backfill.rs` triggers.

The scan parses the whole folder into one `Vec<ScannedFile>` and only then writes it in
`TX_CHUNK_FILES` (2,000) transactions. An `ExtractedMetadata` carries about 29 text fields, so a
first scan of a 50k library holds an estimated 75 to 100 MB of parsed tags at its peak.

- [ ] Parse and ingest per 2,000-file chunk, so the peak follows the chunk rather than the library.
      Overlap the next chunk's parse with the current chunk's write if the phase 0 timing shows a
      dip.
- [ ] Moved files keep their ratings and play counts: `process_batch` resolves candidate hashes
      before its transaction and applies deletes last (CLAUDE.md, file hashing). Check that a chunk
      boundary cannot separate what that ordering relies on.
- [ ] Progress keeps its total, which is known before the parse, and orphan detection keeps its full
      path list.
- [ ] `folders.rs:402` clones the on-disk path set where it could move it. Move it: about 6 MB
      transient at 50k (estimate).

Memory: the first-scan peak goes from following the library to following one 2,000-file chunk.
Nothing changes at idle.
Risk: low to medium, on throughput. Verify with `MELODIA_RSS_SAMPLE=1` on a first scan of the
phase 0 library: peak `RssAnon` and wall time, before and after.

## Phase 6: share repeated strings in the Songs list

Where: `crates/melodia-views/src/ui/track_list_cache.rs` (`convert`),
`crates/melodia-views/src/ui/tracks/mod.rs` (`to_slint_track_list_row`), and the other
`unwrap_or("")` row conversions (`shell/bridge.rs`, `queue_sheet/rows.rs`, `albums/mod.rs`).

Each row allocates its own artist, album, genre, artwork path and duration string, and
`SharedString::from("")` allocates where `SharedString::default()` does not (i-slint-core 1.16.1,
`string.rs`).

- [ ] Intern within one conversion pass: a local map from text to `SharedString` for the fields an
      album's tracks share, so they hold one string each. The pass already runs on a runtime worker,
      so the UI thread pays nothing.
- [ ] One helper in `ui/util.rs`, beside the other row conversions, turns an optional field into a
      `SharedString` and answers `default()` when it is empty. Every `unwrap_or("")` site goes
      through it.
- [ ] The detail conversions take the same interner unless phase A below lands first.

Memory: an estimated 0.3 KB per track, about 15 MB at 50k, plus 48 B for each empty field on each
row. Trivial at 512 tracks; measure on the phase 0 library.
Risk: low. The strings are never mutated, and `SharedString` compares by content.

## Phase 7: page the ListenBrainz backfill

Where: `crates/melodia-store/src/database/queries/track/lookup.rs` (`get_tracks_missing_mbid`),
`crates/melodia-app/src/tasks/mbid_backfill.rs`.

With a ListenBrainz account connected, every library change loads every eligible track and then
drops, in memory, the ones already attempted. `attempted` is also loaded at boot with the feature
off.

- [ ] Page by id, as `get_unrated_track_paths_after` does for the rating import.
- [ ] Load `attempted` only once a token exists.

Memory: an estimated 10 MB transient per library change at 50k, and only with the feature on.
Risk: low.

## Optional, after discussing

Each of these reverses a documented choice or carries a real behaviour risk, so none starts without
a decision first. Each one that lands, alone or with others, ends with a checkpoint run named for
exactly what it includes.

### Phase A: keep ids, not a second row copy, in the detail views

Where: `albums`, `artists`, `genres` and `playlists` under `crates/melodia-views/src/ui/`
(`state.rs`'s `tracks` and `all_tracks`, `detail.rs`, `detail_filter.rs`, `detail_selection.rs`,
`section_state.rs`).

A detail view holds the open entity's tracks three times: `all_tracks`, `tracks` (cloned on the UI
thread when a detail opens, `albums/detail.rs:139`) and the Slint model. Most readers of `tracks`
need only the id; the favourite and rating patch in `section_state.rs` exists because the copy does.

- [ ] `tracks` becomes the displayed ids. Re-sorting derives them from the sorted `all_tracks`, and
      the playlist rollback snapshot becomes ids too.
- [ ] Optionally, back `all_tracks` with a `TrackListCache`, so the model shares its strings and a
      filter keystroke stops re-converting rows.

Memory: about 0.85 KB per track of the open entity (estimate from types), more with the cache, and
no deep clone on the UI thread when a detail opens.
Risk: medium, around range selection and playlist reordering. The four views are near-copies, so
this goes after any work that folds them into one.

### Phase B: the SQLite write connection after a large scan

Where: `crates/melodia-store/src/database/mod.rs` (`write_opts` and the write pool).

The one write connection keeps a page cache of up to 16,000 KiB, has no `mmap_size`, and is retired
only after sqlx's default 10-minute idle timeout (sqlx-core 0.9.0). After a large scan that cache
can stay resident for those 10 minutes.

- [ ] Give it `mmap_size` and a short `idle_timeout` like the read pool, and consider a smaller
      `cache_size`.

Memory: up to about 16 MB for up to 10 minutes after a scan at 50k (estimate).
Risk: bulk insert speed, and the reopen after a reap. Time an ingest on the phase 0 library first.

### Phase C: decode detail heroes at the hero's size

Where: `crates/melodia-views/src/ui/detail_artwork.rs` (a 12-entry LRU per detail type),
`crates/melodia-views/src/ui/util.rs` (`COVER_SIZE`).

Hero covers decode at `COVER_SIZE` (384 px) for a 140 px square. `util.rs` argues that one decode
size across the app is worth more than the buffers it would save "on surfaces holding a single
image", and a 12-entry LRU per detail type is not one.

- [ ] Decode heroes at the 140 px square times the scale, stepped. The Edit Artwork dialog reuses
      the cover at 130 px and still fits.

Memory: 442,368 B to 76,800 B per hero at 1× (160 px), up to about 4.2 MiB per detail type at the
LRU cap.
Risk: reverses a documented decision. The call is whether one size across the app still earns its
buffers.

### Phase D: cap folder-watch batches

Where: `crates/melodia-app/src/tasks/file_event_processor/` (`mod.rs:106`, `reconcile.rs`,
`dedup.rs`).

Everything that arrives inside the 500 ms window becomes one batch, and its extraction holds every
file's tags at once.

- [ ] Cap a batch at about 2,000 events without ever splitting a delete from the create it pairs
      with, which is what carries a cross-device move across with its ratings, play counts and
      favourites intact.

Memory: transient, an estimated 15 to 20 MB for a 10k-file drop.
Risk: medium, on move detection. Only with a test that moves a folder across the cap.

## Checked and left alone

- **Audio buffers.** Nothing decodes a track ahead. The resampler table is one shared table,
  Symphonia's 64 KiB read buffer is its minimum, and the ring, period and download buffer sizes
  protect latency, underruns and the network cushion. Halving the radio prebuffer would save up to
  about 1 MiB but risks dropouts.
- **The cover constants**: the row sizes and their threshold, `CACHE_CAP`, `PROXY_COVER_DIM`,
  `BLUR_TARGET`, `STORE_MAX_DIM`, the decode pool width and `RESIZER_SCRATCH_CAP`, each argued at
  its definition.
- **The binary.** Fat LTO, one codegen unit, `panic = "abort"` and a size-optimised Slint unit are
  already set, and the five embedded fonts come to 423 KB. TLS uses the system roots, which an
  enterprise CA needs.
- **The rounded-clip layer** each artwork tile renders on top of its cover texture is likely a real
  part of the GPU memory, but the clip is there for antialiasing (`slint-pitfalls.md`). Measure
  before touching it.
- **The allocator.** No allocator swap and no periodic trim; both were measured and rejected
  (`rust-performance.md`, `tasks/heap_trim.rs`).

When phases 1 to 7 are in and checkpointed, and each optional phase is either in and checkpointed or
declined, this plan is deleted.
