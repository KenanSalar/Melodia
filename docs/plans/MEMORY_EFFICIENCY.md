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
- **Shader cache:** launch every new binary once, unmeasured, before its first measured run. The
  NVIDIA driver keys its shader disk cache to the executable, so a binary's first launch compiles
  FemtoVG's shaders in-process, and what the compile allocates stays live for the whole session.
  On the phase 0 library on 2026-10-05 that was 101.5–104.0 MiB of Anonymous with the cache warm
  against 140.2–146.7 MiB on a fresh copy of the binary or under `__GL_SHADER_DISK_CACHE=0`. It is
  expected and not Melodia's to fix: glibc counts it as in use rather than held free,
  `malloc_trim(0)` returned under 0.5 MiB of it, and `glReleaseShaderCompiler` with the context
  current returned none. A user on that driver should meet it once per update, on the new binary's
  first launch. `tools/trim_probe.sh` and `tools/compiler_release_probe.sh` in
  `~/Development/melodia-measure` reproduce both halves.

None of these scenarios has a grid page up, the queue sheet open or a scan running, which is where
most of this plan acts. Phase 0 adds those states to the baseline.

## Checkpoints

Each checkpoint is a full run of the baseline protocol, the phase 0 states included, written into
the table below beside the baseline. A phase's own re-measurement (Every phase) is a spot check of
the state it targets, taken to catch a regression early; it does not replace a checkpoint.

1. **After phases 1 to 7.** Once every phase in the main list is in, phase 7 excepted, which was
   dropped. This is the number the plan is judged on.
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
database. `~/Development/melodia-measure/tools` holds what took the phase 4 readings, kept outside
the repo for these runs: the library generator, a first-scan and forced-rescan timer
(`measure_scan.py`), the dev-root Idle runner (`dev_idle.sh`, which restores its snapshot over
the dev root before every run, so a session starts with `dev_idle.sh snapshot` and ends with
`dev_idle.sh restore`), and the cover-decode timer (`decode-check/`).

`scan_rounds.sh` there drives a scan spot check, phase 5's being the first: it warms the page cache,
sets one file an hour older, runs the first scans with the launch order rotated each round,
restamps every file to one time and runs the rescans. The older file is what lets a rescan be
timed at all: `measure_scan.py` ends a rescan once every row carries the newest `date_modified`,
which is stored to the second, so a library already restamped to one time passes that check at
launch. Edit its builds, rotation and output names before a run.

| Run | First scan of the large library | Peak RssAnon | Wall time | Threads after |
|---|---|---|---|---|
| Baseline, phase 0 | 50,400 tracks, 0.18.0, three runs | 189–197 MiB | 2.97 s cold, 2.56–2.58 s | 45–46 |
| After 1–7 | | | | |

Production files the phases touch, in lines (`wc -l`): `cover_thumbs.rs` 622, `folders.rs` 494,
`now_playing/mod.rs` 472, `visualizer.rs` 441, `track_list_cache.rs` 389, `grid_prewarm.rs` 358,
`ui/visualizer/mod.rs` 337, `tracks/mod.rs` 168, `queue_sheet/mod.rs` 167, `scanner.rs` 118,
`callbacks/tags/artwork.rs` 98, `ui/util.rs` 78, `main.rs` 569, `playlists/callbacks/dialog.rs` 544,
`queries/ingest.rs` 484, `shell/bridge.rs` 392, `database/mod.rs` 341,
`boot/ui_setup/views.rs` 268, `engine/backend/mod.rs` 782, `queue_sheet/callbacks.rs` 461,
`now_playing/up_next.rs` 291, `shell/mini_player.rs` 159.

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
- [x] Build a large library outside the repo for phases 4 to 6: ffmpeg-generated short files with
      tags spread over a few thousand albums, 50k tracks or more. Built 2026-10-05 at
      `~/Development/melodia-scale-library`, deliberately not under `~/Music`, which a first launch
      adds: 50,400 one-second MP3s, 840 artists × 5 albums × 12 tracks, each copied from one
      ffmpeg-rendered file and given distinct ID3v2.4 tags (title, artist, album artist, album,
      track, genre, year). No embedded covers.
- [x] Fill the baseline row of the scan table from a first scan of that library into an empty
      database: the installed 0.18.0 RPM, `MELODIA_RSS_SAMPLE=1` sampling every 500 ms.
- [x] `playlists/callbacks/dialog.rs:68` quotes "~603 KiB" for the cover buffer, a figure left from
      the old 448 px tier. Drop the number and keep what the comment argues. Its twin in
      `playlist-mosaic-picker.slint` ("the grid tier (448 px)") went with it.

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
Risk: the cards on single-row grids. `GridGeometry`'s `lone-row-cards` draws a grid that fits on
one row at `MAX_CARD_W`, a 208 px tile and 216.3 px hovered, while the tier is sized off the widest
card the column band draws. At 1× that puts the new tier under the drawn tile for logical widths
2452 to 3651, the baseline machine included, where today only 3652 to 3840 fall short. A filtered
grid, a handful of playlists and Favorites ▸ Artists all draw one row, and FemtoVG upscales
bilinearly, so every window where this phase saves memory is one where those grids soften. The
tier is shared, so sizing it for a lone row gives the saving back wherever one draws. Decide which
before starting. The two surfaces above draw the buffer at something other than the card's tile
too.
Tests, when asked: sweep `grid_prewarm_tests` over widths and scales, asserting the decode never
falls under the drawn tile.

Sizing pass, 2026-10-05:

- The tier moves only on these logical widths: 1× 2452–3651 (224 → 192), 1.25× 1652–2051
  (288 → 256), 1.5× 1452–1651 (352 → 320) and 2452–3651 (320 → 288), 2× 1652–1851 (448 → 416) and
  2452–3651 (416 → 384). 1920×1080 at 1× and 4K at 2× don't change.
- `cover_size`'s `as u32` truncates. At 1.25× for logical 1652–1851 the new formula lands 0.1 px
  under the drawn tile, so round up before the cast.
- `grid_prewarm_tests`' `the_tier_covers_the_card_at_every_sidebar_width` asserts the tier covers
  the whole card and breaks; it becomes tier ≥ (card − 2 × inset) × zoom × scale.
  `assert_eq!(cover_size(500, 1.0), MAX_CARD_W)` passes only through the step's rounding.
- A seventh zoom spelling: `genre-grid.slint:68` sets `1.15` with no condition. Search's strips and
  Radio's station card take the default. Slint can't override one arm of a ternary, so "one
  spelling" needs a card property: an opt-in the hosts set, or the card deriving 1.15 from an
  empty `artwork-path`, which also changes Search and Radio.
- The mosaic picker's preview cells read the grid tier too (`CoverMosaic` →
  `Playlists.request-cover` → `grid_cover`): one pick fills 220 px, two about 214 px each. The
  Edit Artwork decision covers them as well. Decoding the dialog's own cover takes `dialog.rs` out
  of `cover_generation.rs`'s `BLOCKING_LOOKUP_SITES`, an exact list.

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

Sizing pass, 2026-10-05:

- `queue_sheet::install` is called from `crates/melodia/src/main.rs:285`, not from
  `boot/ui_setup/views.rs`. Pass `views.cover_thumbs` there, as `now_playing::install` does beside
  it; `views.rs` doesn't change.
- Most of the double decodes on open come from the open-time prewarm in
  `queue_sheet/callbacks.rs:328-346` (up to 24 covers), not from the misses after it. The phase
  covers both.
- `wire_callbacks` already takes 8 parameters, clippy's threshold, so the shared tier rides in a
  bundle rather than as a ninth.
- `CoverThumbs` has no public non-promoting lookup and no public insert. `holds()` is private, and
  neither tier exposes its size.
- The sheet's tier is tuned once at install, while the shared row tier retunes on every display
  change. After a move across the 1.25× boundary the two sizes differ and an "only at the asked
  size" lookup refuses every match. Fix the retune here or accept the misses.
- Also stale: `ui-patterns.md:772-773` (calls the sheet's tier private on decode size, which it
  isn't) and `globals/queue.slint:41`.

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

Sizing pass, 2026-10-05:

- Rings: a `OnceLock` can't be cleared through `&self`, so once armed the rings stay for the
  session; the saving is the sessions that never open the visualizer. `new(true)` stays eager,
  about 20 tests in `playback/tests/visualizer_tests.rs` pushing straight into it.
  `engine/backend/mod.rs` sits at 782 lines, so its edit stays a comment.
- Analyzers: going by the buffers in `waveform.rs` and `spectrum.rs`, the waveform analyzer alone
  looks near 87 KiB and the spectrum one at least 120 KiB, above the audit's 110 to 170 KiB for
  both. The measurement decides.
- Now Playing pair: three release sites, not one (`now_playing/up_next.rs:145`,
  `shell/mini_player.rs:43-47` in `follow_artwork`, and `:92` on the miniplayer exit), and the
  `follow_artwork` closures hold no `weak` today. `globals/player.slint:130` says "Neither slot is
  ever cleared" and needs the same update as the rule. `release_hero_slots!`
  (`callbacks/macros.rs:165-172`) already clears the detail globals' blur pairs, so the rule's
  "one such site" is untrue today. The miniplayer drain keeps the stack mounted after
  `set-shown(false)`, so clear only the inactive slot, or defer the clear by at least `dur-med`
  and re-check `renders_artwork()`.
- Tag preview: `decode_cover_preview` has a second caller, `tags/open.rs:114`, which decodes the
  track's stored artwork as the dialog opens and gains the same. `decode_jpeg_scaled`'s doc
  ("Every caller passes a store path") stops being true. `bounded_decode.rs` walks three spellings,
  `ImageReader::open(` among them, and stays green. Only `cover_thumbs.rs:599` pairs
  `large_decode_guard` with `decode_capped_to` today.

## Phase 4: run scan work on a pool that ends with the scan

Status: implemented and spot-checked 2026-10-05; its rows wait for the checkpoint after 1–7.

Where: the global-pool `par_iter`s at `library/settings/folders.rs:274`,
`media/ingest/scanner.rs:53`, `database/queries/ingest.rs:335`,
`tasks/file_event_processor/reconcile.rs:63`, `tasks/rating_import.rs:124` and
`tasks/retroactive_hash.rs:41`, plus `library/import.rs` (drag-and-drop, open-with and playlist
import), which reaches `scanner.rs:53` and `ingest.rs:335` through the same two functions.

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
warm at boot (first through `boot/ui_setup/hydrate.rs:51` → `warm_vm_cover`, then
`shell/bridge.rs:98`, both on the blocking pool), Now Playing and the detail heroes
(`artwork_cache.rs:113`), Material You, and the inline decodes the queue sheet and
`grid_cover_blocking` run on the UI thread. Any of those brings back all 16 threads.

Rayon serves injected work only after local and stolen work, so while a scan saturated the global
pool every one of those UI-thread decodes waited for it to drain. Moving scans off that pool ends
the wait as well.

- [x] Build rayon's global pool small and named at startup: two threads (`rayon-{i}`,
      `GLOBAL_RAYON_THREADS`), built in `main.rs` right after the runtime and ahead of
      `AppState::init`, so `jpeg-decoder` keeps a home that costs two threads. The binary gained a
      direct `rayon` dependency for it. The two alternatives each cost something this one doesn't.
      Turning off `jpeg-decoder`'s default features spawns and joins OS threads per decode, the
      churn `TAG_WRITE_POOL`'s doc argues against. Routing those decodes through `cover-decode`
      queues the UI thread's inline decodes behind a grid prewarm.
- [x] `ScanPool` (`melodia-store`'s `media/ingest/scan_pool.rs`) owns the scoped pool: one thread
      per file up to one per core, named `scan-{i}`, ended by the last clone's drop. A pass over no
      files, and a failed build, run on the global pool.
- [x] A scan's three stages are separate `spawn_blocking` closures with `.await`s between them
      (`folders.rs:271`, `:329`, then `ingest.rs:333` per chunk), so no single `install` covers
      them. The walk closure builds the pool once it knows the file count and hands it out; the
      parse closure takes a clone, and `ingest_scanned_files` takes `&ScanPool`. `import.rs` does
      the same. Both drop it after the ingest, ahead of the stats recalc.
- [x] `RUNTIME_NAMED` in `crates/melodia/tests/packaging.rs` holds four files now:
      `tags.rs`, `cover_thumbs.rs`, `scan_pool.rs` and `main.rs`.
- [x] Small watcher batches pay only for what they use: the pool is sized to the batch, so one file
      costs one thread for its length. That replaced the threshold this item first asked for.
- [x] `TAG_WRITE_POOL` (`library/tags.rs:49`) keeps its four threads after the first tag write, now
      named `tag-write-{i}` rather than inheriting `melodia-bg` from the blocking thread that built
      it, and `cover_thumbs.rs`'s three global-pool fallbacks stay. Docs updated for both.
- [x] Docs: `image_decode.rs`'s scratch-cap wording, `cover_thumbs.rs`'s and `tags.rs`'s
      "`num_cpus`-wide global pool", `rating_import.rs`'s `PAGE_ROWS`, `CLAUDE.md`'s and
      `melodia-artwork/Cargo.toml`'s "the one rayon pool that names its threads", and
      `.claude/rules/rayon.md`'s Thread Pool section.
- [x] Spot check, 2026-10-05, the installed 0.18.0 RPM against this branch's release build. The
      512-track rescan was swapped for a forced rescan of the phase 0 library, since an unchanged
      rescan finishes too fast to time.
      - **Idle on the dev library** (footprint script, baseline protocol): threads 45 → 31,
        `melodia-bg` 19 → 3 beside `rayon-0` and `rayon-1`, memory maps 574 → 540, Anonymous
        34.3 → 33.9 MiB, USS 87.7 → 86.2 MiB, CPU 0.07% → 0.08%.
      - **Now Playing opened** on the dev library: 47 threads against 31. Neither build's rayon
        pool grew; 0.18.0's one extra was a tokio blocking thread (`melodia-bg` 20 → 21), which
        tokio drops after 10 s idle. The open
        itself wasn't timed in the app, which reports nothing for it; the decode it waits on, the
        one part this phase touches, was timed outside the app over the 190 stored covers × 20
        with a 2- and a 16-thread global pool: median 463–472 µs against 441–474 µs, p99
        1.51–1.53 ms against 1.48–1.83 ms.
      - **First scan of the phase 0 library** into an empty root, three alternating runs each,
        from the "Auto-added" log line to "Watching folder": 2.97 (cold), 2.58, 2.56 s against
        2.55, 2.48, 2.52 s. Peak RssAnon 189–197 against 191–195 MiB. `scan-*` peaked at 16
        during the pass; threads after it 45–46 against 31–33.
      - **Forced rescan** (every file restamped, re-parsed by the boot reconcile): 4.34, 4.30,
        4.17 s against 4.23, 4.16, 4.03 s. Peak RssAnon 242–247 against 239–246 MiB.
      - Not exercised: the resizer scratch a scan worker now hands back. The phase 0 library
        carries no covers, so no scan resized one.
      - Watch at the checkpoint: RssAnon settled after a first scan at 116–131 MiB on this branch
        against 117–119 MiB on 0.18.0, and after a rescan at 134–150 against 145–155 MiB. Three
        runs show no trend either way.

Memory: 14 fewer idle threads on this machine (the global pool's 16 become two), their touched
stacks, and up to 32 MiB of resizer scratch after a scan that stored oversized covers. All three
baseline rows already carry the 16, so Idle should move.
Cost: one pool build per pass. Two passes running at once (the boot reconcile beside the rating
import) each get a full-width pool where they used to share one.

## Phase 5: parse and ingest a scan in chunks

Status: implemented and spot-checked 2026-10-05; its rows wait for the checkpoint after 1–7.

Where: `crates/melodia-app/src/library/settings/folders.rs` (`scan_folder_internal`,
`TX_CHUNK_FILES`), and the full re-read `tasks/tag_backfill.rs` triggers.

The scan parses the whole folder into one `Vec<ScannedFile>` and only then writes it in
`TX_CHUNK_FILES` (2,000) transactions. An `ExtractedMetadata` carries 22 text fields of its own, 11
more in `SortTags` and `ReleaseTags`, and six variable-length lists of names and IDs. The cover is
written to the store during the parse, so no picture bytes ride along. A first scan of a 50k
library holds an estimated 75 to 100 MB of parsed tags at its peak.

- [x] Parse and ingest per 2,000-file chunk, so the peak follows the chunk rather than the library.
      `scan_folder_internal` takes `TX_CHUNK_FILES` paths at a time off `to_scan`, parses them
      (`parse_chunk`) and ingests them before taking the next. No overlap: it was built and
      measured (below), and bought no time for about 5 MiB more.
- [x] Moved files keep their ratings and play counts. On the scan path that is
      `ingest_scanned_files`, not the watcher's `process_batch`: it matches each chunk's hashes
      against the whole table, and the orphan purge runs once, after the last chunk. Chunking the
      parse keeps that as long as the purge stays after every chunk. The ingest already committed
      per 2,000 files, so its chunks see exactly what they saw before.
- [x] Progress keeps its total, which is known before the parse, and orphan detection keeps its full
      path list. `ScanProgressReporter` counts each chunk's ticks against the whole scan.
- [x] `folders.rs:402` copies every on-disk path into a `String` set (`to_string_lossy`) while
      `files`, never read again, stays alive beside it. The set now takes the buffers out of
      `files` (`into_string_lossy`).
- [x] Spot check, 2026-10-05, on the phase 0 library: two passes of `measure_scan.py`, each three
      rotated rounds of a first scan into an empty root and a forced rescan (every file restamped
      once), against the installed 0.18.0 and the phase 4 release build (the 12:38 binary behind
      phase 4's figures). Pass 2 adds the overlapped variant as a fourth arm. The script polls for
      the end every 0.1 s, so wall times are only comparable past that.
      - **First scan:** peak RssAnon 68.4, 68.6 and 117.9 MiB against 187.4–192.3 on 0.18.0 and
        191.1–193.2 on phase 4. Wall 2.39 s on every run, against 2.39–2.49 and 2.39. Pass 1:
        68.6, 69.1 and 117.5 against 184.0–191.7 and 186.8–237.6. Each pass has one high
        round-1 run, consistent with a binary's first launch compiling its shaders (Shader
        cache, under the baseline's setup).
      - **Forced rescan:** peak 144.0–148.5 MiB against 242.3–247.5 and 243.2–252.9. Wall
        3.88–4.00 s against 3.88–4.00 and 3.85–3.97. Pass 1 read 142.3–142.8 MiB and 4.22–4.33 s
        against 3.86–4.24 for the other two. Pass 2 was run to check that time gap, and it
        didn't reproduce.
      - **Settled 10 s after:** first scan 114.3–164.5 MiB against 114.9–132.3, rescan
        137.7–140.6 against 142.4–159.7.
      - **Overlap** (the next chunk parsing while this one is written): first scan 73.6–73.7 MiB,
        rescan 149.4–150.4 MiB, wall 2.38–2.39 and 3.97–4.00 s. Not kept.
      - **Threads:** as phase 4. `scan-*` peaks at 16, 30–33 threads after against 43–47 on
        0.18.0.
      - **Watcher:** every 0.18.0 and phase 4 rescan overflowed the kernel's inotify queue and
        asked for a full rescan, which the running reconcile absorbed. No phase 5 rescan did,
        serial or overlapped: the scan's own file reads now arrive a chunk at a time.

Per run, rounds 1 / 2 / 3, the launch order rotating by one build each round. "Settled" is RssAnon
10 s after the scan ended. Raw output is `phase5-results.jsonl` (pass 1) and
`phase5b-results.jsonl` (pass 2) in `~/Development/melodia-measure`, with the four binaries in its
`bin/`.

| Pass | Scan | Build | Peak RssAnon, MiB | Wall, s | Settled, MiB | Threads after |
|---|---|---|---|---|---|---|
| 1 | First | 0.18.0 | 189.1 / 191.7 / 184.0 | 2.68 / 2.58 / 2.59 | 121.9 / 131.6 / 134.3 | 44 / 43 / 45 |
| 1 | First | phase 4 | 237.6 / 195.7 / 186.8 | 2.57 / 2.59 / 2.59 | 166.2 / 131.1 / 123.8 | 31 / 31 / 32 |
| 1 | First | phase 5 | 117.5 / 68.6 / 69.1 | 2.58 / 2.49 / 2.49 | 165.3 / 117.2 / 118.6 | 32 / 32 / 32 |
| 1 | Rescan | 0.18.0 | 243.8 / 242.8 / 249.8 | 4.24 / 4.10 / 3.99 | 148.6 / 153.1 / 152.8 | 46 / 47 / 47 |
| 1 | Rescan | phase 4 | 240.1 / 251.1 / 246.0 | 4.11 / 4.19 / 3.86 | 136.5 / 153.6 / 151.7 | 32 / 32 / 32 |
| 1 | Rescan | phase 5 | 142.5 / 142.3 / 142.8 | 4.23 / 4.33 / 4.22 | 142.8 / 137.2 / 140.4 | 33 / 33 / 32 |
| 2 | First | 0.18.0 | 192.3 / 187.4 / 190.6 | 2.49 / 2.39 / 2.49 | 118.8 / 117.5 / 132.3 | 44 / 46 / 45 |
| 2 | First | phase 4 | 191.2 / 193.2 / 191.1 | 2.39 / 2.39 / 2.39 | 117.2 / 122.6 / 114.9 | 33 / 31 / 30 |
| 2 | First | phase 5 | 117.9 / 68.6 / 68.4 | 2.39 / 2.39 / 2.39 | 164.5 / 122.7 / 114.3 | 30 / 31 / 32 |
| 2 | First | phase 5, overlapped | 73.6 / 73.6 / 73.7 | 2.38 / 2.39 / 2.39 | 116.4 / 114.0 / 115.8 | 33 / 32 / 32 |
| 2 | Rescan | 0.18.0 | 247.5 / 242.3 / 242.5 | 4.00 / 3.88 / 4.00 | 153.4 / 152.4 / 142.4 | 44 / 44 / 47 |
| 2 | Rescan | phase 4 | 243.2 / 245.9 / 252.9 | 3.97 / 3.85 / 3.87 | 148.7 / 142.7 / 159.7 | 32 / 31 / 32 |
| 2 | Rescan | phase 5 | 148.5 / 146.2 / 144.0 | 3.88 / 4.00 / 3.98 | 137.7 / 139.5 / 140.6 | 33 / 33 / 33 |
| 2 | Rescan | phase 5, overlapped | 149.4 / 150.4 / 149.6 | 4.00 / 3.99 / 3.97 | 146.5 / 147.8 / 147.2 | 33 / 33 / 33 |

Memory: the first-scan peak goes from following the library to following one 2,000-file chunk,
measured above at about 120 MiB less on a first scan of 50,400 files and about 100 MiB less on a
forced rescan. Nothing changes at idle.
Risk: low to medium, on throughput. Measured above: no change past the script's resolution.

## Phase 6: share repeated strings in the Songs list

Status: implemented and spot-checked 2026-10-05; its rows wait for the checkpoint after 1–7.

Where: `crates/melodia-views/src/ui/track_list_cache.rs` (`convert`),
`crates/melodia-views/src/ui/tracks/mod.rs` (`to_slint_track_list_row`), and the other
`unwrap_or("")` row conversions: `shell/bridge.rs`, `queue_sheet/rows.rs`, `albums/mod.rs`,
`artists/mod.rs`, `playlists/mod.rs`, `search/mod.rs`, `favorites/rows.rs`,
`recently_played/rows.rs`, `browse/cards.rs`, `browse/mod.rs` and `now_playing/metadata.rs`.

Each row allocates its own artist, album, genre, artwork path and duration string, and
`SharedString::from("")` allocates where `SharedString::default()` does not (i-slint-core 1.16.1,
`string.rs`): a 25 B request for the header and the NUL, a 48 B glibc chunk.

- [x] Intern within one conversion pass: a local map from text to `SharedString` for the fields an
      album's tracks share, so they hold one string each. The Songs pass runs on a runtime worker,
      so the UI thread pays nothing there. Favorites' refresh on `library_changed` and
      `stats_changed` converts on the UI thread (`spawn_local`, `favorites/callbacks/lifecycle.rs`),
      as do the detail filters, so time the map there or keep it to the Songs pass.
      Built as `ui::util::StringPool`, a `HashSet<SharedString>` looked up by `&str`, and handed to
      `to_slint_track_list_row` for artist, album, genre, artwork path and duration. The timing
      (below) put a pooled pass under an allocating one, so it runs on every pass, the UI-thread
      ones included. `to_slint_track_list_rows` owns the pool for a whole list, and Browse has the
      same pair.
- [x] One helper in `ui/util.rs`, beside the other row conversions, turns an optional field into a
      `SharedString` and answers `default()` when it is empty. Every `unwrap_or("")` site goes
      through it. It replaces `bridge.rs`'s private `opt_shared`, which allocates on `None`;
      `radio/rows.rs` already spells the allocation-free form inline.
      `opt_shared` takes `Option<impl AsRef<str>>`, so the Now Playing chips' formatted values
      and `radio/rows.rs`'s inline form go through it too. Every `SharedString::from("")` and
      `"".into()` became `SharedString::default()`, `updater_daily.rs` in `melodia-app` included,
      and Browse's disk-only row fills from `TrackListRow::default()`. The one-off writes that
      spelled the same thing as `unwrap_or_default()` take it as well: the Output card's device
      name, the two scrobbling usernames, the radio form's four seeds and the tag editor's
      credit previews and Summary strings.
- [x] The detail conversions take the same interner unless phase A below lands first.
- Not interned: the queue sheet. `queue_sheet/rows.rs` rebuilds every row on the UI thread on
  each queue mutation, every frame of a drag included, and the sheet holds its rows only while
  open. It takes `opt_shared` only.
- [x] Spot check, in the app, 2026-10-05: Idle on My Library ▸ Songs with the phase 0 library,
  the release build of `c18ae28` against this phase's, three rounds each with the order
  alternating (B A, A B, B A). `tools/songs_idle.sh` copies the scanned root `p5b-serial-1` over
  a working root before every run, switches the three online services off, and drives the
  footprint script under the baseline protocol on the 144 Hz screen. Raw output is
  `songs-idle-{before,before-2,before-3,after-1,after-2,after-3}`.
  - **Anonymous:** median 118.5 → 104.8 MiB (−13.7), close to the 12.7 MiB the tool measured
    for the Songs pass. Each round's pair moved the same way: −19.1, −21.6 and −10.8 MiB.
  - **USS:** median 165.8 → 152.0 MiB. RSS moved both ways (449, 394, 440 against 481, 373,
    429 MiB), being mostly file-backed.
  - **CPU, threads:** 0.13–0.15% against 0.12–0.15% of one core; 30–33 threads against 29–31.
  - **Round 1** read about 50 MiB higher than the other two on both builds (168.5 and 149.4
    MiB), each binary's first launch: consistent with the shader compile (Shader cache, under the
    baseline's setup). The before/after gap held through it.
  - **The dev library** (`dev_idle.sh`, Idle on My Library ▸ Songs, one run each): 32.5 → 32.3 MiB
    Anonymous, USS 77.2 and 77.4 MiB, 31 threads on both. At 512 tracks the saving is about
    0.13 MB, under what a median sample resolves.

| Round | Build | Anonymous | USS | RSS | Peak RSS | Threads |
|---|---|---|---|---|---|---|
| 1 | `c18ae28` | 168.5 MiB | 216.3 MiB | 449.2 MiB | 484.1 MiB | 33 |
| 1 | phase 6 | 149.4 MiB | 197.5 MiB | 481.1 MiB | 511.5 MiB | 31 |
| 2 | phase 6 | 96.9 MiB | 144.1 MiB | 372.7 MiB | 405.3 MiB | 29 |
| 2 | `c18ae28` | 118.5 MiB | 165.8 MiB | 394.4 MiB | 421.8 MiB | 31 |
| 3 | `c18ae28` | 115.6 MiB | 163.1 MiB | 440.3 MiB | 467.2 MiB | 30 |
| 3 | phase 6 | 104.8 MiB | 152.0 MiB | 429.1 MiB | 456.6 MiB | 30 |

- [x] Spot check, outside the app, 2026-10-05: `tools/intern-check` in
  `~/Development/melodia-measure` runs today's conversion of the six text fields against the
  pooled one over the phase 0 library's 50,400 rows, exported in `sort_key` order, 20 rounds
  each with the order alternating. A counting allocator reports what a pass leaves held, in
  glibc chunks. The "with covers" rows give each album a store-shaped artwork path and spread
  the durations over 2 to 8 minutes, since the phase 0 library has no covers and one duration.
  Raw output is `phase6-intern-check-1.txt`.

| Rows | Held, today | Held, pooled | Saved per row | Pass and drop, today | Pass and drop, pooled |
|---|---|---|---|---|---|
| Songs, 50,400 | 18.11 MiB | 5.38 MiB | 265 B | 16.5 ms | 10.5 ms |
| One genre, 3,360 | 1.21 MiB | 0.37 MiB | 261 B | 0.96 ms | 0.68 ms |
| One album, 12 | 73 allocations | 18 allocations | 249 B | 3 µs | 2 µs |
| Songs with covers, 50,400 | 20.41 MiB | 5.77 MiB | 305 B | 17.4 ms | 11.5 ms |
| One genre with covers, 3,360 | 1.36 MiB | 0.40 MiB | 299 B | 1.01 ms | 0.76 ms |

Times are medians; each p99 sat within 0.7 ms of its median.

Memory: measured above at 265 B per track on the phase 0 library and 305 B once albums carry
covers, the empty fields included; 13.7 MiB of Anonymous at idle on its 50,400 tracks. The cache
holds every row and the Slint model holds clones sharing the same buffers, so the saving is
counted once. Each pass is also about a third faster, the pool's lookup costing less than the
allocation it replaces.
Risk: low. The strings are never mutated, and `SharedString` compares by content.

## Phase 7: page the ListenBrainz backfill

Status: dropped 2026-10-05. It was implemented and passed the gate, then reverted before any
commit, because the feature it pages is being removed: the auto-tagging matches on tag text and
writes what it guessed into the user's files, and only a track's audio identifies it reliably. The
removal takes the sweep, its query and the attempted set with it, so the memory this phase
targeted goes away entirely.

What was built, should the feature return: keyset pages of 2,000 with the cursor taken off the page
as read (so a page of attempted rows doesn't end the walk), and the attempted set loaded on the
first sweep that has a token, through the async `load_json_or_default`, with the kick still
clearing it without a token.

Where: `crates/melodia-store/src/database/queries/track/lookup.rs` (`get_tracks_missing_mbid`),
`crates/melodia-app/src/tasks/mbid_backfill.rs`.

With MusicBrainz auto-tagging on and a ListenBrainz account connected (`mbid_lookup_token`), every
library change loads every eligible track and then drops, in memory, the ones already attempted.
A library change here includes every favourite and rating toggle, not only scans. `attempted` is
also loaded at boot with the feature off, which costs something only once
`scrobble_mbid_attempted.json` exists.

- [ ] Page by id, as `get_unrated_track_paths_after` does for the rating import. Dropped.
- [ ] Load `attempted` only once a token exists. Dropped.

Memory: an estimated 10 MB transient per library change at 50k, and only with the feature on. A row
is about 350 to 400 B, so that figure assumes 25k to 35k tracks without an MBID.
Risk: low.

Sizing pass, 2026-10-05:

- Advance the cursor off the raw page's last id, not the filtered page's, or a page of rows already
  attempted ends the walk. `rating_import_tests`' `a_page_holding_no_ratings_does_not_end_the_walk`
  is the test to copy.
- `Abandoned`, the 429 retry and shutdown have to break the outer page loop, and `looked_up`
  accumulates across pages so `summarize` still works.
- `mbid_backfill_tests` pins the literal `"async fn run_sweep"` and forbids `bump(` in the file.
- The kick clears and persists `attempted` whether or not a token exists. Decide whether it still
  does once loading waits for a token.
- Paging bounds the peak, not the work: every library change still walks the eligible table, a page
  at a time.
- `load_attempted` reads through blocking `std::fs` on a runtime worker; `atomic_file.rs:36` has the
  async twin.

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

When phases 1 to 6 are in and checkpointed, and each optional phase is either in and checkpointed or
declined, this plan is deleted, and `~/Development/melodia-measure` and
`~/Development/melodia-scale-library` go with it.

The binaries in `melodia-measure/bin/` sit outside `target/`, so each launch counts as a tarball
install and rewrites the per-user launcher, `~/.local/share/applications/com.github.kenansalar.melodia.desktop`,
and its metainfo, `~/.local/share/metainfo/com.github.kenansalar.melodia.metainfo.xml`, to point at
itself. Both shadow the RPM's own entries, so once `bin/` is gone the app-menu entry points at a
missing binary. They are deleted with `melodia-measure`, and only after Kenan says yes: ask first,
naming both paths, and never remove them as part of a cleanup step on your own.
