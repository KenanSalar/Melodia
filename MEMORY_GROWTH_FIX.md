# Fix per-track RSS growth (artwork decode churn + glibc arena retention)

Working doc for `fix/general-improvements`. Delete when the work ships.

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

## Phase 0 — Branch

Repo vocabulary is exactly three prefixes — `feat/`, `fix/`, `ci/` (verified across every
local, remote and historically-merged branch). No `perf/`, `chore/`, `refactor/` or
`improvement/` has ever been used, so introducing one here would be the odd one out.

Use **`fix/general-improvements`**. The headline deliverable is a memory-growth bug, and
there is precedent for a broad mixed branch under this prefix
(`fix/review-audio-and-error-handling`), just as `feat/replaygain-and-improvements` is the
precedent on the feature side. Behavioural adjustments riding along don't change that the
branch exists to fix a bug.

Base it on the **current `feat/visualization` HEAD**, not `dev`: that branch is strictly
ahead of `dev` (27 commits, zero divergence) and it has been editing
`ui/now_playing/` and `ui/globals.slint`, which Phase 4 also touches. Branching off it keeps
the work sequential and conflict-free, and once visualization merges the new branch's diff
against `dev` is just its own commits. If visualization is *not* going to merge first,
branch off `dev` instead and accept the merge later.

```bash
git switch -c fix/general-improvements
```

## Phase 1 — Prove the diagnosis before changing anything

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

## Phase 2 — Stop glibc retaining the freed decode buffers

`src/main.rs`, in the existing `mallopt` block (~line 85-103): add two `mallopt` calls
beside the `M_ARENA_MAX` one, same `cfg(all(target_os = "linux", target_env = "gnu"))` gate
and the same raw-constant-with-comment style already used there.

- `M_MMAP_THRESHOLD` (`-3`) → `128 * 1024`. Pinning it disables glibc's dynamic ratchet,
  so every cover decode is mmap'd and `munmap`'d on free regardless of what came before.
- `M_TRIM_THRESHOLD` (`-1`) → a fixed value (start `256 * 1024`); the dynamic ratchet moves
  this one too, and pinning keeps the automatic top-of-heap trim alive.

Then give the playback path a trim it currently lacks: call `tasks::heap_trim::trim()` off
the UI thread after a now-playing artwork decode evicts from its LRU — the call sites in
`ui/now_playing/up_next.rs:162` and `ui/mini_player.rs:56` are the pattern to copy
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
  448 px entity-grid tier (`ui/albums/state.rs:109`).
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

`ui/globals.slint:43-45` states outright that `np-cover-{a,b}` / `blur-img-{a,b}` are never
cleared, and `write_crossfade_slot` (`ui/now_playing/mod.rs:336-365`) only sets
`has_image = false` on the `None` branch. That pins two covers + two blurs (~1.06 MiB CPU
plus their FemtoVG textures) after the view closes, surviving `NowPlayingArtwork::clear()`
(documented at `ui/now_playing/up_next.rs:153-157`).

Mirror the existing detail-view pattern: reset all four to `Image::default()` on
now-playing close, alongside the `clear()` + trim already there in `up_next.rs:159-163`.
`release_detail_hero_images!` (`ui/callbacks/macros.rs:146-158`) is the model.

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
`ui/globals.slint:43-45` · `src/ui/callbacks/macros.rs:146-158` ·
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
