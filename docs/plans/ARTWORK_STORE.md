# The Artwork Store

Working doc. Delete when the feature ships.

Status: **proposed** · Created: 2026-08-16

> Every number below was **measured on 2026-08-16** against the developer's own
> `~/.local/share/Melodia/` (511 tracks, 401 with artwork), not estimated. Code
> references are against this tree at the same date, and against `image-0.25.10` in the
> registry.

---

## The observed state

`~/.local/share/Melodia/artwork` holds **226 files / 36.9 MB**. Of those, **38 files —
27.5 MB, 75% of the store — are referenced by nothing**: no track, no album, no artist
row. Nothing in the tree ever deletes them. There is no prune, no sweep, no orphan
collection; `crash_report::prune` and `flexi_logger` clean `logs/`, and the artwork store
has no equivalent.

| | files | size | above a 1280 px / 1 MiB cap |
|---|---|---|---|
| **orphaned** | 38 | **27.5 MB** | 4 files / 21.0 MB |
| live | 188 | 9.4 MB | 1 file / 1.1 MB |

The orphans are where the weight is, and they are also where every oversized file is —
which decides the phase ordering below.

The store keeps **source bytes verbatim** — no re-encode, no downscale — so its shape is
whatever the user's tags happened to carry. Long edge across all 226 files:

```
n=226   min=120   p25=480   median=500   p75=1280   p90=1280   p99=1970   max=6016
```

`~/.local/share/Melodia/artists` is the same store under a different name — identical
content-addressed scheme, identical missing prune — at 80 files / 1.4 MB.

---

## What already works, and must not be "improved"

**The store is content-addressed and the dedup is exact.** `artwork.rs:111` hashes the
image bytes with BLAKE3 and takes the first 8 as the filename; the writer guards on
`!file_path.exists()`. So identical artwork bytes produce one file, one DB path string, and
therefore one entry in every path-keyed cache downstream. Measured: **226 files, 226 unique
contents, zero duplicates**; 188 distinct covers serving 401 tracks.

Nothing here changes that. A reader arriving at this plan should not read "artwork dedup"
as work to be done — it is done, and the truncated 64-bit hash is fine (birthday bound
≈ 3×10⁻¹⁰ at 100 000 covers).

---

## Six findings that decide the shape

1. **A sweep, not a refcount.** Artwork is shared, so a per-track delete must *not* unlink
   the file — 11 of 12 rows may still point at it. A refcount would have to be exactly
   right across scan ingest, orphan cleanup, both watcher delete/rename paths, tag edits
   that replace a cover, and composite regeneration; undercount and live art disappears
   silently, overcount and it leaks anyway. A reference sweep cannot undercount because it
   never counts. It also cleans the **existing** 27.5 MB, which a refcount can't — there is
   nothing to decrement — so a refcount would need a one-time sweep regardless, at which
   point it is redundant machinery.

2. **The store holds what the tag carried, and every tier decodes it in full.** The largest
   consumer is `GRID_COVER_SIZE_HIDPI` at 448 px; `COVER_SIZE` is 384 and `compose_artwork`
   resizes its sources to 600. Nothing reads more. Meanwhile `decode_capped` decodes at full
   resolution and resizes afterwards — no JPEG DCT scale-on-decode — and the tiers share no
   decodes with each other, so row, grid, strip, mosaic and `ArtworkCache` each decode the
   same source independently. The 6016×3384 file is a **~58 MiB RGB8 buffer** per decode, to
   draw at 448 px.

3. **The size distribution is clustered, and a cap placed below a cluster makes the store
   *bigger*.** 55 of 226 files sit at exactly 1280 px on the long edge — 24% of the store,
   a canonical source size. That decides the cap:

   | cap | files it would re-encode |
   |---|---|
   | 1024 | 61 |
   | 1200 | **59** |
   | **1280** | **4** |
   | 1440 | 4 |

   And re-encoding them would be actively harmful: that cluster averages **0.167 B/px**,
   with a tail down to 0.030 B/px, where a JPEG q90 re-encode lands around 0.25–0.4 B/px.
   Normalizing them to 1024 would produce **larger** files while losing a generation of
   quality. Hence `STORE_MAX_DIM = 1280` **and** the never-inflate rule in Phase 4 — the
   rule is what makes the number forgiving rather than load-bearing.

4. **The caches upscale a fifth of the store, and the store does not.** `cover_thumbs.rs:273`
   calls `thumbnail_exact(thumb_size, thumb_size)`, which produces exactly that regardless of
   the source; `artwork_cache.rs:110`'s `thumbnail(COVER_SIZE, COVER_SIZE)` routes through
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

5. **Three of the four writers are not atomic, and the failure is permanent.**
   `compose_artwork` (`artwork.rs:331`) stages through `NamedTempFile` and `persist`;
   `cache_image_file` (`:156`), `extract_and_cache_artwork` (`:202`) and `deezer.rs:257`
   all call `fs::write`. A crash, a full disk or a force-exit mid-write leaves a truncated
   file — and because the name is content-addressed and every writer guards on `exists()`,
   **it is never rewritten**. `CoverThumbs` then caches the failed decode as `None`, so the
   cover stops displaying and stays that way until someone deletes the file by hand. The
   sweep won't help: it is still referenced. This contradicts the rule `database/backup.rs`
   states outright — staged under `.tmp` and renamed, so the final name existing means the
   file is complete — and it contradicts its own sibling fifty lines below it.

6. **Composites churn; the sweep is periodic, not one-time.** `compose_artwork` hashes its
   composed 600×600 output, so **every change to a playlist's top-4 writes a new file and
   orphans the old one**. A playlist edited weekly generates a new composite weekly. This is
   a growth *rate*, not a backlog, and it is the reason the sweep runs on every scan rather
   than once at upgrade.

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
src/database/queries/artwork.rs  NEW: the three-column reference query
src/media/cover_thumbs.rs   the tier decode (Phase 2 clamp)
src/ui/artwork_cache.rs     the cover + blur decode (Phase 2 clamp)
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
  predicate is "16 hex chars + a known extension", not "written by the scanner". A
  composite that fails the predicate is deleted on the next scan and every playlist mosaic
  in the app goes blank.
- **Normalization happens at the writer, not at the caller.** All three writers funnel into
  one `store_image(bytes, dir) -> Option<String>`; a caller that decides its own cap is how
  the store ends up with two size policies.
- **The Phase 2 clamp belongs at each decode, not in a shared helper.** The three call sites
  want different things — square, aspect-preserved, and deliberately aspect-*distorted* — so
  a single "fit" helper would have to take a mode argument that is really just the three
  call sites written down again.
- **`src/ui/` reaches none of the store work.** The sweep is a scan-pipeline concern and runs
  where the orphan cleanup already runs.

---

## Phases

Phases 1 and 2 are independent correctness/memory fixes that hold regardless of what the
store contains; ship them in either order. Phases 3–5 are the store itself.

**Phase 3 is the one that pays.** On the reference library the sweep takes 36.9 MB → 9.4 MB
and removes every oversized file as a side effect, because all four live in the orphan set.
Phases 4 and 5 then touch exactly **one** file there. They are insurance against libraries
that aren't this one — a collection of 3000 px iTunes artwork, where the store grows without
bound and every tier pays a full-resolution decode for it. Worth building; not worth
overselling.

### Phase 1 — Atomic writes · no visible change

1. Give `artwork.rs` one private `write_atomic(dir, filename, bytes)` using
   `NamedTempFile::new_in` + `persist`, lifted from `compose_artwork`'s existing body.
2. Route `cache_image_file` and `extract_and_cache_artwork` through it. Re-point
   `compose_artwork` at it too, so the shape exists once.
3. Same treatment for `deezer.rs:257` (artist images).
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
2. **`artwork_cache.rs:110`** (`thumbnail`, aspect-preserved) — clamp the target box the
   same way; `resize_dimensions` will then compute a ratio ≤ 1.
3. **`artwork_cache.rs:115`** (`thumbnail_exact(BLUR_TARGET, blur_spec.height)`) is the
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
   `tracks.artwork_path`, `albums.artwork_path`, `artists.image_path`, reduced to
   basenames. Three columns; miss one and the sweep deletes live art.
2. `artwork::sweep(dir, referenced, grace) -> SweepReport` — list the directory, keep
   anything failing the filename predicate, keep anything in `referenced`, keep anything
   whose mtime is inside `grace`, delete the rest. Returns counts and bytes for the log.
3. **The grace window is the concurrency answer.** A tag edit or a parallel scan worker can
   have written a file whose DB row hasn't committed; without a window the sweep deletes it
   and leaves a dangling reference. An hour is generous and costs one extra scan cycle.
4. Call it from `library/settings/folders.rs`, **after** the orphan-delete transaction
   commits (`:409`), for both directories. Log the report through `services::describe`.

**Exit:** 36.9 MB → 9.4 MB on the reference library, on the next scan. Dangling references
stay at 0 — assert it in the same pass, since a sweep that creates one is worse than the leak
it fixed.

### Phase 4 — Normalize on ingest

1. `store_image(bytes, dir)`: decode-validate through `image_decode::capped_limits`, then
   re-encode **only when the source exceeds a bound** — long edge > `STORE_MAX_DIM`, or byte
   length > `STORE_MAX_BYTES`. Below both, write the original bytes untouched.
2. **`STORE_MAX_DIM = 1280`**, argued at its definition from finding 3: it sits *above* the
   dominant cluster in real data, where 1200 or 1024 would sit just below it and trigger a
   mass re-encode for a 6% dimension reduction. It is also ~2.9× the largest consumer
   (`GRID_COVER_SIZE_HIDPI` 448) and comfortable for **MPRIS**, the one consumer we don't
   control — `media_controls/mod.rs:254` hands `file://<artwork_path>` to the desktop shell
   for lock screens and media popups at sizes it picks.
3. **`STORE_MAX_BYTES = 1 MiB`**, a second and independent trigger: dimensions drive decode
   cost, bytes drive disk, and a file can fail either alone. The reference library has
   exactly one live example — 1107×1106 at 1112 KB, **0.93 B/px** — which the dimension rule
   would miss entirely. It is re-encoded at its existing dimensions; the byte rule never
   resizes.
4. **Never write the normalized version unless it is smaller than the source.** This is the
   rule that makes the cap forgiving: without it, a cap chosen slightly too low inflates the
   store (finding 3), and a cheaply-encoded source is *always* at risk of growing under a
   fixed quality. With it, a badly-chosen cap can waste CPU but can never do damage.
5. **Nothing is ever enlarged.** The bounds are a ceiling, not a target — a 120×120 cover
   stays 120×120 and byte-identical, having failed neither bound and so never being decoded
   at all. On the reference library 221 of 226 files are untouched by both rules.
6. Re-encode to JPEG at quality 90, matching `compose_artwork`.
7. **Hash the stored bytes, not the source bytes.** The filename must describe what is on
   disk or the dedup guard starts lying — two sources that normalize to the same output
   should collapse to one file, and a re-encode not reflected in the name makes `exists()`
   skip a write that should have happened.

### Phase 5 — Renormalize the existing store

Phase 4 only reaches files scanned after it ships; `track_is_current` guarantees the
existing ones are never re-derived.

1. A one-shot background task, dispatched after boot on the blocking pool, gated on a
   `settings.json` marker so it runs once per install.
2. For each **distinct** `artwork_path`: if it is within both bounds, do nothing. Otherwise
   re-encode through `store_image`, which yields a new hash and a new filename, then
   `UPDATE` every row pointing at the old path.
3. The old file is now unreferenced and **the next scan's sweep removes it** — no delete
   logic here at all, which is the point of doing this after Phase 3 rather than before.
4. Not an SQLx migration. It is a data pass over a rebuildable cache, it is slow, and a
   migration failure is fatal at boot (`CLAUDE.md`) — this must not be able to stop the app
   opening.

**Expected on the reference library:** one file, ~1.1 MB → ~0.3 MB. Everything else the
normalization would have caught is already gone by Phase 3. Say so in the log rather than
reporting a percentage.

### Phase 6 — Tests

Source walks, matching the tree's existing style:

1. **No `fs::write` on an artwork path** — over `media/artwork.rs` and `media/deezer.rs`.
   The regression is a fourth writer added later, not the three fixed in Phase 1.
2. **The filename predicate rejects what the module doesn't write** — a fixture directory
   with a `README`, a `.tmp`, a hand-named `mycover.jpg` and a real hash-named file; only
   the last is a sweep candidate.
3. **The reference query names all three columns** — a walk, since the failure mode of a
   missing column is deleting live art rather than a compile error.
4. **A composite survives a sweep** — the case where the predicate and the reference set
   disagree, and the one that blanks every playlist mosaic if it regresses.
5. **The grace window keeps a just-written file**, with the clock injected rather than
   slept on.
6. **No tier upscales** — a source smaller than the tier yields a buffer at the source's
   size, at all three Phase 2 call sites, plus the blur tier keeping its target aspect.
7. Behavioural on `store_image`: a below-bounds source comes back byte-identical; an
   above-bounds one is smaller than its source and its filename matches the hash of what was
   written; and a cheaply-encoded source that would *grow* under re-encode is left alone.

### Phase 7 — Docs and exit

1. `CLAUDE.md` — the `media/` bullet gains the store's invariants: content-addressed,
   atomically written, bounded on ingest, swept against three columns. Two lines; the detail
   belongs in the module's `//!`.
2. `.claude/rules/library-data.md` — the sweep's position in the scan pipeline, beside the
   existing orphan-cleanup and artwork-rollup sentence.
3. `src/media/artwork.rs`'s `//!` — currently absent; the store's contract has grown enough
   to want one.
4. `src/media/cover_thumbs.rs`'s `//!` already says "downscaled to `thumb_size`" — after
   Phase 2 that is true rather than aspirational, and worth one clause noting a smaller
   source stays small.
5. Delete this file.

---

## Cross-cutting

- **No new setting.** The sweep is maintenance, not a preference. If a user-visible
  affordance is ever wanted, it belongs as a "Clean up artwork cache" action in Settings →
  Library beside the existing scan controls, not as a toggle.
- **The store is a cache and this plan treats it as one.** Every byte is rederivable from
  the source files, which is what makes deleting aggressively safe and what makes Phase 5 a
  background task rather than a migration.
- **Two different wins, don't conflate them.** Phase 2 is memory (smaller cache buffers, ~21%
  of covers affected). Phases 3–5 are disk plus decode CPU. The RSS reading should move a
  little on Phase 2 and barely at all on the rest.
- **Don't fold in the tier consolidation.** Seven `CoverThumbs` instances share
  `GRID_COVER_SIZE` and a cap of 48, and `favorites/mod.rs:126` says two of them are the
  same tier in a comment. Real, but it is a re-decode saving rather than a memory one
  (sections are mutually exclusive and release on leave), and bundling it makes both harder
  to review.

---

## Open questions

- **A future full-resolution artwork view should bypass the store, not raise the cap.**
  Serving one at 4K would mean ~2160 px and ~1.1 MB per cover — a 2 GB cache at a few
  thousand covers, which nobody ships. The store exists to serve many small
  frequently-drawn tiles; a full-screen view is one cover, on demand, and should decode
  from the *source* (the embedded tag or the folder's `cover.jpg`, both untouched) and drop
  it. Worth writing down because "raise `STORE_MAX_DIM`" is the tempting answer and it is
  the expensive one.
- **Should the sweep run on every scan, or on a schedule?** Every scan is simplest and the
  cost is one directory listing plus one query. If a very large store makes that visible,
  the fallback is "on full scans only", not a timer.
- **`artists/` has no equivalent of the composite churn** — Deezer images are fetched once
  per artist. Worth confirming whether a re-fetch after a rename orphans the old image, or
  whether the row is simply repointed.
