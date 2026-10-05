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
- **16 worker threads stay alive from launch to quit.** The boot reconcile builds rayon's global
  pool on every launch with folder watching on, the default, so they are already in the baseline's
  41. The installed build showed 19 `melodia-bg` threads on 2026-10-05: two runtime workers, the
  pool's 16 and one blocking thread.
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
168, `queue_sheet/mod.rs` 167, `scanner.rs` 118, `callbacks/tags/artwork.rs` 98, `ui/util.rs` 78,
`main.rs` 555, `playlists/callbacks/dialog.rs` 544, `queries/ingest.rs` 478, `shell/bridge.rs` 392,
`database/mod.rs` 341, `boot/ui_setup/views.rs` 268.

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
`crates/melodia-ui/ui/components/grid/entity-card.slint` (`tile-size`, `cover-scale-hover`), the
six hosts that spell the zoom (`album-grid`, `entity-card-grid`, `browse-card-grid`,
`artist-grid`, `playlist-grid`, `views/my-library/artist-detail.slint`), and
`crates/melodia-views/src/ui/playlists/callbacks/dialog.rs` (Edit Artwork from the grid).

`cover_size` sizes the tier off the widest card a column band draws, but the artwork sits inside
the card's `pad-sm` inset and grows by the hover zoom at most. On a 2560×1440 window at 1× that is a
200 px card, a 184 px tile drawn at up to 191.4 px, and a 224 px decode after the 32 px step.

- [ ] Give the cover zoom one spelling first. `EntityCard` declares 1.04 as its default, but each
      of the six hosts spells `? 1.15 : 1.04` itself, so a pin on the component alone would miss
      a host. Leave 1.04 to the default and have the hosts override only the no-artwork 1.15.
- [ ] Size off `(widest_card - 2 × inset) × hover zoom × scale`, then step: 192 px instead of 224 px
      in the case above, and 384 px instead of 416 px for the same card at 2×.
- [ ] The inset and the zoom become Rust constants beside `MIN_CARD_W` and `MAX_CARD_W`, pinned the
      way `the_card_constants_are_the_ones_the_component_declares` pins `grid-geometry.slint`: the
      zoom against `entity-card.slint`, the inset against `pad-sm` in `theme.slint`. The zoom is
      the largest any grid with artwork uses; the Genres grid raises it only on cards that decode
      no cover.
- [ ] The Edit Artwork dialog opened from the playlist grid draws the grid tier's buffer in its
      220 px preview (`grid_cover_blocking`), so 192 px would upscale it. It already does on
      windows whose cards land at 192 today. Decide whether the dialog decodes its own cover at
      the preview's size or accepts the softness.
- [ ] Radio's logo tier shares the sizing and publishes `logo-decode-size`, which
      `station-card.slint` divides `cover.width` by to estimate a logo's 1:1 size. The insets
      move: with the denominator at 192 instead of 224, each small logo's estimate grows by a
      sixth, logos 96 to 111 px wide fill the tile instead of floating as an inset card, and
      smaller ones inset larger. The estimate lands closer to the true 1:1 size than today's, but
      the page changes, so look at it before calling the phase done.
- [ ] Update `cover_size`'s doc comment and `ui-patterns.md`'s "Decode size via
      `grid_prewarm::cover_size_for_window`" entry, both of which say the tier is derived from the
      card.

Memory: 150,528 B to 110,592 B per cover at 1× (−26.5%), up to −3.5 MiB of heap at the 91-cover cap
that window gets, and the same cut in each mounted card's texture. 519,168 B to 442,368 B per cover
at 2× (−14.8%). The 64 px proxies a section leave keeps are unchanged.
Risk: none on the cards while the size stays at or above what is drawn, FemtoVG minifying without
mipmaps. The two surfaces above draw the buffer at something other than the card's tile, which is
where this phase is visible.
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
- [ ] `queue_sheet::install(ui, state)` has no handle on the shared tier, which is
      `ViewCtx.cover_thumbs`, built in `boot/ui_setup/views.rs`. It takes one.
- [ ] Update the comment at `queue_sheet/mod.rs:74` and `ui-patterns.md`'s `QueueRow` entry.

Memory: up to 512 × 48² × 3 = 3,538,944 B at 1× and 512 × 72² × 3 = 7,962,624 B on HiDPI while the
sheet is open, plus the decode work for every overlapping cover. Nothing changes while it is closed.
The bound needs 512 distinct covers in both tiers at once; a queue drawn from a few albums shares
far fewer. Past its warm-up the sheet decodes a miss on the UI thread (`get_or_load_opt`), so every
cover it finds in the shared tier is also a decode taken off the event loop.
Risk: low. A shared buffer the shared tier later evicts stays alive through the sheet's reference
until the sheet closes, which is what the sheet holds today anyway.

## Phase 3: small holders that outlive their use

- [ ] **Visualizer rings** (`crates/melodia-playback/src/player/playback/visualizer.rs`, built at
      `engine/backend/mod.rs:142`): two decks × 16,384 × 4 B = 131,072 B, written in full when the
      engine is constructed, whether or not the visualizer is ever shown. Allocate them on the first
      enable, on the UI thread and never on the audio callback, keeping `valid_from` and `DeckRun`
      as they behave today. The audio thread then needs a lock-free view of a ring that may not
      exist yet (a `OnceLock` per deck), and `RING_CAP`'s doc, which says the rings are resident
      for the life of the player, is updated.
- [ ] **Analyzers** (`crates/melodia-views/src/ui/visualizer/mod.rs:69`): the spectrum and the
      waveform analyzer are both built whichever style is shown. They already exist only while the
      strip is mounted, so what this saves is the unshown one's share. Build the shown one, and the
      other on a style switch. The audit put the two at roughly 110 to 170 KiB while the strip is
      up; measure before and after. The waveform window is sized to `RING_CAP` and the comment at
      `:73` argues it, so leave it unless the measurement says otherwise.
- [ ] **Now Playing's crossfade pair** (`Player.np-cover-a/b`, `blur-img-a/b`): once no surface
      renders the artwork, the slot not shown still holds a 384 px cover and its blur, about
      0.5 MiB. Clear that slot then, never mid-fade. The miniplayer's card and column read the same
      pair, which `Surfaces::renders_artwork` already answers for. The two-slot rule in
      `slint-pitfalls.md` names `ui::now_playing::source_change` as the one site allowed to clear a
      pair; a second site argues itself the same way, and the rule entry is updated to say so.
- [ ] **Tag editor preview** (`crates/melodia-views/src/ui/callbacks/tags/artwork.rs:91`):
      `decode_cover_preview` decodes the picked file whole through `decode_capped` and then resizes.
      Decode through `decode_capped_to` against `COVER_SIZE`, under `large_decode_guard`. Peak only:
      a 3000 × 3000 JPEG no longer passes through a full-size buffer. The scaled path is chosen by
      the file's extension, so a PNG or WebP pick still decodes whole.
      `crates/melodia/tests/bounded_decode.rs` walks for `capped_limits(` and `ImageReader::new(`
      only, so it stays green.

Risk: low for each. None changes what is drawn or heard.

## Phase 4: run scan work on a pool that ends with the scan

Where: the global-pool `par_iter`s at `library/settings/folders.rs:274`,
`media/ingest/scanner.rs:53`, `database/queries/ingest.rs:335`,
`tasks/file_event_processor/reconcile.rs:63`, `tasks/rating_import.rs:124` and
`tasks/retroactive_hash.rs:41`.

They all run on rayon's global pool, which is built on first use with one thread per logical CPU
and never exits: 16 threads here, alive from the boot reconcile to quit. The reconcile's
`par_iter` at `folders.rs:274` runs ahead of the `files.is_empty()` return, so an unchanged library
builds the pool on every launch with folder watching on. Each thread keeps its touched stack, and
any that resized an oversized cover during a scan keeps up to `RESIZER_SCRATCH_CAP` (2 MiB) of
scratch in a thread-local (`image_decode.rs:258`).

**Moving these six alone does not retire the global pool.** `jpeg-decoder` is built with its
default `rayon` feature, and every colour JPEG it decodes runs its colour pass through
`par_chunks_mut`, with a parallel component decode on top past 128 × 128. On a rayon worker that
work stays on the worker's own pool; anywhere else it builds the global one: the bar's cover
warm at boot (`shell/bridge.rs:98` → `warm_vm_cover`, on the blocking pool), Now Playing and the
detail heroes (`artwork_cache.rs:113`), Material You, and the inline decodes the queue sheet and
`grid_cover_blocking` run on the UI thread. Any of those brings back all 16 threads.

- [ ] Build rayon's global pool small and named at startup (`ThreadPoolBuilder::build_global`
      right after the runtime, ahead of the first rayon use), so `jpeg-decoder` keeps a home that
      costs two threads. The two alternatives each cost something this one doesn't. Turning off
      `jpeg-decoder`'s default features spawns and joins OS threads per decode, the churn
      `TAG_WRITE_POOL`'s doc argues against. Routing those decodes through `cover-decode` queues
      the UI thread's inline decodes behind a grid prewarm. Time a Now Playing open before and
      after.
- [ ] One helper owns the scoped pool for the jobs: built at today's width, dropped at the end of
      the job. It lives in `melodia-store`, the lowest crate among the six sites, which `melodia-app`
      already names; nothing re-exports it. `library/mbid.rs:133` builds its own pool per call and
      folds into the helper.
- [ ] A scan's three stages are separate `spawn_blocking` closures with `.await`s between them
      (`folders.rs:271`, `:329`, then `ingest.rs:333` per chunk), so no single `install` covers
      them. The job holds the pool in an `Arc` and hands it to each closure, which gives
      `ingest_scanned_files` a parameter.
- [ ] Name both pools' threads within 15 bytes (`scan-{i}` and one for the global pool), and add
      each file that names through a closure to `RUNTIME_NAMED` in
      `crates/melodia/tests/packaging.rs`, which holds that list to an exact count.
- [ ] Small watcher batches stay sequential rather than paying for a pool; `reconcile.rs:63` runs
      today on a batch of one file.
- [ ] `TAG_WRITE_POOL` (`library/tags.rs:49`) keeps its four threads after the first tag write. Leave
      it: building one per call is what its doc argues against.
- [ ] `cover_thumbs.rs` falls back to the global pool in three places when its decode pool fails
      to build (`shrink_to_proxy`, `schedule` at `:439`, `prewarm`). Leave them; after this phase
      that pool is the small one.
- [ ] Update the doc comments that say "Rayon worker" where the scratch cap is argued.

Memory: 14 fewer idle threads on this machine (the global pool's 16 become two), their touched
stacks, and up to 32 MiB of resizer scratch after a scan that stored oversized covers. All three
baseline rows already carry the 16, so Idle should move. Measure the thread count after boot,
after opening Now Playing and after a rescan.
Cost: one pool build per scan. Time a forced full rescan on the phase 0 library before and after.

## Phase 5: parse and ingest a scan in chunks

Where: `crates/melodia-app/src/library/settings/folders.rs` (`scan_folder_internal`,
`TX_CHUNK_FILES`), and the full re-read `tasks/tag_backfill.rs` triggers.

The scan parses the whole folder into one `Vec<ScannedFile>` and only then writes it in
`TX_CHUNK_FILES` (2,000) transactions. An `ExtractedMetadata` carries 22 text fields of its own, 11
more in `SortTags` and `ReleaseTags`, and six variable-length lists of names and IDs. The cover is
written to the store during the parse, so no picture bytes ride along. A first scan of a 50k
library holds an estimated 75 to 100 MB of parsed tags at its peak.

- [ ] Parse and ingest per 2,000-file chunk, so the peak follows the chunk rather than the library.
      Overlap the next chunk's parse with the current chunk's write if the phase 0 timing shows a
      dip.
- [ ] Moved files keep their ratings and play counts. On the scan path that is
      `ingest_scanned_files`, not the watcher's `process_batch`: it matches each chunk's hashes
      against the whole table, and the orphan purge runs once, after the last chunk. Chunking the
      parse keeps that as long as the purge stays after every chunk.
- [ ] Progress keeps its total, which is known before the parse, and orphan detection keeps its full
      path list.
- [ ] `folders.rs:402` copies every on-disk path into a `String` set (`to_string_lossy`) while
      `files`, never read again, stays alive beside it. Move the buffers instead: about 6 MB
      transient at 50k (estimate).

Memory: the first-scan peak goes from following the library to following one 2,000-file chunk.
Nothing changes at idle.
Risk: low to medium, on throughput. Verify with `MELODIA_RSS_SAMPLE=1` on a first scan of the
phase 0 library: peak `RssAnon` and wall time, before and after.

## Phase 6: share repeated strings in the Songs list

Where: `crates/melodia-views/src/ui/track_list_cache.rs` (`convert`),
`crates/melodia-views/src/ui/tracks/mod.rs` (`to_slint_track_list_row`), and the other
`unwrap_or("")` row conversions: `shell/bridge.rs`, `queue_sheet/rows.rs`, `albums/mod.rs`,
`artists/mod.rs`, `playlists/mod.rs`, `search/mod.rs`, `favorites/rows.rs`,
`recently_played/rows.rs`, `browse/cards.rs`, `browse/mod.rs` and `now_playing/metadata.rs`.

Each row allocates its own artist, album, genre, artwork path and duration string, and
`SharedString::from("")` allocates where `SharedString::default()` does not (i-slint-core 1.16.1,
`string.rs`): a 25 B request for the header and the NUL, a 48 B glibc chunk.

- [ ] Intern within one conversion pass: a local map from text to `SharedString` for the fields an
      album's tracks share, so they hold one string each. The Songs pass runs on a runtime worker,
      so the UI thread pays nothing there. Favorites' refresh on `library_changed` and
      `stats_changed` converts on the UI thread (`spawn_local`, `favorites/callbacks/lifecycle.rs`),
      as do the detail filters, so time the map there or keep it to the Songs pass.
- [ ] One helper in `ui/util.rs`, beside the other row conversions, turns an optional field into a
      `SharedString` and answers `default()` when it is empty. Every `unwrap_or("")` site goes
      through it. It replaces `bridge.rs`'s private `opt_shared`, which allocates on `None`;
      `radio/rows.rs` already spells the allocation-free form inline.
- [ ] The detail conversions take the same interner unless phase A below lands first.

Memory: an estimated 0.3 KB per track, about 15 MB at 50k, plus 48 B for each empty field on each
row. Trivial at 512 tracks; measure on the phase 0 library. The cache holds every row and the Slint
model holds clones sharing the same buffers, so the saving is counted once.
Risk: low. The strings are never mutated, and `SharedString` compares by content.

## Phase 7: page the ListenBrainz backfill

Where: `crates/melodia-store/src/database/queries/track/lookup.rs` (`get_tracks_missing_mbid`),
`crates/melodia-app/src/tasks/mbid_backfill.rs`.

With MusicBrainz auto-tagging on and a ListenBrainz account connected (`mbid_lookup_token`), every
library change loads every eligible track and then drops, in memory, the ones already attempted.
A library change here includes every favourite and rating toggle, not only scans. `attempted` is
also loaded at boot with the feature off, which costs something only once
`scrobble_mbid_attempted.json` exists.

- [ ] Page by id, as `get_unrated_track_paths_after` does for the rating import.
- [ ] Load `attempted` only once a token exists.

Memory: an estimated 10 MB transient per library change at 50k, and only with the feature on. A row
is about 350 to 400 B, so that figure assumes 25k to 35k tracks without an MBID.
Risk: low.

## Optional, after discussing

Each of these reverses a documented choice or carries a real behaviour risk, so none starts without
a decision first. Each one that lands, alone or with others, ends with a checkpoint run named for
exactly what it includes.

### Phase A: keep ids, not a second row copy, in the detail views

Where: `albums`, `artists`, `genres` and `playlists` under `crates/melodia-views/src/ui/`
(`state.rs`'s `tracks` and `all_tracks`, `detail.rs`, `detail_filter.rs`, `detail_selection.rs`,
`section_state.rs`).

A detail view holds the open entity's tracks three times: `all_tracks` (a deep clone on the UI
thread when a detail opens, `albums/detail.rs:139`), `tracks` (the original, moved) and the Slint
model. Every filter keystroke deep-clones the matching rows into `tracks` on the UI thread again
(`detail_filter.rs:62-69`). Most readers of `tracks` need only the id; the favourite and rating
patch in `section_state.rs` writes both copies, and its `tracks` half exists because the copy does.

- [ ] `tracks` becomes the displayed ids. Re-sorting derives them from the sorted `all_tracks`, and
      the playlist rollback snapshot becomes ids too. The patch keeps its `all_tracks` half.
- [ ] Optionally, back `all_tracks` with a `TrackListCache`, so the model shares its strings and a
      filter keystroke stops re-converting rows.

Memory: about 0.85 KB per track of the open entity (estimate from types), more with the cache, and
no deep clone on the UI thread when a detail opens or a filter key lands.
Risk: medium, around range selection and playlist reordering. The four views are near-copies, so
this goes after any work that folds them into one.

### Phase B: the SQLite write connection after a large scan

Where: `crates/melodia-store/src/database/mod.rs` (`write_opts` and the write pool).

The one write connection keeps a page cache of up to 16,000 KiB, has no `mmap_size`, and is retired
only after sqlx's default 10-minute idle timeout or its 30-minute lifetime (sqlx-core 0.9.0). The
playback monitor writes the resume position every 30 s of playback (`tasks/playback_monitor.rs`),
so while music plays the connection never goes idle and the cache a scan filled stays until the
lifetime runs out. A shorter `idle_timeout` would only help a player that is stopped.

- [ ] Run `PRAGMA shrink_memory` on the write connection at the end of a bulk scan. It hands back
      the clean page cache without closing the connection, so there is no reopen and nothing about
      insert speed changes.
- [ ] Consider `mmap_size` like the read pool, and a smaller `cache_size`, only if the phase 0 ingest
      timing allows it.

Memory: up to about 16 MB for up to 30 minutes after a scan at 50k (estimate).
Risk: low for the pragma; bulk insert speed for the other two. Time an ingest on the phase 0 library
first.

### Phase C: decode detail heroes at the hero's size

Where: `crates/melodia-views/src/ui/detail_artwork.rs` (a 12-entry LRU per detail type),
`crates/melodia-views/src/ui/util.rs` (`COVER_SIZE`).

Hero covers decode at `COVER_SIZE` (384 px) for a 140 px square. `util.rs` argues that one decode
size across the app is worth more than the buffers it would save "on surfaces holding a single
image", and a 12-entry LRU per detail type is not one.

- [ ] Decode heroes at the 140 px square times the scale, stepped.
- [ ] The Edit Artwork dialog opened from a playlist detail hands `PlaylistDetail.cover`, the hero
      buffer, to its 220 px preview (`playlists/callbacks/detail.rs:299-304`,
      `playlist-mosaic-picker.slint:49-50`). A 160 px hero would draw there upscaled 1.375× at 1×.
      The dialog decodes its own cover at the preview's size, or this phase does not go in.

Memory: 442,368 B to 76,800 B per hero at 1× (160 px), up to about 4.2 MiB per detail type at the
LRU cap.
Risk: reverses a documented decision, and the dialog above is the surface that decision protects
today. The call is whether one size across the app still earns its buffers.

### Phase D: cap folder-watch batches

Where: `crates/melodia-app/src/tasks/file_event_processor/` (`mod.rs:106`, `reconcile.rs`,
`dedup.rs`).

Everything that arrives inside the 500 ms window becomes one batch, and its extraction holds every
file's tags at once. The window runs from the first event, and the 256-slot channel only slows the
watcher; nothing caps the batch.

- [ ] Cap a batch at about 2,000 events. A delete pairs with its create by content hash, which is
      not known until extraction, so the rule the cap can hold is that a `Removed` never lands in an
      earlier batch than any `Created`: carry every remove to the last capped batch. That is what
      carries a cross-device move across with its ratings, play counts and favourites intact. A
      remove landing later than its create is harmless, `process_batch` already ordering deletes
      last within a batch. Today's window can split a pair by timing alone.

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
