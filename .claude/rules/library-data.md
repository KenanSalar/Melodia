---
paths:
  - crates/melodia-app/src/library/**/*.rs
  - crates/melodia-store/src/database/**/*.rs
  - crates/melodia-store/src/media/ingest/**/*.rs
  - crates/melodia-core/src/utils/audio_ext.rs
  - crates/melodia-core/src/utils/fold.rs
  - crates/melodia-core/src/utils/trash_dir.rs
  - crates/melodia-net/src/services/net/lyrics_directory/recording.rs
  - crates/melodia-platform/src/services/platform/single_instance.rs
  - crates/melodia-views/src/ui/playlists/**/*.rs
  - crates/melodia-ui/ui/views/my-library/playlist-detail.slint
  - crates/melodia-ui/ui/components/track-list/track-context-menu.slint
  - migrations/**/*.sql
---

# The library: what spans the store

`sqlx.md`, `lofty.md`, `blake3.md` and `rayon.md` hold the per-crate mechanics.

## Scan and artwork

- **A column naming a stored file joins `queries::artwork::ARTWORK_COLUMNS` in the migration that
  adds it**, or the artwork sweep deletes what it names. An undo that writes back a row naming a
  file nothing can rebuild holds it in an `ArtworkHold` taken before the delete commits.
- **`utils::audio_ext::is_audio_extension` is the one audio-extension predicate**; never re-roll a
  lowercase-and-contains at a new call site.
- **`utils::trash_dir` is the one trash-folder predicate**: a walk or watch that finds files by
  itself asks it, never a trash name of its own.
- **Nothing inside a write transaction `stat`s a file**: a second `stat` can pair a new mtime with
  the previous scan's size, which then reads as current (`queries::scan::watch_batch`).
- **A new ingest path consults `excluded_files` inside its inserting transaction, and a rename,
  move or rewrite carries an exclusion as it carries a row** (`queries::excluded_files`,
  `scan::moves`), or a removed duplicate comes back.

## Query shape

- **`database::placeholders(n)` builds every IN list**, paired with `chunked_in_query`, or
  `chunked_in_query_on` on a connection or transaction already held, and `chunked_in_execute_on`
  for a statement that returns no rows; never re-roll `repeat_n("?", n)`, or the chunk loop for a
  statement binding only its list (`ingest::load_existing` excepted, its doc says why). A per-row
  UPDATE is one chunked CTE statement per batch, its `VALUES` list from `values_placeholders`.
- **Nothing outside the FTS migrations and `search_tests` names a shadow table**
  (`tracks_fts_{data,idx,docsize,config}`), fts5's private storage.
- **A statement writing title, artist, album artist, album or genre writes `search_text` beside
  it**, as `bind_track_columns` does (`DerivedColumns`). The backfill fills blanks only, so a miss
  leaves every library-sized filter matching the old name.
- **A card's name and its `search_name` are written together** (`queries::search_name`), the folded
  copy each of `albums`, `artists`, `genres` and `playlists` keeps for its grid's filter.
- **`UNICODE_LOWER` is query-side and never schema** (`database::collation`): the place to break it
  is a migration, far from that module.

## Matching

- **A comparison that ignores punctuation goes through `utils::fold`**: `fold_words`, or `squeeze`
  on text already folded, never a character filter of its own. The duplicate finder and the lyrics
  matcher share it, so a spelling rule lands on both.
- **What makes two rows one song lives in `library::duplicates::matcher`**: anything that groups
  copies asks `library::duplicates::find`, never a match of its own.
- **A per-track column the user builds up joins `remove_duplicates`' merge**
  (`queries::track::duplicate_merge`'s `History`), or removing a copy throws it away.

## Smart playlists

- **Wherever a playlist may be a smart one, split on `is_smart` before asking what it holds**: a
  join on `playlist_items` answers nothing for a smart list, quietly (`queries::track::entity_ids`).
- **`is_smart` gates off every manual-membership edit**, each writing `playlist_items` a smart list
  never reads: reorder and remove in Slint (`playlist-detail.slint`, the row menu's
  `view-context`), file drop and Add to Playlist in Rust (`DetailPage::drop_target`,
  `add_picker`).
