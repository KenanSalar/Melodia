# The Artwork Store

Working doc. Delete when the feature ships.

Status: **proposed** · Created: 2026-08-16 · Re-measured and re-scoped: 2026-08-18

> Every number below was **measured on 2026-08-18** against the developer's own
> `~/.local/share/Melodia/` (512 tracks, 402 with artwork), not estimated. Code
> references are against this tree at the same date, and against `image-0.25.10` in the
> registry.

---

## The observed state

`~/.local/share/Melodia/artwork` holds **227 files / 39.0 MB**. Of those, **36 files —
28.5 MB, 73% of the store — are referenced by nothing**: no track, no album, no artist, no
playlist. Nothing in the tree ever deletes them. There is no prune, no sweep, no orphan
collection; `crash_report::prune` and `flexi_logger` clean `logs/`, and the artwork store
has no equivalent.

| | files | size | above 512 px |
|---|---|---|---|
| **orphaned** | 36 | **28.5 MB** | 35 files / 28.5 MB |
| live | 191 | 10.5 MB | 48 files / 6.3 MB |

The orphans are where the weight is, which decides the phase ordering below.

The store keeps **source bytes verbatim** — no re-encode, no downscale — so its shape is
whatever the user's tags happened to carry. Long edge across all 227 files:

```
n=227   min=120   p25=480   median=500   p75=1280   p90=1280   p99=2154   max=6016
```

`~/.local/share/Melodia/artists` is the same store under a different name — identical
content-addressed scheme, identical missing prune — at 80 files / 1.3 MB, of which **39 are
orphaned**. Deezer images are fetched once per artist and are uniformly 250 px, so that
directory has no size problem at all; it has the same leak, at half its contents.

---

## What already works, and must not be "improved"

**The store is content-addressed and the dedup is exact.** `artwork.rs:114` hashes the
image bytes with BLAKE3 and takes the first 8 as the filename; the writer guards on
`!file_path.exists()`. So identical artwork bytes produce one file, one DB path string, and
therefore one entry in every path-keyed cache downstream. Measured: **227 files, 227 unique
contents, zero duplicates**; 189 distinct covers serving 402 tracks.

Nothing here changes that. A reader arriving at this plan should not read "artwork dedup"
as work to be done — it is done, and the truncated 64-bit hash is fine (birthday bound
≈ 3×10⁻¹⁰ at 100 000 covers).

---

## Seven findings that decide the shape

1. **A sweep, not a refcount.** Artwork is shared, so a per-track delete must *not* unlink
   the file — 11 of 12 rows may still point at it. A refcount would have to be exactly
   right across scan ingest, orphan cleanup, both watcher delete/rename paths, tag edits
   that replace a cover, and composite regeneration; undercount and live art disappears
   silently, overcount and it leaks anyway. A reference sweep cannot undercount because it
   never counts. It also cleans the **existing** 28.5 MB, which a refcount can't — there is
   nothing to decrement — so a refcount would need a one-time sweep regardless, at which
   point it is redundant machinery.

2. **The reference set is four columns, and the fourth is the one that bites.** Alongside
   `tracks.artwork_path`, `albums.artwork_path` and `artists.image_path` there is
   **`playlists.thumbnail_path`**, which points into `artwork/` under the same hash-named
   scheme. Two of the four playlist thumbnails in the reference library are reachable
   through *no other column*:

   ```
   33fb807d1f1b7cbb.jpg  600x600  0 tracks   ← a three-column sweep deletes this
   4cccaf4d4b4cea11.jpg  600x600  0 tracks   ← and this
   2156d927a2d5749f.png  1107x1106  also on 8 tracks   (survives by coincidence)
   bd64b8b331d984ac.png             also on 3 tracks   (survives by coincidence)
   ```

   Two survive only because they happen to alias a track's cover, which is not a property
   to rely on. This is finding 1's own failure mode landing on the query that implements
   it, so the sweep owes a test that names the columns rather than a reviewer who
   remembers them.

3. **The store holds what the tag carried, and every tier decodes it in full.** The largest
   consumer is `GRID_COVER_SIZE_HIDPI` at 448 px; `COVER_SIZE` is 384 and `compose_artwork`
   composes at 600. Nothing reads more. Meanwhile `decode_capped` decodes at full
   resolution and resizes afterwards — no JPEG DCT scale-on-decode — and the tiers share no
   decodes with each other, so row, grid, strip, mosaic and `ArtworkCache` each decode the
   same source independently. The 6016×3384 file is a **~58 MiB RGB8 buffer** per decode, to
   draw at 448 px. **The store's cap is therefore the ceiling on every transient decode
   buffer in the app**, which is the memory argument for capping it at all.

4. **The distribution has two clusters, and the cap belongs just above the lower one.**
   Long edges, by frequency:

   ```
     128  ###################  (19)
     480  ###########################################################  (59)
     500  ####################################  (36)
     512  #  (1)
     600  ##########  (10)
     640  ########  (8)
    1280  #######################################################  (55)
    6016  #  (1)
   ```

   **96 files — 42% of the store — sit at 480–512**, and 55 more at exactly 1280. A cap
   placed *below* a cluster does maximum work for minimum return, because the pixel
   reduction is too small to pay for a re-encode: measured, with the never-inflate rule of
   Phase 4 arbitrating,

   | cap | tried | kept | **discarded (grew)** | live store | serves the 448 grid tier |
   |---|---|---|---|---|---|
   | 1280 | 4 | 4 | 0 | 10.5 → **10.5 MB** | yes |
   | **512** | 83 | 77 | **6** | 10.5 → **6.0 MB** | **yes** |
   | 448 | 179 | 116 | 63 | 10.5 → 4.9 MB | exactly, with no margin |
   | 384 | 180 | 118 | 62 | 10.5 → 4.1 MB | **no** |

   1280 sits above both clusters and so does nothing: it re-encodes four files, all of them
   orphans the sweep already removes, and leaves the live store untouched. 448 and 384 slice
   the 480–512 cluster and do near-identical work — 179 vs 180 attempts, 63 vs 62 of them
   thrown away — for 0.8 MB of difference, so if the cluster is going to be cut there is no
   reason to stop at 448.

   **512 is the answer, and the invariant is what picks it, not the histogram.** The store
   must hold at least what the largest tier decodes; that floor is 448, and 512 is the
   smallest round value above it. That it also lands exactly past the lower cluster is what
   makes it cheap.

5. **The caches upscale a fifth of the store, and the store does not.** `cover_thumbs.rs:273`
   calls `thumbnail_exact(thumb_size, thumb_size)`, which produces exactly that regardless of
   the source; `artwork_cache.rs:129`'s `thumbnail(COVER_SIZE, COVER_SIZE)` routes through
   `resize_dimensions` (`image-0.25.10/src/math/utils.rs:56`), whose `ratio = min(nw/w, nh/h)`
   has **no clamp at 1.0**. Measured against the tiers:

   | tier | files below it |
   |---|---|
   | blur 192 px | 35 (15%) |
   | grid 256 px | 41 (18%) |
   | cover 384 px | 47 (20%) |
   | grid HiDPI 448 px | **48 (21%)** |

   19 files are 128 px. Each is currently held as a 448×448 / 588 KB buffer carrying 128×128
   of information — a 12× multiplier, box-filter upscaled.

6. **Three of the four writers are not atomic, and the failure is permanent.**
   `compose_artwork` (`artwork.rs:362`) stages through `NamedTempFile` and `persist`;
   `cache_image_file` (`:158`), `extract_and_cache_artwork` (`:204`) and `deezer.rs:258`
   all call `fs::write`. A crash, a full disk or a force-exit mid-write leaves a truncated
   file — and because the name is content-addressed and every writer guards on `exists()`,
   **it is never rewritten**. `CoverThumbs` then caches the failed decode as `None`, so the
   cover stops displaying and stays that way until someone deletes the file by hand. The
   sweep won't help: it is still referenced. This contradicts the rule `database/backup.rs`
   states outright — staged under `.tmp` and renamed, so the final name existing means the
   file is complete — and it contradicts its own sibling fifty lines below it.

7. **Composites churn; the sweep is periodic, not one-time.** `compose_artwork` hashes its
   composed output, so **every change to a playlist's top-4 writes a new file and orphans
   the old one**. A playlist edited weekly generates a new composite weekly, and `artists/`
   leaks the same way at 39 of 80 files. This is a growth *rate*, not a backlog, and it is
   the reason the sweep runs on every scan rather than once at upgrade.

**And one consequence worth stating before it surprises someone in Phase 4:** normalizing
on ingest only affects newly-scanned files. `track_is_current` skips unchanged files, so
artwork is never re-derived for a track already in the library — the existing store stays as
it is without a deliberate pass. Hence Phase 5.

---

## Structure

No new module. Both halves belong to the file that already owns the store.

```
src/media/artwork.rs        the writers (atomicity), normalization, the filename predicate
src/media/artwork/sweep.rs  NEW: the reference sweep, one fn over (dir, referenced set)
src/database/queries/artwork.rs  NEW: the four-column reference query
src/media/cover_thumbs.rs   the tier decode (Phase 2 clamp)
src/ui/artwork_cache.rs     the cover + blur decode (Phase 2 clamp)
melodia-ui/ui/views/now-playing-view.slint   the card ceiling (Phase 4)
```

Ownership rules:

- **The sweep is one function taking a directory and a referenced set.** `artwork/` and
  `artists/` differ only in those two arguments; a second copy specialized to artists is
  precisely the drift to avoid — they already share a filename scheme, a dedup guard and a
  missing prune because they were written twice.
- **The filename predicate lives beside the writers, and the sweep calls it.** The rule is
  "delete only names this module writes"; that is only checkable if the writing and the
  parsing sit in one file. Precedent: `crash_report::timestamp_of` and
  `database/backup.rs`'s retention, both of which refuse to delete a name they can't parse
  back into their own scheme.
- **The store's directory is shared with `compose_artwork`'s playlist composites**, so the
  predicate is "16 hex chars + a known extension", not "written by the scanner" — and, per
  finding 2, the reference set is what actually keeps a composite alive. Both halves have to
  be right; either one alone blanks every playlist mosaic in the app.
- **Normalization happens at the writer, not at the caller.** All three writers funnel into
  one `store_image(bytes, dir) -> Option<String>`; a caller that decides its own cap is how
  the store ends up with two size policies.
- **`COMPOSITE_SIZE` is derived from `STORE_MAX_DIM`, not spelled beside it.** The composite
  is written *into* the store, so a composite larger than the cap is encoded once and
  immediately re-encoded — a second generation loss on the one path finding 7 says is hot.
- **The Phase 2 clamp belongs at each decode, not in a shared helper.** The three call sites
  want different things — square, aspect-preserved, and deliberately aspect-*distorted* — so
  a single "fit" helper would have to take a mode argument that is really just the three
  call sites written down again.
- **`src/ui/` reaches none of the store work.** The sweep is a scan-pipeline concern and runs
  where the orphan cleanup already runs. The one `src/ui/` and `.slint` edit in this plan is
  the now-playing card ceiling, which is a tier question rather than a store one.

---

## Phases

Phases 1 and 2 are independent correctness/memory fixes that hold regardless of what the
store contains; ship them in either order. Phases 3–5 are the store itself.

**Phase 3 is still the one that pays**, taking 39.0 MB → 10.5 MB on the reference library.
Under the old 1280 cap Phases 4 and 5 were insurance that touched one live file; at 512 they
are load-bearing, taking what the sweep leaves from **10.5 MB → 6.0 MB** and bounding every
decode buffer in the app at 512 px. Both halves now earn their place on this library rather
than on a hypothetical one.

### Phase 1 — Atomic writes · no visible change

1. Give `artwork.rs` one private `write_atomic(dir, filename, bytes)` using
   `NamedTempFile::new_in` + `persist`, lifted from `compose_artwork`'s existing body.
2. Route `cache_image_file` and `extract_and_cache_artwork` through it. Re-point
   `compose_artwork` at it too, so the shape exists once.
3. Same treatment for `deezer.rs:258` (artist images).
4. Keep the `exists()` fast path — it is the dedup, and it is correct *given* an atomic
   write. What it can't survive is a partial one.

**Exit:** `cargo clippy --all-targets --locked -- -D warnings` clean, `cargo test` clean.

**Note:** this does not repair files already truncated on disk. Nothing can identify them
short of decoding every file, and the honest fix for a user who hits one is Phase 3 plus a
re-scan — the sweep won't remove a referenced file, but a re-scan after the row is gone will.

### Phase 2 — Stop the caches upscaling

Independent of everything else: different files, different mechanism, and it is a memory
win rather than a disk one.

1. **`cover_thumbs.rs:273`** — clamp the square target to the source's own long edge. A
   128 px source yields a 128 px buffer, not a 448 px one.
2. **`artwork_cache.rs:129`** (`thumbnail`, aspect-preserved) — clamp the target box the
   same way; `resize_dimensions` will then compute a ratio ≤ 1.
3. **`artwork_cache.rs:143`** (`thumbnail_exact(BLUR_TARGET, spec.height)`) is the
   one that needs care: its aspect distortion is **deliberate**, squashing a square cover
   into a landscape band. Scale the *target rectangle* down uniformly until it fits the
   source rather than clamping each axis independently — otherwise the amount of distortion
   starts depending on the source's shape, which is a behaviour change rather than a saving.
4. Nothing on the Slint side changes. Every consumer draws these through `image-fit: cover`
   on a GPU texture, so a smaller buffer is simply magnified at draw time — work the GPU was
   doing anyway, and with bilinear filtering rather than the box-filtered upscale currently
   baked into the buffer. Expect it to look the same or slightly better.

**Exit:** clippy + test clean. A unit test that a source smaller than the tier produces a
buffer at the source's size, and that the blur tier keeps its target aspect ratio.

### Phase 3 — The reference sweep

1. `queries::artwork::referenced_filenames(pool) -> HashSet<String>` — the union of
   `tracks.artwork_path`, `albums.artwork_path`, `artists.image_path` and
   **`playlists.thumbnail_path`**, reduced to basenames. Four columns; miss one and the
   sweep deletes live art (finding 2).
2. `artwork::sweep(dir, referenced, grace) -> SweepReport` — list the directory, keep
   anything failing the filename predicate, keep anything in `referenced`, keep anything
   whose mtime is inside `grace`, delete the rest. Returns counts and bytes for the log.
3. **The grace window is the concurrency answer.** A tag edit or a parallel scan worker can
   have written a file whose DB row hasn't committed; without a window the sweep deletes it
   and leaves a dangling reference. An hour is generous and costs one extra scan cycle.
4. Call it from `library/settings/folders.rs`, **after** the transaction carrying
   `prune_orphans` commits (`:433`), for both directories. Log the report through `services::describe`.

**Exit:** 39.0 MB → 10.5 MB on the reference library, on the next scan, and `artists/`
1.3 MB → 0.6 MB. Dangling references stay at 0 — assert it in the same pass, since a sweep
that creates one is worse than the leak it fixed.

### Phase 4 — Normalize on ingest

1. `store_image(bytes, dir)`: decode-validate through `image_decode::capped_limits`, then
   re-encode **only when the source exceeds a bound** — long edge > `STORE_MAX_DIM`, or byte
   length > `STORE_MAX_BYTES`. Below both, write the original bytes untouched.
2. **`STORE_MAX_DIM = 512`**, argued at its definition from finding 4. The store must hold at
   least what the largest tier decodes — `GRID_COVER_SIZE_HIDPI` at 448 — so 448 is a floor
   and 512 is the smallest round value clearing it with room to retune a tier without
   rebuilding the store. It sits just above the dominant real-world source sizes, so the
   normalizer fires on the tail rather than the body of the distribution, and it is
   comfortable for **MPRIS**, the one consumer we don't control — `media_controls/mod.rs:254`
   hands `file://<artwork_path>` to the desktop shell for lock screens and media popups at
   sizes it picks.
3. **`COMPOSITE_SIZE` becomes `STORE_MAX_DIM`** (600 → 512). It is currently the only
   in-tree writer that would exceed its own store, so left at 600 every playlist edit would
   encode a JPEG and immediately re-encode it. 512 still clears both tiers that read a
   composite back — the playlist grid at 448 and the detail hero, which draws
   `Theme.hero-artwork` at 140 logical px through `COVER_SIZE`.
4. **`now-playing-view.slint:25`'s ceiling goes 380px → 384px.** The card is the largest
   artwork the app paints and `COVER_SIZE` was sized to it; leaving the two four pixels apart
   means the tier's own doc comment has to say "~380". They are not otherwise related — one
   is a Slint logical length and the other a decode size in physical pixels, equal only at 1× —
   so align the numbers without writing a comment that claims a derivation.
5. **`STORE_MAX_BYTES = 1 MiB`**, a second and independent trigger: dimensions drive decode
   cost, bytes drive disk, and a file can fail either alone. At a 512 cap it has **no live
   example** — the largest in-cap file on the reference library is 153 KB, seven times under
   it — because ordinary encodings can't reach 1 MiB inside 512 px. It is there for the ones
   that can: 16-bit PNG, uncompressed TIFF. It re-encodes at existing dimensions; the byte
   rule never resizes.
6. **Never write the normalized version unless it is smaller than the source.** This is the
   rule that makes the cap forgiving: without it, a cap chosen slightly too low inflates the
   store (finding 4), and a cheaply-encoded source is *always* at risk of growing under a
   fixed quality. With it, a badly-chosen cap can waste CPU but can never do damage. At 512
   it fires on 6 of the 83 files that exceed the cap, so it is live machinery rather than a
   theoretical guard.
7. **Nothing is ever enlarged.** The bounds are a ceiling, not a target — a 120×120 cover
   stays 120×120 and byte-identical, having failed neither bound and so never being decoded
   at all. On the reference library 144 of 227 files are untouched by both rules.
8. Re-encode to JPEG at quality 90, matching `compose_artwork`.
9. **Hash the stored bytes, not the source bytes.** The filename must describe what is on
   disk or the dedup guard starts lying — two sources that normalize to the same output
   should collapse to one file, and a re-encode not reflected in the name makes `exists()`
   skip a write that should have happened.

### Phase 5 — Renormalize the existing store

Phase 4 only reaches files scanned after it ships; `track_is_current` guarantees the
existing ones are never re-derived.

1. A one-shot background task, dispatched after boot on the blocking pool, gated on a
   `settings.json` marker so it runs once per install.
2. For each **distinct** stored path: if it is within both bounds, do nothing. Otherwise
   re-encode through `store_image`, which yields a new hash and a new filename, then
   `UPDATE` every row pointing at the old path — all four columns from Phase 3, since a
   playlist thumbnail is as re-pointable as a track's cover.
3. The old file is now unreferenced and **the next scan's sweep removes it** — no delete
   logic here at all, which is the point of doing this after Phase 3 rather than before.
4. Not an SQLx migration. It is a data pass over a rebuildable cache, it is slow, and a
   migration failure is fatal at boot (`CLAUDE.md`) — this must not be able to stop the app
   opening.

**Expected on the reference library:** 48 live files exceed 512 px, 42 of them shrink and 6
are spared by the never-inflate rule, taking the swept store **10.5 MB → 6.0 MB**. Log the
counts and the bytes rather than a percentage.

### Phase 6 — Tests

Source walks, matching the tree's existing style:

1. **No `fs::write` on an artwork path** — over `media/artwork.rs` and `media/deezer.rs`.
   The regression is a fifth writer added later, not the three fixed in Phase 1.
2. **The filename predicate rejects what the module doesn't write** — a fixture directory
   with a `README`, a `.tmp`, a hand-named `mycover.jpg` and a real hash-named file; only
   the last is a sweep candidate.
3. **The reference query names all four columns** — a walk, since the failure mode of a
   missing column is deleting live art rather than a compile error, and the column that was
   missed once is the one a reviewer is least likely to miss twice.
4. **A composite referenced only by `playlists.thumbnail_path` survives a sweep** — the
   case finding 2 measured, and the one that blanks every playlist mosaic if it regresses.
5. **The grace window keeps a just-written file**, with the clock injected rather than
   slept on.
6. **No tier upscales** — a source smaller than the tier yields a buffer at the source's
   size, at all three Phase 2 call sites, plus the blur tier keeping its target aspect.
7. **`COMPOSITE_SIZE <= STORE_MAX_DIM`** — a `const` assertion, not a runtime test. The
   failure is silent (a double re-encode on every playlist edit), so it belongs where it
   can't be skipped.
8. **Every tier's decode size fits under `STORE_MAX_DIM`** — the invariant finding 4 picked
   the cap from, asserted against `GRID_COVER_SIZE_HIDPI`, `COVER_SIZE` and
   `row_cover_size`, so a future tier bump fails the build instead of quietly upscaling.
9. Behavioural on `store_image`: a below-bounds source comes back byte-identical; an
   above-bounds one is smaller than its source and its filename matches the hash of what was
   written; and a cheaply-encoded source that would *grow* under re-encode is left alone.

### Phase 7 — Docs and exit

1. `CLAUDE.md` — the `media/` bullet gains the store's invariants: content-addressed,
   atomically written, bounded on ingest at the largest tier's decode size, swept against
   four columns. Two lines; the detail belongs in the module's `//!`.
2. `.claude/rules/library-data.md` — the sweep's position in the scan pipeline, beside the
   existing orphan-cleanup and artwork-rollup sentence, and the fourth column with it.
3. `src/media/artwork.rs`'s `//!` — currently absent; the store's contract has grown enough
   to want one.
4. `src/media/cover_thumbs.rs`'s `//!` already says "downscaled to `thumb_size`" — after
   Phase 2 that is true rather than aspirational, and worth one clause noting a smaller
   source stays small.
5. `src/ui/util.rs`'s `COVER_SIZE` doc says "~380 px" — after Phase 4 it is exact.
6. Delete this file.

---

## Cross-cutting

- **No new setting.** The sweep is maintenance, not a preference. If a user-visible
  affordance is ever wanted, it belongs as a "Clean up artwork cache" action in Settings →
  Library beside the existing scan controls, not as a toggle.
- **The store is a cache and this plan treats it as one.** Every byte is rederivable from
  the source files, which is what makes deleting aggressively safe and what makes Phase 5 a
  background task rather than a migration. The user's own files are never touched:
  `tag_writer::cover_picture_from_path` embeds the picked cover's **original bytes** and
  decodes only to validate, so a capped store never caps what is written back to a tag.
- **Two different wins, don't conflate them.** Phase 2 is memory (smaller cache buffers, ~21%
  of covers affected). Phases 3–5 are disk plus decode CPU, and after Phase 4 the largest
  transient RGB8 buffer any tier can allocate from the store falls from tens of MiB to under
  a megabyte. The RSS reading should move on Phase 2 and on prewarm bursts after Phase 5.
- **Don't fold in the tier consolidation.** Seven `CoverThumbs` instances share
  `GRID_COVER_SIZE` and a cap of 48, and `favorites/mod.rs:129` says two of them are the
  same tier in a comment. Real, but it is a re-decode saving rather than a memory one
  (sections are mutually exclusive and release on leave), and bundling it makes both harder
  to review.
- **The HiDPI tier steps stay.** `row_cover_size` and `cover_size` are what set the 448
  floor this plan's cap is derived from; dropping the grid step would let the cap fall to
  384 for about 2 MB, at 10–30% magnification on HiDPI grid cards across normal window
  widths. It would also leave the row tier stepping with DPI and the grid tier not, while
  deleting neither `set_thumb_size` nor the deferred boot retune — both of which the row
  tier and the LRU *capacity* tuning keep alive regardless.

---

## Open questions

- **A future full-resolution artwork view must bypass the store, not raise the cap.** This
  was already the answer at 1280 and it is the premise at 512: the store exists to serve
  many small frequently-drawn tiles, and a full-screen view is one cover, on demand, which
  should decode from the *source* (the embedded tag or the folder's `cover.jpg`, both
  untouched) and drop it. Worth keeping written down because "raise `STORE_MAX_DIM`" is the
  tempting answer and it is the expensive one — at 4K it would mean ~2160 px and ~1.1 MB per
  cover, a 2 GB store at a few thousand covers.
- **Should the sweep run on every scan, or on a schedule?** Every scan is simplest and the
  cost is one directory listing plus one query. If a very large store makes that visible,
  the fallback is "on full scans only", not a timer.
- **What orphans `artists/` at 49%?** Finding 7 assumes it churns like composites do, but
  the mechanism is unconfirmed — a Deezer re-fetch after an artist rename would do it, and
  so would a rollup that re-points the row without unlinking. The sweep cleans it either
  way; knowing which decides whether there is a second bug behind it.
