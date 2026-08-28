---
paths:
  - src/library/**/*.rs
  - src/database/**/*.rs
  - src/entities/**/*.rs
  - src/media/**/*.rs
  - src/tasks/**/*.rs
  - src/ui/playlists/callbacks/**/*.rs
  - migrations/**/*.sql
  - melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint
  - melodia-ui/ui/components/dialog/tag-editor-body.slint
  - melodia-ui/ui/components/star-rating.slint
---

# The library — scan, projections, and the write-through paths

The backend data model and the four features that write back into user files or into virtual
membership. The per-crate mechanics live elsewhere and load on the same reads: `sqlx.md` for query
shape, `lofty.md` for tag access, `blake3.md` for hashing, `rayon.md` for the parallel walk.

## Scan and change signalling

- **Scan ingest is chunked + batched.** Bulk scans (`to_scan > SCAN_BULK_THRESHOLD`) ingest in
  per-`TX_CHUNK_FILES` write transactions (the writer connection frees between chunks; the
  per-chunk stats-trigger drop/create stays crash-safe) with multi-row `INSERT … RETURNING id,
  file_path` via `insert_tracks_batch` — ids mapped back **by path**, RETURNING order being
  unspecified while DnD import relies on input order. Small deltas keep the stats triggers enabled
  and skip `recalculate_all_stats` entirely. Orphans + artwork rollup + recalc land in one final
  tx; `library_changed_tx` bumps once after it.

- **The artwork sweep runs *after* that tx commits, never inside it** (`tasks::artwork_sweep`,
  spawned beside `retroactive_hash`). It deletes by reference rather than by refcount — artwork is
  shared, so no per-track delete can safely unlink a file, and a sweep cannot undercount because it
  never counts. Two gates, both required: the name has to parse back into the scheme
  `media::artwork` writes, and nothing in the reference set may name it. **That set is six
  columns** — `tracks.artwork_path`, `albums.artwork_path`, `artists.image_path`,
  **`playlists.thumbnail_path`**, **`radio_stations.artwork_path`** and
  **`radio_logo_answers.artwork_path`**. The last three are the ones that bite, each reachable
  through no other row: dropping the playlist arm blanks every custom mosaic, and dropping either
  radio arm deletes a station logo that came off a third-party host and can never be re-derived.
  The logo-answers arm is the one entry that is not a row the user owns — a browsed station's logo
  is held alive by its cache row alone, so the sweep has to see it or the cache is left naming a
  file the sweep just deleted. That also fixes an order: `tasks::radio_logo_cache` drops expired
  rows *before* the sweep runs, or every one of them still counts as referenced and the store
  never shrinks. A one-hour grace window covers the file a tag edit or scan worker has
  written but not yet committed a row for. `queries::artwork` owns both the read side and the
  `UPDATE`s the renormalize pass re-points with, pinned against one column ledger — a missing
  column is silent one way and destructive the other.

- **`stats_changed_tx` vs `library_changed_tx`.** Play-count flushes bump the stats channel only;
  its two subscribers are Favorites (hero mosaic + Most Played rank by `play_count`) and
  Recently-Played (ordered by `last_played`, written on the same flush). Everything structural —
  scans, watcher, imports, favorite toggles — stays on `library_changed_tx`.

- **First launch** auto-adds `dirs::audio_dir()` and scans. The same `first_launch::run` then
  starts the watcher and calls `reconcile_watched_folders`, which re-runs `scan_folder_internal`
  over every enabled folder — so a normal boot scans each folder once more to catch changes made
  while closed. That reconcile is the scan path's *common* case (almost nothing to re-parse), which
  is why its incremental filter is the part worth keeping fast.

- **One audio-extension predicate: `media::is_audio_extension(ext)`.** Case-folded
  (`eq_ignore_ascii_case` against ASCII `AUDIO_EXTENSIONS`), allocating nothing — the library walk
  asks it for *every* file in the tree. The walk, the watcher's `is_audio_file`, DnD/import
  validation and Browse all route through it; don't re-roll `ext.to_lowercase()` +
  `AUDIO_EXTENSIONS.contains(...)` at a new call site.

- **Two entry points into extraction, and the scan paths take the lenient one.**
  `metadata::extract_or_filename_row` keeps a filename-derived row for a file whose tags won't
  parse, which is the only way Matroska and CAF reach the library at all; both scan sites use it
  (`scanner::scan_files_parallel`, `file_event_processor::reconcile`), so an `Err` there now means
  a file that can't be *read*. `extract_metadata` stays strict, and the tag-write and MBID
  re-reads depend on that: a row built from a parse that didn't happen would blank the track
  instead of reporting the failure. Duration on a fallback row comes from
  `player::rodio_backend::probe_duration`, the one edge `media/` has into `player/`. Each half is
  argued at its own definition, including why identification is `FileType::from_buffer` and never
  lofty's junk-tolerant `Probe::guess_file_type`.

- **Derive `date_modified` from a `Metadata` you already hold.**
  `metadata::date_modified_from_metadata(&meta)` is the single source of the stored RFC-3339 mtime
  string and `scanner::track_is_current` compares against it byte-for-byte, so a second
  `fs::metadata` risks size and mtime coming from different instants. Load-bearing on the
  **watcher paths**: `update_track_location` (re-point on move/rename) writes mtime but **not**
  size or hash, so a mtime re-read at write time would land beside the *previous* scan's size, and
  an in-place tag edit that didn't change the size would then read as current to `track_is_current`
  forever. Hence `handle_created` and `handle_renamed` both take the mtime off the
  `ExtractedMetadata` the batch already extracted (an older mtime only fails toward a re-parse, the
  safe direction), and `retroactive_hash` gets its mtime from the same `fs::metadata` that proved
  the file exists. `extract_date_modified(path)` is the fallback for the one caller with nothing in
  hand: `handle_renamed` when extraction failed or the renamed-to file vanished.

## Query shape

- **`crate::database::placeholders(n)` for IN-clause lists.** Single-pass,
  capacity-preallocated; don't re-roll `repeat_n("?", n)…join`. Pair with `chunked_in_query`.
  Tuple-row CTE UPDATEs follow the `batch_update_hashes` / `flush_artwork_backfill` shape — one
  chunked UPDATE per N rows, not N UPDATEs. Runtime-built SQL `String`s (placeholder lists, column
  projections) are wrapped in `sqlx::AssertSqlSafe(sql)` at the query call site (sqlx 0.9
  `SqlSafeStr`) — data never rides in the string, only through `.bind()`.

- **Track projections by use case**, each with a `*_columns()` helper: `TrackSummary` (17 cols;
  queue/NP/playback, incl. 4 ReplayGain + `rating`); `TrackListRow` (20; lists incl. Files/Browse,
  which render through the shared `TrackList` so there is no narrower browse slice; incl.
  `rating`); `TrackMeta` (8; NP chips); `TagEditRow` (24; Edit-Tags dialog — the only production
  by-id multi-fetch, incl. composer/comment/bpm + technical Summary cols); `PlaylistExportRow` (5;
  `file_path`/`file_hash`/`title`/`artist`/`duration_ms`, M3U8 export only); `Track` (44; scan
  ingest, hash backfill, detail, fixtures). Pick the slimmest — `SELECT *` into `Track` for a list
  view costs ~24 unused decodes/row.

  - **The other half of narrowing is `ui::track_list_cache`**: the two views that retain a whole
    list resident (My Library's Songs tab, Favorites') keep *converted* rows rather than
    `TrackListRow`s, so the columns a list doesn't render are freed at fetch rather than held for
    the session. **Recently Played's Songs tab retains a list too and deliberately stays off it** —
    `get_recently_played` is capped at 200 rows, so what the cache is for (a resident set whose
    second copy in the Slint model is the larger half of the view's footprint) doesn't apply, and
    it keeps the plain `Mutex<Vec<TrackListRow>>` plus `track_matches`. An *uncapped* fourth view
    belongs on the cache, not on a sixth projection.

  - **It is also why the whole-table fetches take no sort**: `get_all_tracks_for_list` and
    `get_favorite_tracks_for_list` lost their `sort_by`/`sort_dir` when their callers started
    re-permuting retained rows through `ui::track_sort`, and the eight-arm `track_list_order_by`
    behind them went with them — one fixed `TRACK_LIST_ORDER` const in its place, argued at its
    definition. Don't reintroduce a sort parameter for a fifth caller; retain and permute like the
    other two.

- **`tracks_fts` indexes eight columns, and adding a ninth is a migration, not an edit.** fts5 has
  no `ALTER`, so a change means dropping the table plus all three triggers and rebuilding.
  Migration `20260802000001` carries the column list, the tokenizer and the bm25 weights with the
  argument for each; `src/database/queries/search.rs` carries the query shape and the folding
  asymmetry. Two things neither of them can tell you: **a ninth column is two edits**, since the
  per-view filter boxes never touch this index and walk in-memory caches through
  `ui::row_match::search_fields`, which mirrors the column list by hand
  (`.claude/rules/ui-patterns.md` owns that side, including the two places the answers deliberately
  diverge); and **nothing outside that migration and `search_tests` may name a shadow table**,
  `tracks_fts_{data,idx,docsize,config}` being fts5's private storage, taking no foreign key and
  none of it droppable, `_docsize` least of all since bm25 reads it.

## Ratings

- **Star ratings mirror the favorite path.** Inert `tracks.rating` (0–5) surfaced via a
  **hover-revealed** `StarRating` (`melodia-ui/ui/components/star-rating.slint`) inside the
  track-row Title cell — no rating column, no in-table sort. Rides on `TrackListRow` +
  `TrackSummary`. Writes via `library::ratings::{set_rating, set_current_rating}` (clamped 0–5),
  mirroring `favorites::{set_favorite, toggle_current_favorite}` down to the `sync_current_track_*`
  helper (over `player::state::sync_current_track_if_in`) that flips the playing track's cached
  field + emits, so the NP star updates from a list-row edit — it re-checks the id under the emit
  lock, safe against a mid-write track change. Rating **never changes list membership**, so every
  surface is optimistic (`flip_rating`/`apply_row_rating`, detail siblings), wired via
  `wire_row_flag!`; Search is excluded on purpose and stays non-optimistic. NP parity via
  `wire_now_playing_rating`.

## Write-through to files

- **Tag editing = "Edit Track Information", write-through the scan pipeline**
  (`src/library/tags.rs::apply_tag_edit`, `src/media/tag_writer.rs`). Right-click rows → **Edit
  Tags…** (`Dialog.kind == "edit-tags"`); **batch is the point** — **touched-tracking is a
  Rust-side diff against a populate-time snapshot** (Keep/Clear/Set), so only changed fields write.
  **Lyrics live in the file, not the DB** (single-track tab only). The writer always targets the
  **primary tag type** (never `first_tag_mut()` — an ID3v1-only MP3 would drop
  album-artist/composer/BPM/lyrics); BPM writes `IntegerBpm` **and** `Bpm`; **M4A `Ilst` flattens
  every `pic_type` to `Other`**, so `clear_front_cover` must remove *both* `CoverFront` and `Other`
  or Replace/Remove silently revert. Cover picks decode-validate up front, so a corrupt pick fails
  the batch before any file is touched. After the write it's the scan pipeline: re-extract via
  `extract_metadata` (**never hand-build the UPDATE** — a fresh mtime beside a stale hash is the
  one state `track_is_current` can't repair) → `update_track_metadata`. Own writes stay out of the
  watcher via `SelfWrites` (TTL 30 s, `mark` per-file *before* its write). Post-commit refresh is
  the `library_changed_tx` bump, **not** an optimistic patch — a retag can change list membership.

- **Playlist import/export = Extended M3U8** (`src/library/playlist_files.rs` + the pure `m3u`
  submodule; hand-rolled writer/parser, no crate). One `.m3u8` per playlist; writer emits
  `#EXTM3U`/`#PLAYLIST:`/`#EXTINF:` + a custom `#MELODIA-HASH:<blake3>` line + an absolute native
  path, parser tolerantly ignores unknown `#` comments and a leading BOM. Import is
  **skip-and-report**: re-match each entry by `file_path` then BLAKE3 `file_hash`, always **create
  a new playlist** (the name isn't unique), count misses; never auto-imports on-disk files. UI is
  the Import / Export pills in the My Library band's Playlists-tab row
  (`melodia-ui/ui/views/my-library/tab-pills.slint`), callbacks in
  `src/ui/playlists/callbacks/files/`. A **tag edit rewrites the file and changes its
  `file_hash`**, so a previously exported `#MELODIA-HASH` line goes stale — the `file_path`-first
  re-match degrades gracefully, but re-export after retagging if hash portability matters.

## Smart playlists

- **Smart / dynamic playlists = virtual, criteria-derived membership**
  (`src/entities/smart_criteria.rs`, `src/database/queries/smart_playlist.rs`,
  `src/library/smart_playlists.rs`). `playlists.is_smart` + `smart_criteria TEXT` store a JSON
  `SmartCriteria` rule set instead of `playlist_items` — **resolved at read time**, never
  materialized (updates live). `#[serde(default)]` + a `version` field keep it forward-compatible.
  The evaluator builds WHERE via `sqlx::QueryBuilder` — **only** enum-derived
  `&'static str` fragments are `push`ed; every user value goes through `push_bind`. **The UI reuses
  the same grid + detail** (My Library's Playlists tab, where the retired nav 7 folds):
  `PlaylistRow.is_smart` **gates off every manual-membership edit** — reorder, remove, file-drop,
  Add-to-Playlist, adding being a write of orphan `playlist_items` a smart list never reads.
  `DurationMs` stores **whole seconds**, scaled ×1000 to the ms column. A **`stats_changed_tx`
  subscriber** gated on `has_stat_dependent_smart_playlists()` recounts only smart lists whose
  criteria `depends_on_play_stats`. `rating` rules need the **full** `idx_tracks_rating` (a partial
  `WHERE rating > 0` index is *not* used for `rating >= N`). **Load-bearing index alignment:** the
  editor's inline `@tr` dropdown arrays mirror the const arrays in `entities::smart_criteria`
  (`FIELDS`/`ops_for`/`MATCH_MODES`/`LIMIT_ORDERS`) **by position** — `smart_criteria_tests`
  `include_str!`s the `.slint` and pins order + length so drift fails the build. New globals must
  be in `app-window.slint`'s import **and** `export {}` or Slint prunes them from the Rust API.
