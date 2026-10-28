---
paths:
  - crates/melodia-app/src/library/**/*.rs
  - crates/melodia-store/src/database/**/*.rs
  - crates/melodia-core/src/entities/**/*.rs
  - crates/melodia-store/src/media/ingest/**/*.rs
  - crates/melodia-artwork/src/media/image/**/*.rs
  - crates/melodia-app/src/tasks/**/*.rs
  - crates/melodia-core/src/utils/audio_ext.rs
  - crates/melodia-core/src/utils/self_writes.rs
  - crates/melodia-views/src/ui/playlists/callbacks/**/*.rs
  - migrations/**/*.sql
  - crates/melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint
  - crates/melodia-ui/ui/components/dialog/tag-editor-body.slint
  - crates/melodia-ui/ui/components/star-rating.slint
---

# The library: scan, projections, and the write-through paths

The backend data model and the features that write back into user files or into virtual
membership. Each module argues its own design and each constant its value; this file holds what
couples the store to the app or the UI, or breaks from outside its owner. `sqlx.md`, `lofty.md`,
`blake3.md` and `rayon.md` hold the per-crate mechanics.

## Scan and change signalling

- **Scan ingest is chunked**: a write transaction per `TX_CHUNK_FILES` files, then one for the
  orphans, cover roll-ups and stats, after which `library_changed` bumps once if anything changed.
- **The artwork sweep runs after a scan's transaction commits, never inside it**
  (`tasks::artwork_sweep`), against one reference set: `queries::artwork::ARTWORK_COLUMNS`, the
  ledger every read, re-point and clear of a stored path is pinned to. **A column naming a stored
  file joins that ledger in the migration that adds it**, or the sweep deletes what it names; the
  last three (a playlist's composite, a station's logo, a browsed station's cached logo) can't be
  rebuilt. `artwork::sweep` argues the gates and the grace window, `tasks::radio_logo_cache` why
  its prune runs first.
- **Every scan entry repairs before it walks**, the sweep's inverse (`library::scan::repair`).
- **Play-count flushes bump `stats_changed` alone**; everything structural (scans, the watcher,
  imports, favourites) bumps `library_changed`. `AppState` argues both.
- **Every launch with watching on re-scans each folder** (`tasks::resume_watching::run`), so that
  reconcile, with almost nothing to re-parse, is the scan's common case and its incremental filter
  the part worth keeping fast.
- **`utils::audio_ext::is_audio_extension` is the one audio-extension predicate**, asked of every
  file the walk sees; never re-roll a lowercase-and-contains at a new call site.
- **The scan paths extract through the lenient `metadata::extract_or_filename_row`, and the tag
  write's re-read through the strict `extract_metadata`**, each arguing why.
- **`date_modified` comes from a `Metadata` already in hand**
  (`metadata::date_modified_from_metadata`), `scanner::track_is_current` comparing it byte for
  byte: a second `stat` can pair a new mtime with the previous scan's size, which then reads as
  current forever (`queries::scan::WatchedChange::Renamed`). Nothing inside a write transaction
  `stat`s a file.

## Query shape

- **`database::placeholders(n)` builds every IN list**, paired with `chunked_in_query`; never
  re-roll `repeat_n("?", n)`. A per-row UPDATE is one chunked CTE statement per batch, the shape
  `batch_update_hashes` writes.
- **Each `tracks` projection is a narrow `FromRow` struct in `entities::track`**, its columns in
  `queries::track::columns`. Pick the slimmest; a list never takes the full `Track`.
- **A list that can grow with the library holds ids, not rows**: `queries::track::keyset` and
  `queries::entity_keyset` order and filter in SQL, and the rows near the viewport come back by id
  (`ui-patterns.md`'s `PagedRows`). Recently Played's Songs tab stays off it, capped at
  `RECENTLY_PLAYED_LIMIT`; an uncapped list never may.
- **Every keyset arm reproduces `ui::track_sort`'s**, the oracle
  `crates/melodia/tests/cross_tier.rs` holds it to across crates that can't name each other.
  `get_all_tracks_for_list` serves no list and takes no sort: it is that test's input.
- **A card's right-click Play queues through `queries::track::entity_ids`**, a second SQL spelling
  of the page's **default** order (`keyset::detail_order`). The page's default is the
  `resolve_view_sort` literal in each detail file and moves separately; `cross_tier.rs` holds the
  two together. Never copy `TRACK_LIST_ORDER` into a batch query.
- **`tracks_fts` takes a ninth column only through a migration, and that is three edits**: the
  rebuild (fts5 has no `ALTER`; the newest, `20260910000000`, argues the columns, tokenizer and
  weights), `SearchFields`, the filter boxes' hand mirror `search_tests` pins to the index, and a
  migration blanking `tracks.search_text` for the per-launch backfill. `queries::search` argues the
  query shape and the folding asymmetry.
- **Nothing outside the FTS migrations and `search_tests` names a shadow table**
  (`tracks_fts_{data,idx,docsize,config}`), fts5's private storage.
- **A statement writing title, artist, album artist, album or genre writes `search_text` beside
  it**, as only `bind_track_columns` does today (`DerivedColumns`). The backfill fills blanks only,
  so a miss leaves every library-sized filter matching the old name.
- **A card's name and its `search_name` are written together** (`queries::search_name`), the folded
  copy each of `albums`, `artists`, `genres` and `playlists` keeps for its grid's filter. An album
  packs its displayed artist line, refilled after `replace_album_credits`; an artist its sort name,
  refilled in `apply_artist_details`. The backfill fills blanks only.
- **`UNICODE_LOWER` is query-side and never schema** (`database::collation`): the place to break it
  is a migration, far from that module.

## Ratings

- **A rating never changes list membership, so every surface writes it optimistically**, Search
  included, through `wire_row_flag!` and a paged list's `PagedRows::patch`. `library::ratings`
  mirrors `favorites` down to the playing track's copy (`player::state::sync_current_track_if_in`).

## Write-through to files

- **Tag editing lands through the scan pipeline, never a hand-built `UPDATE`** (`library::tags`
  argues it). The form is diffed against its populate-time snapshot as `Keep`/`Clear`/`Set` per
  field, so a batch edit writes only what was touched; `SelfWrites` keeps the watcher off its echo;
  and the refresh is the `library_changed` bump rather than a patch, a retag being able to change
  list membership. `tag_writer` argues the per-format keys, `cover_embed` the picture.
- **A release-level tag is written to every selected file and read off `albums`**, the dialog
  folding it across the selection. `upsert_album` can raise one and never lower it, so a clear goes
  through `queries::album::clear_release_tags` after the commit's upserts. **Editing a subset of a
  release isn't durable**: the untouched files keep the old value, and the next re-ingest of any of
  them puts it back.
- **Playlists export to Extended M3U8** (`library::playlist_files` argues the shapes and the
  skip-and-report import). A tag edit rewrites the file and its `file_hash`, so an exported
  `#MELODIA-HASH` goes stale; the import re-matches by path first.

## Smart playlists

- **Membership resolves from the stored criteria at every read and is never materialized**
  (`entities::smart_criteria`, `queries::smart_playlist` and `library::smart_playlists` argue the
  format, the injection contract and the evaluation).
- **Wherever a playlist may be a smart one, what it holds is asked through `queries::track`'s
  `Membership`, never a join on `playlist_items`**, which answers nothing for a smart list, quietly.
  Its automatic thumbnail is filled on read (`smart_playlists::fill_from_rules`), so the artwork
  sweep holds nothing for it.
- **`PlaylistRow.is_smart` gates off every manual-membership edit in Slint**: reorder, remove, file
  drop and Add to Playlist, each writing `playlist_items` a smart list never reads.
- **The editor's inline `@tr` dropdown arrays mirror `entities::smart_criteria`'s constant arrays
  by position** (`FIELDS`, `ops_for`, `MATCH_MODES`, `LIMIT_ORDERS`), held by
  `smart_criteria_tests`, which reads the `.slint`.
- **A play-count flush recounts only the smart lists whose criteria read play stats**
  (`smart_playlists::reading_play_stats`).
