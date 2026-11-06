# Plan: Indexed Songs orders: stored keys for artist, album and genre, expression indexes for year and length

Status: **Phases 1, 2 and 2b implemented, gated and measured; the hand test and Phase 4 wait on Kenan's go** · Owner: Kenan · Created: 2026-11-05

Working doc. Delete when every phase has shipped, Tests included.

## Context

**What you see today.** On the 51k-track dev library, opening Songs sorted by artist, album or genre takes three to four times as long as by title, and typing a filter in those orders pays the same. The keyset alone takes 25.8 ms for artist, 31.0 for album and 28.0 for genre, against 7.6 for title ([Evidence](#evidence-2026-11-05)).

**Why.** Those three orders sort the whole `tracks` table through `UNICODE_LOWER`, a Rust collation SQLite calls on every comparison (540,637 per artist sort). No index can carry it (`database::collation`), so every open is a full sort with a callback in its inner loop. It is also where the rusqlite bump's code-placement slowdown landed (`RUSQLITE_MIGRATION.md`, Phase 5 step 4).

**Outcome.** Each of the three fields gains a stored lowercase key with an index behind it, so the whole library opens in any of those orders as one index walk, and title order walks the `sort_key` index it already has. Year and length order, plain SQL rather than a collation, walk an index on their own expressions (Phase 2b).

- **Unchanged:**
  - every list's order, ascending and descending, row for row, and the `ui::track_sort` oracle it is held to;
  - every filter's result;
  - the album, artist and genre grids, which still sort through `UNICODE_LOWER` (Decision 5);
  - the detail pages' title tie-break, still `UNICODE_LOWER`.
- **Added:**
  - `tracks.artist_key`, `album_key` and `genre_key`, one migration, and an index per key;
  - in the same migration, `idx_tracks_year_order` and `idx_tracks_length_order` on the Songs year and length expressions;
  - their write beside `sort_key` and `search_text` in every track write;
  - a fill of the rows that predate them, inside `init_database`.
- **Removed:** `keyset::lowered`, the track lists' last caller of `UNICODE_LOWER` outside the detail tie-break.

## Decisions settled here

1. **A key is the field's Rust `to_lowercase()`, a missing field reading as `''`.** That is `COALESCE(field, '') COLLATE UNICODE_LOWER`'s order exactly: the collation compares `to_lowercase()` strings, which is a byte compare, so `BINARY` on the stored key agrees row for row. Tag-supplied sort names (`ARTISTSORT` and kin, already extracted into `SortTags`) would change what users see and stay out.
2. **Stored, filled from Rust.** SQLite's `lower()` folds ASCII alone, and the tree calls Rust only through a collation, never a scalar function (`20261018000000_library_search_text.sql` argues it), so neither a generated column nor an expression index can hold the key.
3. **NULL means not filled yet; `''` is a real key**, an untagged field's. Every write fills the three together, so `artist_key`'s NULL stands for all three.
4. **The fill runs in `init_database`, after the migrations, before the pool is handed out.** An unfilled key sorts first, so a list read before the fill would misorder, where `search_text`'s background fill only widens a filter for a moment. Not `backfill_folded`: its marker is `''`, a real key here, and it fills one column per pass. Each page is the head of `idx_tracks_artist_key`'s NULL range, which a filled row leaves, so the fill needs no cursor and no sort, and a filled library's check on every open reads an empty range. The fill is one transaction read 1,000 rows at a time: a commit per page paid a WAL checkpoint nearly every time, and a larger page held more rows for less gain (Phase 3). After a fill that wrote rows it releases the writer's page cache, which it leaves full.
5. **Each index is `(key, sort_key COLLATE NOCASE)`,** the order's first two terms, with the rowid closing it as the third, so an unfiltered library list in that order is one covering index walk with no sort.
6. **Only the whole library, unfiltered, walks an index.** Any list whose rows a filter or a scope narrows first writes its key behind `+` (and title order keeps today's `COALESCE`), so SQLite finds the rows and sorts the few left. Left to choose, it walks the index and looks up every row the filter then drops: an artist-ordered filter matching 427 rows took 12.6 ms that way against 8.1 sorted ([Evidence](#evidence-2026-11-05)).
7. **The grids keep `UNICODE_LOWER`.** They sort 3 to 5 thousand cards in 1.7 to 2.7 ms; keys there would be three more columns and write paths for about a millisecond.
8. **Title order walks `idx_tracks_sort_key` by naming `sort_key` bare.** No row has a NULL one: every track write fills it (`DerivedColumns`), and the walked list is the only order relying on that.
9. **Year and length index their expressions, not a stored key.** `COALESCE(year, 0)` and `MIN(MAX(duration_ms, 0), 2147483647)` are built-in SQL, so an index on each, with the natural key after it, is the walk with no column, no fill and no write code, SQLite keeping it. Indexing bare `year` instead would move a missing year apart from a year 0, and the year alone, without the tie-break, would still sort each year's ties. The cost is the spelling: an expression index serves only its expression written the same, so `SONGS_YEAR` and `SONGS_LENGTH_CAP_MS` name their index and a respelling sorts right and walks nothing (Phase 4 pins the plans). The indexes join `20261105120000`, unmerged, rather than a migration of their own.

## Structure (who owns what afterwards)

| File | Owns | Single reason to change |
|---|---|---|
| `migrations/20261105120000_track_order_keys.sql` | The three columns and their indexes, and the year and length expression indexes | Frozen once shipped |
| `crates/melodia-store/src/database/queries/track/order_keys.rs` (new) | `OrderKeys` (the three keys of one row, from its stored fields) and `fill_missing`, the fill Decision 4 places | How a key is derived and when an old row gets one |
| `crates/melodia-store/src/database/queries/scan/mutations.rs` | `DerivedColumns` gains the keys through `OrderKeys`, `TRACK_INSERT_COLUMNS`, `bind_track_columns` and `update_track_metadata`'s `SET` list gain the three columns | The track write's column set |
| `crates/melodia-store/src/database/mod.rs` | `init_database` calls `fill_missing` after the migrations | Open order |
| `crates/melodia-store/src/database/queries/track/keyset.rs` | `Page::Songs` carries `IndexUse` (walked or sorted), `Page::indexed` writes any indexed term bare or behind `+` from it, and `order_by` reads the keys, the year and length expressions and the natural key through it; `lowered` goes | How a track list orders |

**Duplication collapsed on the way through:** the three `lowered("t.artist" | "t.album" | "t.genre")` arms become one key reference keyed by field.

**Line budget:** `keyset.rs` (197) and `mutations.rs` (400) grow by a few dozen lines between them; the new module is small. Nothing nears 800.

## Every phase

- **Gates:** `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings`, `cargo test --locked --workspace`.
- **Doc files:** each phase updates what it makes stale, held to a fact at its anchor.
- **Existing tests** the phase breaks are brought to the new truth in that phase; new tests wait for Phase 4.
- **Hand tests and measurements** run in the dev data folder, backed up first, on Kenan's go.

## Phase 1: The keys exist and every write fills them ✅

- The migration (`20261105120000_track_order_keys.sql`): three nullable `TEXT` columns and three `(key, sort_key COLLATE NOCASE)` indexes. Its comment carries Decisions 2 to 4's reasons without naming a function, its text being frozen with its checksum.
- `order_keys.rs`: `OrderKeys::of(artist, album, genre)` lowercases each `Option<&str>`, a missing one as `''`. `fill_missing` fills the NULL range a page at a time, in one transaction, until a page writes nothing (Decision 4).
- `DerivedColumns` holds an `OrderKeys` built from exactly what `bind_track_columns` binds to `artist`, `album` and `genre`. `TRACK_INSERT_COLUMNS`, the bind order and `update_track_metadata`'s `SET` list add the three after `search_text`.
- `init_database` awaits `fill_missing` after `migrate::apply` and logs how many rows it filled.
- **Docs:** `library-data.md`'s derived-columns bullet names the keys beside `search_text`.
- **Gates (2026-11-05):** fmt, clippy and the 4,429 tests pass, none needing a change.

## Phase 2: The track lists read the keys ✅

- `Page::of` takes the needle: `Library` with an empty needle is `Songs(Keys::Walked)`, `Library` and `Favorites` otherwise `Songs(Keys::Sorted)`, every other scope `Detail`.
- `order_by` reads `t.{field}_key` for artist, album and genre through `Page::key`, behind `+` unless walked, and the natural key through `Page::natural`, bare when walked and `COALESCE`d otherwise. `detail_order` stays a sorted shape. `lowered` is gone.
- **Docs:** `collation.rs`' `//!` says what `UNICODE_LOWER` still orders; `rusqlite.md`'s Performance section takes the walked-or-guarded rule.
- **Gates (2026-11-05):** fmt, clippy and the 4,429 tests pass, none needing a change.
- **Checked on a copy of the dev library** with the migration applied, under SQLite 3.54.1 and before any statistics exist for the new indexes, as a first boot after the upgrade sees them:
  - **Plans:** each walked artist, album, genre and title order, both directions and inside the `group_concat` subquery, is a covering index scan with no sort; year order, a filtered list and Favorites sort only the rows they find; the fill's page is a search of `idx_tracks_artist_key`'s NULL range.
  - **The fill**, statement for statement as the store runs it: 51,012 rows in 308 ms, then 5 µs for the open after.
  - **Results:** 50 orders compared with today's queries (each field walked, filtered by six needles including a non-ASCII one, and Favorites, both directions, plus walked title), every one identical.

## Phase 2b: Year and length walk an index on their expressions ✅

- The migration, unmerged and so still open to edits, gains `idx_tracks_year_order ON tracks (COALESCE(year, 0), sort_key COLLATE NOCASE)` and `idx_tracks_length_order ON tracks (MIN(MAX(duration_ms, 0), 2147483647), sort_key COLLATE NOCASE)`, its comment saying why an expression and why the spelling (Decision 9).
- `keyset.rs`: `Keys` becomes `IndexUse`, `Page::indexed` writes a term bare when walked and behind `+` otherwise, and `Page::key`, `Page::year` and `Page::length` go through it on the Songs pages. `SONGS_YEAR` holds the year's spelling, and it and `SONGS_LENGTH_CAP_MS` name the index each feeds. Detail pages keep `t.year` and `t.duration_ms`.
- **The dev library** had not applied the migration (every measurement batch restored it to its pre-migration backup), so the edited one applies on its next launch with nothing to remove.
- **Docs:** `rusqlite.md`'s `ORDER BY` bullet says an expression index serves only its spelling, and the guard bullet names `keyset::IndexUse`.
- **Gates (2026-11-06):** fmt, clippy and the 4,429 tests pass, none needing a change.
- **Plans,** the edited migration applied to a copy of the dev library under SQLite 3.54.1: walked year and length, both directions and inside the `group_concat` subquery, are covering scans of their indexes with no sort; a filtered year order and Favorites in length order sort only their rows; a detail page's year order reads its album's rows by `idx_tracks_album_id`.
- **Measured by the harness** on a copy of the migrated dev library ([Evidence](#evidence-2026-11-05)).
- **Result in the app (2026-11-06):** run unattended on Kenan's go, `c57cf442` (base) against the working tree (after), fresh release builds in `../Melodia-m-keys` whose binaries differ, each build's later runs starting from the dev folder its own upgrade boot migrated. Every run quit through the tray with "All background tasks completed" and left no `-wal`, and the dev folder was restored to its pre-migration backup and checked against its manifest.
  - **Store calls,** five interleaved rounds, every result identical: Songs open in year order 17.65 to 2.55 ms descending and 17.57 to 2.53 ascending, in length order 16.73 to 2.67 ascending and 16.68 to 2.49 descending, every round within a hundredth of its median ratio; process CPU for those opens 1,076 to 166 ms. A filter keystroke in year, title and artist order, search, and the artist and title opens level, each filter's instructions equal.
  - **The upgrade boot:** migration and fill 0.510 s against 0.475, the two expression indexes' build. The database 78.7 MB against 76.0, the 662 pages the harness found.
  - **Full scan, three runs each:** 15.43 s median against 15.24, every after run behind every base one; CPU 55.0 against 54.4 s on 641 against 632 G instructions; peak threads level; peak Anonymous 498 to 686 MiB against 489 to 1,096, the spread unsettled as before. Both builds scanned slower than in Phase 3, the session's drift, so only the interleaved difference reads.
  - **Heaptrack boot:** 19.81 against 19.79 MB at 185 s, peak 32.10 against 32.03, 1.68 against 1.69 million allocation calls. The upgrade boots idled at 34.3 against 35.9 MiB Anonymous on 1.78 against 1.80 s of CPU.

## Phase 3: Measure and hand test (on Kenan's go)

1. **Plans:** `EXPLAIN QUERY PLAN` for every Songs order on the dev library, unfiltered and filtered, under the built binary's engine: a walk only where Decision 6 allows one.
2. **Store bench**, Phase 5's harness from `RUSQLITE_MIGRATION.md`, interleaved against the commit before Phase 1: Songs open in every order, a filter keystroke in artist and title order, search, the grids. Every result identical.
3. **The upgrade boot:** the dev library opening under the migration for the first time, the backup, the migration and the fill timed from the log; then a boot with nothing to fill.
4. **Footprint:** idle Anonymous and a heaptrack boot against the commit before Phase 1, and the database's size before and after.
5. **Hand test:** every Songs order both ways, unfiltered and filtered, in My Library and Favorites; a detail page in each order; a rescan and a tag edit, after which the edited track sits where its new artist puts it.

- **Result (2026-11-06),** steps 1 to 4: run unattended on Kenan's go, `db4a5312` (before) against the working tree with Phases 1 and 2 (after), fresh release builds in detached worktrees (`../Melodia-m-bump`, `../Melodia-m-keys`) whose binaries differ, on the scripts of `RUSQLITE_MIGRATION.md`'s Phase 5. The after build's runs started from the dev folder its upgrade boot migrated. Every run quit through the tray with "All background tasks completed" and left no `-wal`, and the dev folder was restored to its pre-migration backup and checked against its manifest. Step 5 has not run.
  - **Plans:** Phase 2's check stands.
  - **Store calls,** five interleaved rounds, medians per call, every result identical between builds:
    - Songs open (keyset, first window and count) in artist order 25.58 to 2.23 ms, album 30.80 to 2.29, genre 28.57 to 2.28, artist descending 28.63 to 2.27, title 9.10 to 2.23, each round within a hundredth of the median ratio. Year order is level (16.90 against 17.16 ms).
    - A filter keystroke in artist order 8.01 to 7.92 ms, in title order 7.76 to 7.89, search 1.37 to 1.41, the grids and scrolling level, rounds falling either side.
    - Process CPU for the artist, album and genre opens 1,867 to 149 ms, the artist filter 2,518 to 2,164 ms.
  - **The upgrade boot:** the backup took 88 ms, then the migration and the fill 1.27 s at 1,000-row pages, filling all 51,012 rows; every later boot of either build was ready at 0.053 to 0.061 s, with nothing to fill. The fill's cost was its commits, nearly each paying a WAL checkpoint on the NVMe. A harness running the store's statements there, timed twice and heaptracked once per shape:
    - a commit per 1,000-row page 1.21 to 1.32 s, peak heap 17.37 MB;
    - a commit per 10,000-row page 0.61 to 0.63 s, 18.97 MB;
    - one transaction read 1,000 rows at a time 0.40 to 0.44 s, 17.37 MB, and 333 rows at a time 0.39 to 0.40 s, 17.30 MB. The shared 17 MB is mostly the writer's page cache, which `cache_size` caps and the reaper frees.

    The fill is one transaction read 1,000 rows at a time since. **Confirmed in the app (2026-11-06),** upgrade boots of the pre-migration dev library, the commit-per-page build kept as the same-session control:
    - migration and fill 0.478 s, against 1.272 and 1.378 s per page; under heaptrack 0.557 against 1.407 s;
    - 1.74 against 2.48 s of CPU over the 190 s run;
    - peak heap 48.92 MB under both, the heap at 185 s 19.77 against 19.75 MB, 2.32 against 2.34 million allocation calls.

    Either fill took the upgrade boot's peak 17 MB above a plain boot's: 16.87 MB of it the writer's page cache (`pcache1Alloc`), filled to `cache_size` by the pages the fill touches and held until the reaper closed the writer at 120 s, the 135 s trim returning it.
  - **The fill releases the writer's page cache** after its commit (`Connection::release_memory`, which empties one connection's page cache and needs no `SQLITE_ENABLE_MEMORY_MANAGEMENT`), only when it wrote rows. Upgrade boots against runs 19 and 22:
    - heap from 2 to 115 s 20.9 MB against 38.0, the exact peak 31.95 MB against 48.92 (a plain boot's 32.00), the heap at 185 s 19.7 against 19.8;
    - Anonymous 40.3 MiB to the 10 s trim and 36.4 after it, against 54.9 and 51.8, both 35.3 to 35.4 MiB after the 135 s trim;
    - migration and fill 0.493 s (0.550 under heaptrack), 2.34 million allocation calls, the fill's time and work unchanged.

    The 150 s settle a footprint row needs stays: the release frees one connection's page cache, while every pooled connection keeps its map of `melodia.db`, its statements and its schema until the reaper's 120 s tick.
  - **Boot, two runs each:** rescan done at 0.31 to 0.32 s against 0.34, on 1.20 to 1.22 s of CPU over 190 s against 1.22 to 1.23, 33 peak threads and 26 idle in both, every connection closed at the 120 s tick, idle Anonymous 33.7 to 34.4 MiB against 34.1 to 39.5. Into Songs in artist order: 8.56 against 9.05 G instructions, CPU level.
  - **Heaptrack:** a boot holds 19.73 against 19.76 MB at 185 s, peaks at 31.98 against 32.00 and makes 1.73 million allocation calls in both. Into Songs in artist order, 20.05 against 20.02 MB, peak 32.29 against 32.26, 1.77 against 1.80 million calls. That pair is the re-run: the first one redrew the window throughout, 5.6 million extra calls in Slint's property bindings and none in the store.
  - **The database:** the file grew from 68.9 to 76.0 MB, the free pages taken first, and the pages in use by about 12 MB, the new indexes' statistics among them. The upgrade's pre-migration backup is 61.8 MB, as every migration's is.
  - **Full scan, three runs each:** 14.86 s median against 14.59, every after run behind every before one; CPU 48.6 against 48.5 s on 625 against 618 G instructions; peak threads level; peak Anonymous 332 to 480 MiB against 337 to 637, the spread step 1 of `RUSQLITE_MIGRATION.md`'s Phase 5 already found. The keys' three indexes cost the scan about 0.3 s, more than the 0.09 s the tmpfs harness put on them.

## Phase 4: Tests

Tests are added where the landed change earns them, on Kenan's go.

## Risks / notes

- **The fill at open** adds its time to the first boot after the upgrade, once: 1.27 s for 51,012 tracks with a commit per page in the app, about 0.4 s as one transaction by the harness (Phase 3), plus the pre-migration backup every migration takes. It grows with the library, holds 1,000 rows at a time whatever its size, and leaves the boot's peak heap a plain boot's.
- **Disk:** 9.9 MB on the dev library, 4.0 for the keys and 5.9 for the indexes, about 195 bytes a track. File-backed, through the reader's mmap, never Anonymous; each pre-migration backup grows with it.
- **Writes:** the keys cost nothing measurable; their indexes cost a full scan of the dev library about 0.3 s on disk (Phase 3). The year and length indexes added 0.4 to 0.6 s to re-inserting every track through the triggers on the NVMe, which a scan overlaps with parsing, and about 0.2 s to the app's full scan (Phase 2b). Play counts, ratings, favourites and the last position touch none of the indexed columns, so playback's writes pay nothing.
- **An expression index serves one spelling.** A respelled `SONGS_YEAR` or a changed `SONGS_LENGTH_CAP_MS` orders right and stops walking, with no error; Phase 4 pins each walked order's plan.
- **A row inserted by raw SQL** (test fixtures) carries NULL keys until a fill runs. None of the existing tests needed a change; Phase 4's tests seed through the ingest path or run the fill.
- **Downgrade:** an older build refuses the migrated database as it refuses any unknown version.

## Evidence (2026-11-05)

A scratch harness on rusqlite 0.41.1 and SQLite 3.54.1 with Melodia's reader pragmas and comparator, on a copy of the dev library (51,012 tracks), the keys backfilled from Rust's `to_lowercase`. Medians of 30 calls of the keyset (`group_concat` of ordered ids), every result identical to today's, ascending and descending.

| Order | Today | Key, no index | Key + `(key, sort_key)` index |
|---|---|---|---|
| Artist | 25.8 ms | 12.3 ms | 1.64 ms |
| Album | 31.0 ms | 12.3 ms | 1.75 ms |
| Genre | 28.0 ms | 16.0 ms | 1.75 ms |
| Title, `sort_key` bare | 7.6 ms | | 1.55 ms |

- SQLite's own working memory per sort falls from about 2.9 MB to 0.8 MB (title 2.0 to 0.85).
- **Filtered, artist order**, with the key behind `+` against today: 49,967 rows 27.1 to 14.1 ms, 14,792 rows 14.3 to 10.5, 9,615 rows 11.8 to 9.4, 1,022 rows 9.1 to 8.7, 427 rows 8.6 to 8.1, none 8.0 to 7.8. Favorites (24 rows) answers from `idx_tracks_is_favorite` either way.
- **Writes**, every track re-inserted through the table's triggers in 200-row transactions, three runs each: 479 to 510 ms today, 475 to 480 with the keys, 574 to 580 with the keys and indexes.
- **The tracks triggers** are `UPDATE OF` named columns, none of them a key, so the fill fires none.
- **Year and length (2026-11-06),** on a copy of the dev library already migrated and filled, 342 tracks yearless and none at year 0 or below: the keyset in year order 15.7 ms ascending and 15.5 descending to 1.75 both, in length order 14.8 and 14.9 to 1.74, every result identical, SQLite's working memory per sort 2.1 MB to about 0.65. The two indexes took 662 pages, 2.7 MB. Filtered in year order, the guarded sort against the unguarded walk: 'o' 18.5 against 12.4 ms, 'orange' 8.4 against 14.8, '199' 10.0 against 14.1.
- **Their writes,** every track re-inserted through the triggers on the NVMe, three runs each: 1.96 to 2.02 s with the keys' schema, 2.44 to 2.54 s with the two expression indexes.
