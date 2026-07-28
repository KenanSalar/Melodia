# Fix per-track RSS growth (artwork decode churn + glibc arena retention)

Working doc for `fix/general-improvements`. Delete when the work ships.

## ⚠ The premise below did not survive measurement — read this first

Phase 1 has now been run (2026-07-28, debug build, 25 skips with Now Playing **open**).
**The per-track story is not what was reported.** Corrected account from the person who
saw it: the app sat at ~160 MB, was left **playing continuously for hours**, and was at
~400 MB on return. That is growth over *time under playback*, not over *distinct tracks*.

Measured, three ~95 s runs, same protocol:

| run | peak `VmRSS` | `RssAnon` spread | `RssFile` spread |
|---|---|---|---|
| A — baseline | 240.7 MiB | 11.0 MiB | 10.0 MiB |
| B — `GLIBC_TUNABLES` mmap pin | 230.0 MiB | 7.6 MiB | 9.6 MiB |
| C — Phase 2 compiled in | 228.7 MiB | 6.9 MiB | 9.6 MiB |

Consequences for the rest of this doc:

- **25 distinct tracks moved `RssAnon` by 11 MiB, not ~175 MB.** The Synthwave playlist
  used for the runs holds 25 tracks over **21 distinct covers** (10 × 500², 9 × 1280×720,
  one 640×480, one 500×349), so the artwork genuinely varied — 32.3 MiB of RGB8 if each is
  decoded once, ~65 MiB for the two decodes per track this doc describes. Only ~11 MiB of
  that was retained. **The "~7.4 MB/track" model is wrong because glibc reuses free-list
  chunks**: covers cluster into a few size classes, so retention plateaus at about one
  chunk per class, not one per decode. The doc's own "then plateaus" wording is the tell.
  **Phase 3's entire justification rests on that model** — do not implement it.
- **`RssFile` is a one-time fill, not growth.** Split by quartile it does ~+9.5 MiB in the
  first quarter of a run and ~+0.2 MiB across the remaining three, in every run, with and
  without the fix. Mesa's GPU pool reaching steady state during first paint. Do not count
  it as a leak and do not reorder the phases around it.
- **`RssAnon` has a sustained component the fix does not reach.** Per quartile, runs B/C go
  +2.1, +1.5, +0.5, +0.8 MiB — decaying, but flattening onto a *nonzero* floor of roughly
  0.7 MiB per 23 s ≈ **1.8 MiB/min ≈ 110 MiB/hour**. Extrapolated over the ~2 h of the real
  observation that is ~220 MiB, against the ~240 MB reported. This is the actual bug, it is
  in the heap, and Phase 2 is already compiled in for those runs. **Caveat: Q4 is 23 s of
  data and the extrapolation is ~300×** — it needs a ~20 min capture under *normal*
  playback (no skipping) to confirm the rate and its linearity.
- The first two attempts at Phase 1 were wasted because the protocol never said to open
  the Now Playing view. **It must be open** (press `F`), or the 384 px decode this doc
  indicts never runs at all — it is gated at `src/ui/now_playing/track_change.rs:80`.

### The real bug, isolated (runs D/E, 15 min each, normal playback, no skipping)

| run | Now Playing | `RssAnon` slope, back half |
|---|---|---|
| D | **open** (mirrored bars) | **+0.87 MiB/min → +52 MiB/hour** |
| E | closed, never opened | +0.15 MiB/min → +9 MiB/hour |

D climbs **linearly with no plateau** across the whole run with the track never changing;
E is flat for its first three-fifths. So the core playback machinery is clean and the leak
is behind the Now Playing view. At ~52 MiB/hour the reported ~2 h / ~240 MB session is the
right order of magnitude.

**Do not conclude "the visualizer leaks" from this.** D repaints at ~60 fps (the strip
writes every band every tick *specifically* to force repaints — see
`.claude/rules/visualizer.md`) while E repaints at ~2 fps off the position tick. That
confounds the visualizer's own allocations with anything that leaks **per repaint**, and no
further open/closed A/B can separate them. ~0.87 MiB/min at 60 fps is only ~240 bytes per
frame — small and relentless, which is the profile of a per-repaint retention, not of a
per-frame buffer the analyzers already own (`src/ui/visualizer/frame.rs` documents the bars
path as allocation-free, and it reads that way).

### Root cause, from heaptrack (8.7 min, Now Playing open, mirrored bars)

```
calls to allocation functions: 169,594,237  (323,798/s)
temporary memory allocations:   25,467,091   (48,623/s)
peak heap memory consumption:   30.81M
peak RSS:                      300.63M  (incl. heaptrack overhead)
```

**A 30.81 MB live heap behind 169 million allocations.** This is not a leak — it is
temporary-allocation *churn* at a rate that fragments glibc's arena faster than it can be
reused, so RSS climbs while the heap itself stays small. Nothing is retained; the pages
simply never go back.

79% of those calls come from two merged stacks, both the FemtoVG render path:

```
Vec<femtovg::renderer::Vertex>::from_iter
femtovg::path::cache::PathCache::expand_fill
femtovg::Canvas<OpenGl>::fill_path
GLItemRenderer::draw_border_rectangle
i_slint_core::items::BasicBorderRectangle::render
  ← InnerSpectrumBars_root / InnerComponent_spectrumbars / InnerVisualizerStrip
```

The chain: `border-radius` routes an element through `draw_border_rectangle` → `fill_path`,
which tessellates into a **freshly heap-allocated `Vec<Vertex>` per element per frame**. The
bars Timer runs at **16 ms** (`visualizer-strip.slint:55`, per-style — the waveform already
uses 33 ms), and the repaint it forces is *full-window*, so every rounded element in the view
re-tessellates 60×/s, not just the 64 bands.

`spectrum-bars.slint` is already tuned as far as it goes — the outer per-band `Rectangle`
binds no `background` (so it stays `Empty`, per the pitfall in `slint-pitfalls.md`) and each
bar is childless. The remaining cost is inherent to rounded rectangles under FemtoVG.

**This is what the removed periodic `malloc_trim` was for.** The old `Cargo.toml` comment
described it exactly — "the Slint render path allocates short-lived buffers per repaint while
a track plays; glibc's arena retains those pages … a slow RSS climb that plateaus on pause."
`heap_trim.rs:9-12` records why it was dropped: *"measurement showed only the first call
returned meaningful pages."* **That measurement was taken over too short a window** — the
climb needs ~15 min to separate from noise, and the runs above are the evidence.

### Root cause round 2 — it is a Wayland event leak, not allocator churn

A 60-minute run (Now Playing open, no skipping, then closed onto Tracks for 3 min) settles it.

**The growth is unbounded and linear.** `RssAnon` per 10-minute block: +3.5, +6.0, +5.0, +5.4,
+3.4, +5.4 MiB — slope holds ~0.43–0.5 MiB/min from minute 10 through minute 60, 39.2 → 68.0
MiB over the hour. No plateau, so it is not the artwork LRU saturating (cap 8, ~4.4 MiB total).

**It is not reclaimable, and `malloc_trim` is the wrong tool.** A periodic trim gated on the
visualizer was tried and produced **no sawtooth whatsoever** — not one drop at a 60 s boundary
in 15 minutes. Closing the view after the full hour returned **2.6 MiB**, less than the
artwork LRU's own live contents; the other ~26 MiB stayed put. Reverted.

**heaptrack names the producer.** Of 17.76 MiB surviving an 8.7-minute traced session, ~13 MiB
(73%) is Wayland:

| leaked | calls | site |
|---|---|---|
| 6.50M | 457,533 | `wl_closure_init` |
| 4.11M | 120,303 | `wl_display_read_events` → `InnerReadEventsGuard::read_non_dispatch` |
| 2.39M | 96,280 | same |

`wl_closure_init` allocates one closure per incoming Wayland event, freed after dispatch.
Half a million alive at exit means events are **read but not dispatched** — `read_non_dispatch`
is the frame name. That fits every observation: Now Playing repaints at 30–60 fps → a flood of
frame-callback events → linear accrual; view closed → ~2 fps → nearly flat; and they are *live*
allocations, which is exactly why no amount of trimming reached them.

The stack runs `calloop_wayland_source::WaylandSource::before_handle_events` → winit's event
loop → `slint::run_event_loop_until_quit`, i.e. **below this codebase** — winit 0.30's Wayland
event source (the vendored fork at `winit/`), via `calloop-wayland-source 0.3.0` /
`wayland-backend 0.3.15`. The second `calloop-wayland-source 0.4.1` in the lockfile is
`smithay-clipboard` pulling a newer SCTK and is *not* implicated; there is one shared
`wayland-backend`.

*Caveat:* `main()` ends in `process::exit(0)`, so the Wayland connection is never torn down and
some "leaked" bytes are merely that. The RSS curve is the evidence; heaptrack only named the
allocator.

**Open question — does it reproduce off Wayland?** `WINIT_UNIX_BACKEND` was removed in winit
0.29, so the way to force X11/XWayland is to unset `WAYLAND_DISPLAY`:

```bash
env -u WAYLAND_DISPLAY RUST_LOG=info MELODIA_RSS_SAMPLE=1 \
  target/debug/Melodia 2>&1 | grep MEM > /tmp/mem-x11.log
```

20 minutes with Now Playing open is enough — the slope is stable by minute 10. A collapse to
E-like numbers confirms the Wayland path and makes this an upstream bug to report against
winit; an unchanged slope means the repaint volume matters more than the backend and the
answer is to cut repaints further.

### Fix applied (render churn only)

1. **Cut the churn at source.** `visualizer-strip.slint`'s Timer is now 33 ms for *every*
   style, not 16 ms for bars — halving the repaint rate halves the tessellation churn. The
   per-style branch collapsed with it, since the waveform already wanted 33 ms.
2. ~~Periodic `malloc_trim`, gated on the visualizer.~~ **Tried and reverted** — measured as
   doing nothing (see above). `heap_trim.rs` is back to one-shot, and its module doc now
   carries the evidence and the reason trimming cannot work on this bug, so it doesn't get
   re-added a fourth time.

Measured effect of the 33 ms change alone, 15-minute runs, Now Playing open:

| run | `RssAnon` slope |
|---|---|
| D — before | +0.87 MiB/min |
| F — after | +0.64 MiB/min |
| F2 — after | +0.54 MiB/min |
| 60-min run, back half | +0.43 MiB/min |

Roughly a third off, and the visual difference was judged negligible. It does not fix the
bug — halving the repaint rate halves the frame-callback events, which is why it helps at
all, and is consistent with the Wayland diagnosis.

Everything below this line is the original hypothesis, preserved for its still-valid
source research.

## Context

RSS grows roughly linearly with the number of **distinct tracks played**, then plateaus:
debug build sits at ~155 MB, and after a 25-track playlist it reaches ~400 MB and stays
there. One track on repeat does not grow. This is not a Slint cache problem — Slint's
image cache is a 5 MiB LRU and Melodia's covers bypass it entirely (`SharedPixelBuffer`
→ `ImageCacheKey::Invalid`), and the text cache has been item-scoped since Slint 1.15.
It is ours, and it has two halves:

**Half 1 — we allocate far more per track than we need.** Every cover is decoded at full
source resolution before being downscaled: `cover_thumbs`, `now_playing_artwork` and
`detail_artwork` all reach `media::image_decode::decode_capped`, whose only bound is
`MAX_SOURCE_DIM = 8192` — a ceiling against a forged header, not against a real 1280×720
sleeve. Each track change does this **twice**: once on the Slint UI thread for the 72 px row
thumb (`bridge`'s `get_or_load_opt`), once on a blocking worker for the 384 px now-playing
pair (`now_playing/track_change`'s `get_or_decode`). Sampling this library's own artwork
cache (218 files, 207 JPEG):

```
59 × 480x360    46 × 1280x720    33 × 500x500    21 × 300x300    1 × 6016x3384
```

A 1280×720 cover decodes to ~3.7 MB; two of those per distinct track ≈ **7.4 MB/track**,
which matches the observed rate. The single 6016×3384 file decodes to ~81 MB.

**Half 2 — glibc never gives it back.** `main.rs`'s `mallopt` pins `M_ARENA_MAX=2` but leaves
`M_MMAP_THRESHOLD` unset, so glibc's *dynamic* mmap threshold is live: each time an mmap'd
block is freed the threshold is raised to that block's size (ceiling 32 MiB), and the trim
threshold to twice that. After the first few covers the threshold sits above every
subsequent decode, so those allocations come from the arena free list instead of mmap and
freeing them returns nothing to the OS. The 6016×3384 outlier pins the threshold at its
32 MiB ceiling for the rest of the process. `malloc_trim(0)` runs **once, 5 s after
startup** (`tasks::heap_trim::spawn`) and on view-close paths — never on any playback path.

Outcome wanted: flat RSS across a long listening session, and no full-resolution decode on
the playback path at all.

## Phase 1 — Prove the diagnosis before changing anything ✅ done (2026-07-28)

Outcome in the banner at the top: the allocator half is real but small, the per-track
premise is not confirmed, and `RssFile` carries a majority of the short-run growth.
**Add to the protocol below: open Now Playing (`F`) before skipping.**

No rebuild, no code change. Uses the existing sampler (`tasks/rss_sampler.rs`, gated on
`MELODIA_RSS_SAMPLE`).

Run A (baseline) and run B (allocator hypothesis applied via glibc tunable — setting
`mmap_threshold` disables the dynamic ratchet, exactly what Phase 2 will do in code):

```bash
RUST_LOG=info MELODIA_RSS_SAMPLE=1 target/debug/Melodia 2>&1 | grep MEM | tee /tmp/mem-a.log
RUST_LOG=info MELODIA_RSS_SAMPLE=1 GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072 \
  target/debug/Melodia 2>&1 | grep MEM | tee /tmp/mem-b.log
```

In each run: open the Synthwave playlist and press `N` ~25 times, pausing ~2 s per track so
each artwork decode completes. Skipping reproduces it without waiting for playback — the
trigger is a *distinct artwork path*, not elapsed time.

Read off the first and last `VmRSS` / `RssAnon` / `RssFile` of each log.

- Growth in **RssAnon**, and run B stays flat → diagnosis confirmed, proceed as written.
- Growth largely in **RssFile** → Mesa GPU-side, not the heap; Phase 4 becomes the primary
  fix and Phases 2–3 are secondary. Re-scope before continuing.

## Phase 2 — Stop glibc retaining the freed decode buffers ⚠ half done (2026-07-28)

**Done:** the `mallopt` pins are in `src/main.rs` — `M_MMAP_THRESHOLD` (`-3`) and
`M_TRIM_THRESHOLD` (`-1`), both at `128 * 1024`. Note the values are glibc's *own*
initial ones, not new tuning: the ratchet is what's being removed. Run C above is the
verification. Costs roughly 5× the minor page faults for no measurable user-time change.
The stale `Cargo.toml` comment is corrected (it pointed at `src/player/handlers.rs`,
which has no `malloc_trim` call).

**Held:** the artwork-decode `heap_trim::trim()` below. It targets per-track churn, and
per-track churn is not the confirmed symptom; if the real cause turns out to be steady
allocation under playback, a *periodic* trim is the right shape and adding both would be
redundant. Decide after the multi-hour capture.

The `M_TRIM_THRESHOLD` value below says "start `256 * 1024`" — 128 KiB shipped instead,
so the compiled behaviour matches what run B measured rather than deviating from it.

`src/main.rs`, in the existing `mallopt` block (~line 85-103): add two `mallopt` calls
beside the `M_ARENA_MAX` one, same `cfg(all(target_os = "linux", target_env = "gnu"))` gate
and the same raw-constant-with-comment style already used there.

- `M_MMAP_THRESHOLD` (`-3`) → `128 * 1024`. Pinning it disables glibc's dynamic ratchet,
  so every cover decode is mmap'd and `munmap`'d on free regardless of what came before.
- `M_TRIM_THRESHOLD` (`-1`) → a fixed value (start `256 * 1024`); the dynamic ratchet moves
  this one too, and pinning keeps the automatic top-of-heap trim alive.

Then give the playback path a trim it currently lacks: call `tasks::heap_trim::trim()` off
the UI thread after a now-playing artwork decode evicts from its LRU — the call sites in
`src/ui/now_playing/up_next.rs:162` and `src/ui/mini_player.rs:56` are the pattern to copy
(`runtime.spawn_blocking(crate::tasks::heap_trim::trim)`).

While here, correct two comments that are already wrong (they claim periodic trimming from
the playback monitor, which was removed): `Cargo.toml:317-324` on the `libc` dep, and the
`heap_trim.rs:9-12` module doc once its behaviour changes.

## Phase 3 — Pre-scaled artwork on disk (the real fix)

Goal: the playback path never decodes a large image. Artwork files are already content-hash
named (`{hash16}.{ext}`, `media/artwork.rs:152-154` and `:204-206`), so a derivative keyed
by that hash needs no DB change and no migration — `tracks.artwork_path` keeps pointing at
the original.

New module `src/media/artwork_thumb.rs`:

- `const DERIVATIVE_MAX_DIM: u32 = 512` — covers every consumer, the largest being the
  448 px entity-grid tier (`src/ui/albums/state.rs:109`).
- `derivative_path(artwork_path) -> PathBuf` — `<artwork_dir>/thumbs/<file_stem>.jpg`.
  Keyed on the stem (the hash) so source extension is irrelevant, and a `thumbs/` subdir
  stays trivially disposable.
- `resolve_decode_path(artwork_path) -> PathBuf` — the derivative when it exists, else the
  original.
- `write_derivative(&DynamicImage, dest)` — downscale to fit `DERIVATIVE_MAX_DIM` and encode
  JPEG (~q85), written via tempfile-then-persist so a torn write can't be read as valid.
  Reuse the `NamedTempFile::new_in` + `persist` shape from
  `media/artwork.rs::compose_artwork:348-378`.

**Generation is lazy, not scan-time.** Each of the three decode sites resolves through
`resolve_decode_path`, and after decoding an *original* whose long edge exceeds
`DERIVATIVE_MAX_DIM`, hands the `DynamicImage` off to write the derivative (off-thread; these
call sites already run under `spawn_blocking` except the row tier). Covers already at or
below 512 px never generate one and keep decoding the original, which is cheap by
definition — so "no derivative" is never ambiguous.

This deliberately avoids the scan-pipeline change and the backfill task that a scan-time
design would need. It is the same total work, moved to first-view instead of scan-time, and
it self-heals for the already-scanned library with no migration. The one trade-off: the
*first* play of a track still pays one full-resolution decode, once ever.

Call sites to route through the resolver:

- `src/media/cover_thumbs.rs` (`decode_thumb_buffer`)
- `src/ui/now_playing_artwork.rs` (`decode_artwork`)
- `src/ui/detail_artwork.rs` (its `decode_artwork` sibling)

Also lower the bound from `8192` to `4096` on those three: 8192 permits a single 268 MB
decode, and nothing above 4096 is useful for a ≤512 px derivative. **Pass `4096` at the three
call sites rather than editing `MAX_SOURCE_DIM` itself** — the constant has three other
readers the derivative work has no business tightening:

- `src/ui/mosaic_blur.rs` (`decode_tile`) and `src/ui/callbacks/tags.rs`
  (`decode_cover_preview`) — both `.ok()?`, so a tightened bound degrades silently.
- `src/media/tag_writer.rs` (`cover_picture_from_path`, via `capped_limits`) — and this one
  is **user-facing**: it maps a decode failure to `AppError::metadata`, so at 4096 a user
  picking a 5000 px cover in the tag editor gets a hard "Failed to decode cover" instead of
  an embed. A hand-picked cover is the one artwork the user chose deliberately; it should not
  inherit a cap that exists to keep the *playback* path cheap.

Material You already passes its own `2048` (`services/material_you.rs`) and is unaffected
either way.

Leave `compose_artwork` reading originals: it is a scan/playlist-edit path, not playback,
and its 600 px canvas is above the derivative size.

## Phase 4 — Release the now-playing image slots

`melodia-ui/ui/globals.slint:43-45` states outright that `np-cover-{a,b}` / `blur-img-{a,b}` are never
cleared, and `write_crossfade_slot` (`src/ui/now_playing/mod.rs:336-365`) only sets
`has_image = false` on the `None` branch. That pins two covers + two blurs (~1.06 MiB CPU
plus their FemtoVG textures) after the view closes, surviving `NowPlayingArtwork::clear()`
(documented at `src/ui/now_playing/up_next.rs:153-157`).

Mirror the existing detail-view pattern: reset all four to `Image::default()` on
now-playing close, alongside the `clear()` + trim already there in `up_next.rs:159-163`.
`release_detail_hero_images!` (`src/ui/callbacks/macros.rs:146-158`) is the model.

Fixed-ceiling, not growth — so this is a correctness cleanup unless Phase 1 shows RssFile
carrying the climb, in which case it moves to the front.

## Verification

1. `cargo clippy --all-targets -- -D warnings`, then `cargo test`.
2. Re-run the **Phase 1 protocol** on the debug build (run A only — the fix is now in the
   binary). Compare against `/tmp/mem-a.log`: `RssAnon` should be flat across the 25 skips
   instead of climbing ~7 MB per track.
3. Confirm the derivative cache populates and is used: `ls ~/.local/share/Melodia/artwork/thumbs`
   should fill only with the oversized covers (the 1280×720s and the 6016×3384), not all 218.
   Play the same tracks a second time and confirm no new full-resolution decode (RSS
   unchanged, no new files).
4. Visual check that artwork quality is unchanged at every tier: rows, entity grids (448 px
   is the tightest consumer), album/artist detail heroes, now-playing cover + blur backdrop,
   mini-player.
5. Release gate: `cargo build --release` then
   `/usr/bin/time -v target/release/Melodia` — play a long session and confirm peak RSS
   stays near the ~110 MB baseline (ceiling is 200 MB).

## Sources

Everything below was read directly during investigation, not recalled.

### Melodia source

`src/main.rs:85-103` (the lone `mallopt`) · `src/tasks/heap_trim.rs` (whole file) ·
`src/tasks/rss_sampler.rs` · `src/media/artwork.rs` (whole file) ·
`src/media/{cover_thumbs,image_decode}.rs` · `src/ui/now_playing_artwork.rs` ·
`src/ui/detail_artwork.rs` · `src/ui/bridge.rs` · `src/ui/now_playing/{mod,track_change,up_next}.rs` ·
`melodia-ui/ui/globals.slint:43-45` · `src/ui/callbacks/macros.rs:146-158` ·
`src/tasks/material_you.rs` · `src/player/{state,handlers,actions,rodio_backend,decks,equalizer,replaygain,visualizer,spectrum}.rs` ·
`src/services/scrobble/queue.rs` · `Cargo.toml` · `git` branch topology and merge history.

### Measured on this machine

`~/.local/share/Melodia/artwork` — 218 cached covers, 207 JPEG / 13 PNG, dimension
histogram quoted in Context above. This is the evidence the ~7.4 MB/track figure rests on.

### Slint upstream — whether any of this is fixable from their side

Conclusion: **no cache-control API exists in Slint 1.17.1 or on master**, and none is
planned. Code search for `clear_image_cache` / `set_image_cache*` / `image_cache_size` /
`clear_caches` across `slint-ui/slint` returns zero hits. No milestone or roadmap item.

- [#12379](https://github.com/slint-ui/slint/issues/12379) — image cache under-weighted parsed
  SVGs (512 B assumed), so the 5 MiB cap held 10,000+ trees. A user asked outright whether
  caching can be disabled; ogoffart's answer was to *bypass* it with a `SharedPixelBuffer`,
  which is already what Melodia does. Fixed by [PR #12386](https://github.com/slint-ui/slint/pull/12386)
  (merged 2026-07-15).
- [PR #11792](https://github.com/slint-ui/slint/pull/11792) — embedded fonts re-registered per
  component instantiation, ~4 MB per window per font (merged 2026-05-19).
- [#12545](https://github.com/slint-ui/slint/issues/12545) / [PR #12548](https://github.com/slint-ui/slint/pull/12548)
  — FemtoVG box-shadow cache was frame-scoped, reblurring stable shadows every redraw
  (merged 2026-07-20, i.e. after 1.17.1 — lands in 1.18).
- [#10837](https://github.com/slint-ui/slint/issues/10837) — **open**, `need triaging`.
  Baseline memory regression from the 1.14 parley/fontique text rework, reported against
  *Zeedle*, another Rust music player: ~10 MB on 1.13.1 vs ~50 MB on 1.15.1 on Windows.
  Baseline, not growth-over-time.
- [#2714](https://github.com/slint-ui/slint/issues/2714) — **open** since 2023. No texture
  sharing between `slint::Image`s built from the same `SharedPixelBuffer`; the same cover in
  a row and a grid uploads two GPU textures. Directly relevant to us.
- [#12481](https://github.com/slint-ui/slint/issues/12481) — **open**, untriaged. A `LazyImage`
  request with `auto-unload` / LRU. The closest thing to a user-facing caching knob anyone
  has asked for.
- [#11266](https://github.com/slint-ui/slint/issues/11266), [#3029](https://github.com/slint-ui/slint/issues/3029),
  [discussion #2750](https://github.com/slint-ui/slint/discussions/2750) — the 2023-era
  femtovg *text* cache growth, keyed by string content. Superseded: 1.15+ keys the text
  layout cache by `ItemRc` and frees it on component destroy.
- [CHANGELOG](https://raw.githubusercontent.com/slint-ui/slint/master/CHANGELOG.md) — latest
  release 1.17.1 (2026-07-07).

### Slint / FemtoVG source read locally

Read against **`slint 1.16.1` + `femtovg 0.23.2`** — what `Cargo.lock` actually pins — and
cross-checked against 1.17.0 (also on disk) to confirm nothing relevant moved.
**No Slint upgrade is needed for any of this work**, see the note below.

- `i-slint-core-1.16.1/graphics/image/cache.rs:51` — the image cache is a 5 MiB `CLruCache`,
  thread-local, weighted by decoded pixel bytes.
- `i-slint-core-1.16.1/graphics/image.rs:351` — `ImageCacheKey::Invalid` → `None`, i.e.
  programmatically-created images are never cached. This is Melodia's path.
- `i-slint-renderer-femtovg-1.16.1/images.rs:283` — texture cache is drained **every frame**,
  evicting anything with `Rc::strong_count == 1`.
- `i-slint-renderer-femtovg-1.16.1/itemrenderer.rs` — the uncached fallback; textures for
  `SharedPixelBuffer` images live per-item in the graphics cache, not in the texture cache.
- `i-slint-core-1.16.1/textlayout/sharedparley.rs:27` + `item_rendering.rs:125` — the
  `TextLayoutCache` is an `ItemCache<Vec<TextParagraph>>` keyed by (component ptr, item
  index), released on `component_destroyed`.
- `femtovg-0.23.2/src/text.rs:437` — glyph atlas pages, deduped per (font, size, style).
- `image-0.25.10/src/codecs/jpeg/decoder.rs` — **no DCT-scaling API**. image 0.25 is backed
  by zune-jpeg and the old `JpegDecoder::scale()` is gone, which is why Phase 3 pre-scales
  to disk rather than decoding JPEGs at reduced resolution in-place.

#### Does the 1.16.1 → 1.17.x upgrade matter here? No.

The four load-bearing facts are unchanged between the two releases — the cache cap is on the
same line (`:51`), `drain` is on the same line (`:283`), the `Invalid` → `None` guard moved
by exactly one line (`:351` → `:352`), and `component_destroyed` is identical at `:125`.
`sharedparley.rs` was reorganised (`:27` → `:58`) but the type and its lifetime semantics are
the same. The files differ, none of the differences touch this bug.

Nor would upgrading fix anything for us: 1.17's cache work was the SVG cache-weighting fix
([#12386](https://github.com/slint-ui/slint/pull/12386)), and we load raster covers, not SVGs.
The FemtoVG box-shadow cache fix ([PR #12548](https://github.com/slint-ui/slint/pull/12548))
landed *after* 1.17.1 and ships in 1.18 — worth having eventually for drop-shadow redraw cost,
but it is a CPU/redraw fix, not a memory one. Treat the Slint bump as a separate,
independently-scheduled decision; nothing in this plan blocks on it or is invalidated by it.

### glibc

`mallopt(3)`: the dynamic mmap threshold, its 32 MiB `DEFAULT_MMAP_THRESHOLD_MAX` ceiling on
64-bit, and the fact that setting `M_MMAP_THRESHOLD` / `M_TRIM_THRESHOLD` explicitly
disables the dynamic adjustment. `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=` is the
env-var equivalent used for the Phase 1 A/B.
