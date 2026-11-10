---
paths:
  - crates/melodia-app/src/library/**/*.rs
  - crates/melodia-app/src/tasks/file_event_processor/**/*.rs
  - crates/melodia-store/src/database/**/*.rs
  - crates/melodia-store/src/media/ingest/**/*.rs
  - crates/melodia-store/src/media/cue/**/*.rs
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

`rusqlite.md`, `lofty.md`, `blake3.md` and `rayon.md` hold the per-crate mechanics.

## Scan and artwork

- **A column naming a stored file joins `queries::artwork::ARTWORK_COLUMNS` in the migration that
  adds it**, or the artwork sweep deletes what it names. An undo that writes back a row naming a
  file nothing can rebuild holds it in an `ArtworkHold` taken before the delete commits.
- **`utils::audio_ext::is_audio_extension` is the one audio-extension predicate**, and
  `is_cue_sheet` the one cue-sheet one; never re-roll a lowercase-and-contains at a new call site.
- **An image a cue sheet claims never reaches the ordinary ingest**: the planner
  (`media::cue::plan`) takes it out of a scan's files, a watcher batch's events
  (`file_event_processor::sheets`) and an import's files (`library::import`), and its sheet's job
  writes its tracks (`queries::scan::cue_units`). A sheet claims a file it carves into two or more
  rows, one sheet wins an image, and a sheet that fails to read keeps its claim. A new path that
  ingests a file a sheet may claim asks the planner, or the image gets a whole-file row beside its
  tracks.
- **`utils::trash_dir` is the one trash-folder predicate**: a walk or watch that finds files by
  itself asks it, never a trash name of its own.
- **Nothing inside a write transaction `stat`s a file**: a second `stat` can pair a new mtime with
  the previous scan's size, which then reads as current (`queries::scan::watch_batch`).
- **A new ingest path consults `excluded_files` inside its inserting transaction, and a rename,
  move or rewrite carries an exclusion as it carries a row** (`queries::excluded_files`,
  `scan::moves`), or a removed duplicate comes back. An exclusion covers a whole file, so nothing
  excludes a cue sheet's track (`DuplicateCopy::is_removable`).
- **A track filed outside the library links to no album, artist or genre**
  (`queries::scan::TrackFolder`): a write path resolves its ids through `resolve_track_context`
  with the row's own folder, or a file opened from Downloads lands on the album, artist and genre
  pages and in their counts. A library scan reads a row with no links as unstamped
  (`file_rows::LINKED_STAMP`), which is what links an import a library folder absorbs.
- **A cue sheet's track owns no file**: its image's tags, cover, lyrics and rating state the whole
  disc. A write into a file reaches only rows that play it whole (`get_whole_file_paths_by_ids`),
  and a read of a file's tags or lyrics for one track skips a sheet's (`TagEditRow::is_sheet_track`,
  `lyrics::LyricsKey`).

## Query shape

- **A statement keyed on a file's path says which of its rows it means**
  (`queries::track::file_rows`): one reading or writing the file's own tags adds `WHOLE_FILE`, and
  a move or a delete takes every row of the path, a file moving with all its rows. A cue sheet
  splits one file into a row per track, so a bare `file_path = ?` reaches every one of them. A
  lookup of ids by path or hash answers every row with its number (`queries::track::FileTracks`),
  the caller matching on it or taking one file's rows whole, never one row picked at random.
- **A statement listing, counting, searching or ranking the library's tracks adds `IN_LIBRARY`**
  (`queries::track::file_rows`), or a file opened from outside every library folder shows in it.
  Such a file keeps a row, which the queue, a playlist and every read by id reach, so
  `missing_ids_on` asks every row, never the `Library` scope.
- **`database::placeholders(n)` builds every IN list**, paired with `chunked_in_query`,
  `chunked_in_execute` for a statement that returns no rows, and `chunked_in_statements` for one
  binding a value ahead of its list; never re-roll `repeat_n("?", n)`, or the chunk loop for any of
  those three shapes (`ingest::load_existing` excepted, its doc says why). A per-row UPDATE is one
  chunked CTE statement per batch, its `VALUES` list from `values_placeholders`.
- **Nothing outside the FTS migrations and `search_tests` names a shadow table**
  (`tracks_fts_{data,idx,docsize,config}`), fts5's private storage.
- **A statement writing title, artist, album artist, album or genre writes `search_text` beside
  it, and artist, album or genre its order key**, as `bind_track_columns` does (`DerivedColumns`).
  The backfill fills blanks only, so a miss leaves every library-sized filter matching the old
  name, and the key fill only NULLs, so a missed key sorts the row by the old name.
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
- **A smart playlist becomes a regular one only through `queries::playlist_convert`**, which reads
  the membership inside the write that flips `is_smart` (`convert_to_regular`).
