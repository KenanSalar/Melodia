# Plan: query tuning after the rusqlite move

Working doc. Delete once the hand tests still owed at the end are done.

Status: **in progress** · Created: 2026-11-08

> Figures are from runs on a copy of the dev database (51,012 tracks), in memory unless noted, on
> system SQLite 3.51.2. The app bundles 3.54.1, so plans get re-read there during the work.

## What ships

What a large library pays today:

- Opening any folder in Browse reads the whole `tracks` table to find the few rows in that folder.
- A smart playlist with a Genre or Credits rule costs about 20 ms per card, on every Playlists grid
  load.
- Dropping one file onto the app recomputes every artist, album and genre count and rebuilds the
  stats triggers.
- Adding one song to a playlist rewrites every row of that playlist.
- Reordering or removing a track in a playlist reloads the grid hidden behind it.

No hot path runs a query per row, and scrolling and scanning already batch correctly. The one N+1 by
shape is the smart-playlist cards, bounded by how many smart playlists the user has made.

| # | Fix | Today | After |
|---|-----|-------|-------|
| 1 | Browse by `file_path` range | 2.8 ms, whole table (on file) | 0.007 ms, index range (on file) |
| 2a | Uncapped Genre/Credits rule, rare value | 17 ms | 0.05 ms |
| 2a | Uncapped Genre rule, common value | 16.4 ms | 8.1 ms |
| 2a | `Any` rule, genre or title | 21.7 ms | 3.4 ms |
| 2a | "Does not contain" | 17 ms | 6.7 ms |
| 2b | Favorite rule | 2.3 ms, table scan | 0.011 ms, partial index |
| 3 | Renumber a 5,000-row playlist after an add | 3.5 ms, 5,000 rows written | 1.4 ms, none written |
| 4 | Import of one file | 23 ms stats rebuild, 18 DDL statements | neither |
| 5 | Playlist edit under an open detail | hidden grid reloads per edit | one reload on close |

## Phases

Each phase carries its own doc comments and stops at `cargo fmt --all` and the clippy gate, fixing
any existing test the change breaks.

### Phase 1: Browse reads a path range ✅

Browse lists the same files, and "play this folder" plays the same subtree. One deliberate change:
on a case-sensitive disk, playing `abba/` stops pulling in a sibling `ABBA/`, which `LIKE`'s case
folding reaches today. Browse's own list already drops those rows.

- `queries/track/list.rs` owns the one helper giving a directory's `file_path` bounds.
- `get_tracks_in_directory` and `track_ids_under_directory` read through it.
- `directory_like_prefix`, `LIKE_SEPARATOR` and all escaping go, a range having no wildcards.

### Phase 2: Smart rules ✅

Every rule keeps matching the same tracks. In `queries/smart_playlist.rs` only.

- A set-valued rule tests with `tracks.id [NOT] IN (SELECT …)` when the query reads every match: an
  uncapped count or membership, and `pick_random`.
- It stays `EXISTS` where a query can stop early, capped lists and `first_cover`, where `IN` is a
  regression. A `MemberTest` enum names the choice.
- `MemberRows.correlate` becomes the bare track-id column, so both forms build from one set of
  fragments.
- `push_bool_predicate` spells `= TRUE` and `= FALSE`, which is what reaches
  `idx_tracks_is_favorite`.

### Phase 3: Writes that redo work ✅

Positions, counts and durations end up exactly as they do now.

- `renumber_playlist_positions_tx` writes only the rows that move.
- `commit_import` drops the triggers and recalculates only above the bulk threshold.
- `ingest::is_bulk(rows)` becomes the threshold's one home, replacing the comparisons in
  `Ingested::for_delta`, `watch_batch.rs` and `duplicate_merge.rs`.

### Phase 4: Don't reload a grid nobody can see ✅

The grid is current whenever you return to it. `refresh_after_write` owes the grid its reload while
a detail is open, the path a library change already takes.

### Phase 5: Tests

After the hand test and on Kenan's go. The landed diff decides what earns a test.

## Checked and left alone

| Candidate | Why not |
|-----------|---------|
| Batching smart-card counts into one statement | After 2a a card costs 4.5 to 8 ms against 16 to 21 ms. Capped sets can't share a scan, and one combined statement is hard to read |
| Narrower signal for hearts and stars | Songs' id walk is 1 ms and pruning a 51k queue 7.6 ms, in the background. It also needs every row write mirrored to every list first |
| Caching the 17-row track insert | Saves 0.27 s of a whole first scan. Single-row inserts are slower (24.7 against 18.6 µs per row) |
| `json_each` for scroll pages | 177 against 191 µs per page |
| FTS `prefix=` index | One letter is 11 ms, two letters 1.7 ms; not worth a rebuild migration |
| `WHEN` guards on the triggers | 1.5 s across a whole-library rewrite, which spends far longer on file I/O |
| Limiting the sweeps after a watcher batch | All four statements total under 2 ms |
| Album cache for watcher batches | Not measured. Only matters on a drop of thousands, and would make the first file's release tags win |
| Dropping `idx_tracks_album_id` | Looks redundant, but the album cover roll-up needs its `id` order |
| Smart list's numbering and second sort | 32 against 17 ms at 32k matches, once per open |

## Hand test

✅ Linux, 2026-11-08, in the dev data folder: browse folders with `_`, `%` and non-ASCII names; play
a folder card; open smart playlists with Genre, Credits, Favorite and capped rules and compare
counts with before; add to, remove from and reorder a playlist, then return to the grid; import one
file and check its album and artist counts. A file opened from the command line stood in for an OS
drop, both reaching `commit_import`.

Still owed, neither runnable on Linux. On Windows: browse and play a library folder at a drive root.
On Windows and macOS, whose disks ignore case by default: rename a folder in case only, then play
its card.
