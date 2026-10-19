# Plan: Paged track lists (every library-sized list holds ids, not rows)

Status: **in progress** (Phases 0 to 2 done, Phase 3 hand-checked, measured and tested with four fixes awaiting their hand check, Phase 3.5 open to discuss, Phase 4 done, Phase 4.1 dropped, Phase 4.5 done (decided, built in Phase 5), Phase 5 built with its docs done, its hand check pending with one finding open, and its tests and census awaiting go) · Owner: Kenan · Created: 2026-10-18 · Issue #133, branch `perf/paged-track-lists`

> **Validated 2026-10-18** against `6d7e7648` (`feat/settings-sidebar`), three read-only code traces of the list views, the queue and the index/boot/measurement side, and live heap walks of the release build on the 51,012-track dev library (method in Appendix A, protocol in Appendix B).

Working doc. Delete when every phase has shipped, passed by hand, and the "after" column of [Targets](#targets) is filled from a census run, the 512-track row included.

## Context

**What you see today.** On the 51,012-track library, Anonymous memory (the memory Melodia allocated itself) climbs with every library-sized surface you open and none of it comes back while the session lasts:

| Step (one process, 2026-10-18) | Anonymous | Live heap | Added |
|---|---|---|---|
| Idle, playlist detail open | 37.0 MiB | 13.9 MiB | |
| My Library ▸ Songs open | 61.3 MiB | 38.5 MiB | +24.3 |
| Play all from Songs (51k queued) | 87.0 MiB | 70.4 MiB | +25.7 |
| Queue sheet open | 113.8 MiB | 97.6 MiB | +26.8 |

RSS reached 385 MB, past the 200 MB budget. The artwork is not the cause: decoded cover pixels were 0.15 to 3.84 MiB in every state, capped by the window rather than the library.

**Why.** Each surface keeps its own full copy of every row it can show, as owned strings:

- Songs keeps a `TrackListCache` of converted rows, sort keys and folded search keys (about 430 B per track, measured).
- The queue keeps an `Arc<TrackSummary>` per entry (about 530 B per track, measured).
- The queue sheet keeps a `VecModel<QueueRow>`, a `LinkCache` and a shadow (about 550 B per track, measured).
- By code reading, a detail view keeps three copies of its rows and the Most Played grids keep their whole uncapped list.

All of it grows linearly with the library, and the README's footprint runs (opened on a playlist) never paid for any of it.

**Outcome.** Every list that can be library-sized holds its ordered track ids (the keyset) plus a capped page cache of the rows near the viewport, fetched from SQLite by id. Sort and filter run in SQL. The queue holds ids; the engine reads a small prefetched window of full records.

- **Unchanged**, with every rule below pinned against today's code:
  - what every list shows;
  - its sort order;
  - which rows a filter matches;
  - selection, Play / Play Next / Add to Queue / Shuffle semantics;
  - gapless and crossfade;
  - the `queue.json` format;
  - idle memory.
- **Removed:**
  - `TrackListCache` and `TrackListModel`;
  - `DetailCache`'s two row `Vec`s;
  - `QueueState`'s full records;
  - the queue sheet's `LinkCache` and per-row records;
  - the per-pass `StringPool` for these lists;
  - the duplicate `to_slint_most_played_row`;
  - each card grid's in-memory copy, sort and filter, and the flat `Playlists.rows` (Phase 5).
- **Added:** about 4 to 8 bytes per track or entity per open list or grid, plus a few hundred cached rows.

## Decisions settled here

1. **Keyset paging, not a resident index.** Kenan chose this on 2026-10-18 over a resident compact index (about 7.5 to 9 MiB at 51k, estimate, growing with the library) and a lazy one.
   - Prior art:
     - The SQLite forum's recommended method for scrolling a large result is a keyset-driven cursor: keep the result's primary keys in order, fetch rows by key per page (<https://sqlite.org/forum/info/17ad7a9b3905d487>).
     - Android Paging sizes the cache as `maxSize >= 2 * prefetchDistance + pageSize` and shows placeholders while a page loads (<https://developer.android.com/reference/kotlin/androidx/paging/PagingConfig>).
     - Slint's `ListView` instantiates only visible rows over a custom `Model` (<https://docs.slint.dev/latest/docs/rust/slint/trait.Model.html>).
   - What it buys: idle memory stays flat at any library size, and an open list costs bytes per track instead of hundreds.
2. **Sort runs in SQL and reproduces every arm of `ui::track_sort` exactly.**
   - Register a Unicode lowercase collation on both pools through `SqliteConnectOptions::collation`. It replaces `to_lowercase()` on artist, album and genre, and on a detail view's title tie-break.
   - Today's arms (`track_sort.rs:112-163`):
     - **Title:** `sort_key COLLATE NOCASE`, which matches `to_ascii_lowercase()`.
     - **Track number:** `(disc, ±track, secondary)` with the `i32::MAX` sentinel for a missing track, never reversed as a whole.
     - **Year:** NULLs first. Songs and Favorites read a missing year as `0` (`COALESCE(year, 0)`, not `NULLIF`), so it sorts beside an explicit 0 and after negative years, pinned at `track_list_cache_tests.rs:339`.
     - **Length.** Songs and Favorites clamp it to `0..=i32::MAX` ms, as their cached row holds it.
     - **Tie-breakers:** Songs and Favorites break ties on `sort_key`, the details on lowercased title, playlists on position.
     - **Descending:** reverses the ascending order.
   - SQL appends `id` as the last key so the order is total, which is what makes `DESC` an exact reverse. Rows tied on every key today come back in whatever order the fetch gave them; they will now come back in id order. That is the only visible difference and it is deliberate.
   - `track_sort` stays, because the bounded lists still sort in memory, and it becomes the oracle the SQL parity tests compare against.
3. **Filter runs in SQL against a stored folded column, `tracks.search_text`.** It is `SearchFields::pack()`: `fold::push_folded` over title, artist, album_artist, album and genre, joined by `\n` (amended 2026-10-18 from `\0`: `instr` walks past a NUL, but `SQLite` leaves NUL inside TEXT undefined and `length()` already stops there, while Slint turns a newline pasted into a single-line box into a space, so no needle carries one).
   - The match is `instr(search_text, ?) > 0`. When the needle is all digits, it also accepts `instr(CAST(year AS TEXT), ?) > 0`, which is `Needle::matches_number`.
   - `SearchFields` and `Needle` move to `melodia-core`, so the store writes the column with the same function the bounded lists match with, and takes a `&Needle` rather than a string that could be either spelling.
   - sqlx exposes collations but no scalar functions, so a stored column is the only way to keep today's accent-folding semantics without `unsafe`.
   - **The backfill runs every launch off the column itself, not a one-shot settings marker** (amended 2026-10-18). A filled value is never `''`, so `''` is "not filled yet", `retroactive_hash`'s shape. A marker in `settings.json` would survive a restored pre-migration backup that re-adds the column empty. A partial index over the unfilled rows keeps the per-launch check free once the library is filled.
   - **No filter gate** (amended 2026-10-18): the clause is `search_text = '' OR instr(…)`, so a row the backfill hasn't reached matches every needle rather than none.
4. **One Slint model type for every paged list: `ui::paged_rows::PagedRows<R>`.** It holds:
   - the keyset as an `Arc<[i64]>`;
   - an LRU of rows **keyed by id** (amended 2026-10-18 from pages), capped at 512 rows, the Android rule over 64-row pages and 2 pages of prefetch either side. A row reads the same wherever it sits, so a resort, a refilter or a removal keeps every cached row valid;
   - the selected-id set, which `TrackListModel` already kept as a `HashSet<i32>`;
   - a read stamp per row instead of a generation: a row read before the one the list holds never replaces it, so a late fetch can't revert a patch.

   `row_data` answers from the cache, or returns a placeholder row (id, `selected`, empty text) and asks for what the window lacks. The window is the page Slint last read and 2 pages either side.

   **One fetch is in flight per list** (amended 2026-10-18, after the first hand test). It asks for up to 128 missing rows nearest the row Slint read last, and each landing asks again from wherever the list has got to. The query and the conversion run on tokio, and a landing repaints only the window. The first cut fetched every page Slint passed and polled the fetches on the UI thread: a fast drag on the 51k library queued enough of them that the read pool logged 2 s waits, and each landing walked the whole keyset to find its rows.

   **A load seeds the window where the list is scrolled** (amended 2026-10-18), read off `ScrollPoint`, Slint keeping the scroll offset across a reset. Seeding the top painted placeholders after any sort made while scrolled down.

   Two things must hold:
   - **`selected` never depends on the page.** `track-list.slint:116-117` and `queue-sheet.slint:297-298` read the neighbours' `.selected`.
   - **The `ModelRc` is installed once per list.** A new keyset swaps inside it and calls `reset`, because `Repeater` compares models by pointer.
5. **The queue holds ids. The engine reads a prefetched window, and a miss defers rather than blocks.**
   - **Shape:** `QueueState.tracks` becomes `Vec<i64>`. A new `TrackWindow` holds at most six `Arc<TrackSummary>` keyed by id: previous, current, next under the repeat mode, next under manual Next (they differ under repeat-one, `queue.rs:261` vs `:304`), and the two after that for the auto-skip chain (`actions.rs:276-285`).
   - **Filling it:** an app task, `tasks::queue_window`, watches the queue version and fetches missing ids with `get_track_summaries_by_ids`. It also reads every record again on a library tick (amended 2026-10-19), which keeps an upcoming track's heart, stars and path current and is now how a retag reaches Now Playing: `apply_tag_edit`'s own resync went. The four start sites that are already async (Play, a queue row's double-click, the restore and the prune's reseat) fetch the records they start from ahead of the emit.
   - **A miss:** every synchronous transition reads the window. On a miss it moves the queue, leaves the deck empty under `Loading` so the monitor neither ticks nor ends the drained stream a second time, and returns `PlayerAction::StartWhenResolved(AwaitedStart)` (amended 2026-10-19 from `{ id, mode }`: a crossfade never defers, it bails as it does with no next, and the start carries a re-pause and the auto-skip count, so the whole-lap guard holds across a deferral). The engine can't name the database, so `execute_actions` publishes it on `sinks.awaited_start`; the window task fetches the record and calls `actions::start_awaited`, which, while the state is still `Loading` with no source, starts whatever the queue's current slot holds once the window has its record, or stops where the queue emptied (amended 2026-10-19 from requiring the slot and id the transition chose: a queue edit landing during the fetch left the player stuck under `Loading`, Play and Pause doing nothing). The gapless and crossfade gates treat a missing next as "no next yet" and re-ask every 500 ms tick, so a transition loses gapless only if the window is still empty in its final `PRELOAD_LEAD_MS`.
   - **Publishing:** `QueueViewModel` carries the ids as an `Arc<[i64]>` shared copy-on-write with `QueueState`. Publishing on every advance becomes O(1), where `state/mod.rs:240-244` rebuilds a whole `Vec<Arc<_>>` under the lock today.
   - **Duplicates and missing rows** keep today's answer: `get_track_summaries_by_ids` drops both (`lookup.rs:98-101`), so `player_play_tracks` dedups and drops through `existing_ids`.
6. **Lists refetch their keyset, never their rows.** A `library_changed` tick refetches ids (one indexed query) and re-reads the rows the list holds, those around the viewport first (amended 2026-10-18 from dropping the cache, which would put every row on screen back on placeholders at each tick). A heart or star click patches the cached row and refetches only the keysets whose membership it moves (Favorites, smart playlists). The write still bumps `library_changed`, so a visible Songs tab follows a click with one keyset query and that re-read.

   Today every click refetches and reconverts the whole library while Songs is visible (`subscribers.rs:23-36`; `favorites.rs:57,84`; `ratings.rs:41,83`).
7. **Details** take their membership ids from SQL and sort them there with their own arm.
   - Membership sources: album, artist and genre through the join tables (`list.rs:64-101`); playlists by `position`; smart playlists through `get_smart_playlist_track_ids`.
   - Hero facts come from SQL aggregates over the same membership, not from folding resident rows: count, running time and dominant genre (`hero_folds`).
   - The detail model is released on close. Today it survives until the tab leave (`callbacks/track_detail.rs:41-61`).
   - Playlist drag-reorder still moves one id in the keyset optimistically (`playlists/detail.rs:289-324`).
8. **The card grids take a keyset too** (amended 2026-10-19 from Most Played alone, Phase 4.5's outcome, built in Phase 5). Most Played takes `(id, play_count)` pairs in `MOST_PLAYED_ORDER` (`ranking.rs:20`); Albums, Artists, Genres, Playlists and Favorite Artists take entity ids in their sort arm's order, filtered against a folded `search_name` per entity table. One outer grid model, `PagedGrid`, builds each chunk's row from the page cache and memoizes chunk `ModelRc`s per generation in a small LRU, so re-reading a chunk returns the same pointer (`slint-pitfalls.md`, the repeater-by-pointer entry). Phase 5 amended both halves while building: Most Played reads back packed ids, and the memo keeps whatever chunks Slint holds rather than an LRU (its amendments 3 and 11).
9. **Bounded lists stay in memory as they are:** Search (50 / 20 caps), Recently Played Songs (200) and Browse (one directory, with disk-only rows that have no id). They keep `row_match` and `track_sort`.
10. **The Songs prewarm stops warming the full row tier.** Today it warms 512 covers on open (measured +3.7 MiB); with paging it warms the first screenful plus the prefetch pages.

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-core/src/entities/track_search.rs` (new, Phase 1) | `SearchFields` (was `row_match::search_fields`) and `pack()` (was `RowSearchKey::from_row`'s packing) |
| `crates/melodia-core/src/entities/track_list.rs` (new, Phase 1) | `TrackListScope`, `TrackSortField`, `TrackOrder`, and `SortDir` moved from the app's settings |
| `crates/melodia-core/src/utils/needle.rs` (new, Phase 1) | `Needle` and `fold_needle`, moved from `row_match` |
| `migrations/20261018000000_track_search_text.sql` (new, Phase 1) | `search_text TEXT NOT NULL DEFAULT ''` and the partial index over unfilled rows |
| `crates/melodia-store/src/database/collation.rs` (new, Phase 1) | `UNICODE_LOWER`; `register` is called on the write, read and test pool options |
| `crates/melodia-store/src/database/queries/track/keyset.rs` (new, Phase 1) | `track_ids` per scope with the sort arm and needle, read back as one packed row (Phase 2), the needle clause, and `detail_order` that `entity_ids` now takes its two orders from |
| `crates/melodia-store/src/database/queries/track/membership.rs` (new, Phase 3) | `Membership`, moved out of `keyset.rs`: which tracks a scope holds, a random cap resolved to its draw's pick, as the `FROM` and `WHERE` both the keysets and the scope facts start from |
| `crates/melodia-store/src/database/queries/track/scope_facts.rs` (new, Phase 3) | `scope_facts` (count, running time, artist and album spread in one aggregate, which `get_favorite_stats` reads too) and `genre_lines` (each genre line with its track count, the album hero's genre chip) |
| `crates/melodia-store/src/database/queries/track/list.rs` | `get_all_tracks_for_list`, the comparator's input, and Browse's fetches. The three per-entity `for_list` fetches went in Phase 3, with `get_playlist_tracks_for_list` and `get_smart_playlist_tracks` |
| `crates/melodia-store/src/database/queries/track/ranking.rs` | `most_played_ids` and `most_played_favorite_ids`, beside `MOST_PLAYED_ORDER` |
| `crates/melodia-store/src/database/queries/track/lookup.rs` | `get_track_list_rows_by_ids` (which also serves the queue sheet, every `QueueRow` field being on `TrackListRow`) and `get_most_played_by_ids`, sharing one in-order helper with the four by-id fetches that each spelled it |
| `crates/melodia-store/src/database/queries/track/search_text.rs` (new, Phase 1) | `backfill_search_text`, paged by id, writing only rows still `''` |
| `crates/melodia-store/src/database/queries/scan/mutations.rs` | writes `search_text` through `DerivedColumns`, which also took over the sort key all three writers computed |
| `crates/melodia-app/src/library/keyset.rs` (new, Phase 1) | the library API over it, `facts` and `genre_lines` included (Phase 3) |
| `crates/melodia-app/src/tasks/search_text_backfill.rs` (new, Phase 1) | the per-launch backfill, spawned beside `retroactive_hash` |
| `crates/melodia-app/src/tasks/queue_window.rs` (new, Phase 4) | the window prefetch, its refresh on a library tick, and running a start a transition owed |
| `crates/melodia-app/src/tasks/heap_trim.rs` | the two boot trims |
| `crates/melodia-engine/src/player/engine/queue.rs` (510) | ids and orders, the play order's ids shared until it moves, and `window_slots` |
| `crates/melodia-engine/src/player/engine/queue_window.rs` (new, Phase 4) | `TrackWindow`. Keeps `handlers.rs` (666) from growing. `backend/mod.rs` (795) is not touched. |
| `crates/melodia-views/src/ui/paged_rows/queue_rows.rs` (new, Phase 4) | `QueueRows` over `QueueRow`: the placeholder, the fetcher (`track_rows::fetch_by_id` over its own conversion), the open's seed and the row shape Up Next shares |
| `crates/melodia-views/src/ui/paged_rows/mod.rs` (new, Phase 2) | `PagedRows<R>`: the model, the row cache, the window and its one fetch, `ScrollPoint`, the selection, `move_row` for a drag (Phase 3), and the `KeysetTickets` that order a list's loads, keep a library change's re-read owed until one shows, and `revoke` every load out when a list is released (Phase 3) |
| `crates/melodia-views/src/ui/paged_rows/track_rows.rs` (new, Phase 2) | `TrackRows` over `TrackListRow`: the placeholder, the fetcher, the seeded `load`, the cover prewarm, and `settle_reload`, the reload tail Songs, Favorites and the details share (Phase 3) |
| `crates/melodia-views/src/ui/track_list_cache.rs` (498), `track_list_model.rs` (127) | deleted (Phase 2) |
| `crates/melodia-views/src/ui/track_detail/cache.rs` (132) | `DetailCache<G>`: the global's weak handle, the entity, the sort and needle a load runs under, the tickets and the scroll point. No rows (Phase 3) |
| `crates/melodia-views/src/ui/track_detail/load.rs` (new, Phase 3) | the list half of every detail's open (`Opening::seat`), reload and requery |
| `crates/melodia-views/src/ui/track_detail/{filter,selection}.rs` | deleted (Phase 3); the details select through `wire_row_selection` like the flat lists |
| `crates/melodia-views/src/ui/queue_sheet/links.rs` (106) | deleted; the album, artist and genre ids arrive with the row fetch |
| `crates/melodia-views/src/ui/{favorites,recently_played}/rows.rs` | each page's models; the one `to_slint_most_played_row` left for `ui::most_played` (Phase 5) |
| `migrations/20261019000000_entity_search_name.sql` (new, Phase 5) | `search_name` on `albums`, `artists`, `genres` and `playlists`, with a partial index over each table's unfilled rows |
| `crates/melodia-core/src/entities/grid_order.rs` (new, Phase 5) | each grid's sort field, as `track_list.rs` holds the tracks' |
| `crates/melodia-store/src/database/queries/entity_keyset.rs` (new, Phase 5) | the five entity keysets per sort arm and needle, read back packed as `track_ids` is |
| `crates/melodia-store/src/database/queries/search_name.rs` (new, Phase 5) | `backfill_search_name` over the four tables, writing only rows still `''` |
| `crates/melodia-store/src/database/queries/{album,artist,genre,playlist}.rs` | each kind's rows by id, in place of `get_all_*`; `playlist.rs` (570) keeps `get_all_playlists` for the two picker dialogs |
| `crates/melodia-app/src/library/keyset.rs` | the entity keysets and rows by id beside the tracks' |
| `crates/melodia-app/src/tasks/search_text_backfill.rs` | fills `search_name` after `search_text` |
| `crates/melodia-views/src/ui/paged_rows/grid.rs` (new, Phase 5) | `PagedGrid<G>` over its chunk row (`GridRow`): the chunk rows over a `PagedRows` store that reports to them (`RowNotify`), the chunk memo and the column count. Kept out of `paged_rows/mod.rs` |
| `crates/melodia-views/src/ui/card_grid.rs` (new, Phase 5) | `CardGrid`, which each of the seven library grids implements, and the loads they share: reload, requery, `refresh_cards`, release and the seeded prewarm |
| `crates/melodia-views/src/ui/most_played.rs` (new, Phase 5) | the Most Played card both curated pages draw, its rows by id and the ids its Play queues |
| `crates/melodia-views/src/ui/{albums,artists,genres,playlists}/{state,grid}.rs` | `GridData`, the sort keys, `GridIndexCache` and the in-memory sort and filter deleted (Phase 5); each `grid.rs` is its kind's `CardGrid`, and its card type's `PagedRow` sits beside its converter |
| `crates/melodia-views/src/ui/grid_rows.rs` | `chunk_rows` and `chunk_entity_rows` deleted (Phase 5); Browse and Radio keep `chunk_built_rows` |
| `crates/melodia-views/src/ui/favorites/grids/{apply,fetch,sort,warm}.rs`, `recently_played/grid/` | deleted (Phase 5); each page keeps its `CardGrid`s and their tab wiring |
| `crates/melodia-views/src/ui/callbacks/card_selection.rs` | `visible_ids` reads the seven library scopes' keysets; `flatten` stays for Radio's three (Phase 5) |
| `crates/melodia-ui/ui/globals/playlists.slint` | `count` in place of the flat `rows` (Phase 5) |

## Every phase

- [ ] Say what keeps working before saying what moved. Describe the change at the call site, not as a diff.
- [ ] `cargo fmt --all --check` and `cargo clippy --all-targets --locked --workspace -- -D warnings` are clean. Tests the phase breaks are fixed in the phase. New tests and doc edits wait for Kenan's go after his manual check.
- [ ] No production Rust file past 800 lines. The closest today are `backend/mod.rs` 795 (untouched), `handlers.rs` 666, `library/playback.rs` 587 and `now_playing/mod.rs` 542.
- [ ] No `unwrap`; logged errors go through `error::describe`; no `Result<_, String>`.
- [ ] A census run (Appendix B) for the states the phase touches, recorded in the phase's `Memory:` line.

## Phase 0: Measurement tooling and the "before" column

Status: done. The 512-track reference moved to the end: it runs once every phase has shipped, before this doc is deleted.

- [x] Land Appendix A as `scripts/heap-census.py`, so the method outlives a scratchpad. A reboot wiped the session copy once already.
- [x] Build the branch's merge base in a detached worktree under a `target/` dir, and run Appendix B on the 51k library.
  - Built: `065275c4` at `target/worktrees/base/target/release/Melodia`, sha256 `43d06cbc9a042e325f00194bc93a291a8707ab9a403dcfb4be5fea2b2c7d01d9`.
  - The re-run landed within about 2 MiB of every figure the original measured (idle 35.3, Songs 59.1, Play all 86.2, queue sheet open 113.2 MiB), each step's growth within 1.4 MiB of the original's, so the Before column keeps the original run. **Its idle is the Synthwave playlist detail, not Settings**, which the tables and Appendix B now say; Settings idle reads 31.2 MiB.
  - The one new figure is the queue sheet closed: closing gives back 16.5 of the 26.6 MiB the sheet took. The 56 B-per-track `QueueRow` model and the row strings go; the 8,523,760 B table and a 24 B-per-track array stay, which is what Phase 4's "closed = Play all" target has to remove.
  - Run on the dev data folder with the pre-migration copy `backups/melodia-v20261014000000.db` (a Phase 1 build had already migrated the live one, which the base build refuses), into a null sink, everything restored after.
- Moved to the end: the same five states on a 512-track data dir, the flatness reference.

## Phase 1: SQL side (no UI change)

Status: done (2026-10-18). Passed by hand; fmt, clippy and the full suite green with the tests below.

- [x] Add the `search_text` migration, the ingest writes and the backfill, which runs every launch off the column itself (decision 3).
- [x] Register the collation on both pools and the test pool.
- [x] Write the keyset queries, plus rows-by-ids for `TrackListRow` (which covers the queue sheet's fields) and Most Played (with `play_count`).
- [x] Tests, all in existing files:
  - `cross_tier.rs`: every sort token × direction on Library, Favorites, the album, artist and genre details and a manual playlist against `track_sort`, over a fixture of the partitions the two could answer differently; the SQL filter against `track_matches` over fourteen needles; and the card fixture gains an accented case pair, so `entity_ids` is held to the collation too.
  - `track_tests.rs`: the backfill (fills `''` rows only, then nothing), an unfilled row matching every needle, rows by id, both Most Played keysets, and a manual and a smart playlist's own order.
  - `scan_tests.rs`: an insert and a re-ingest both write the column. `mod_tests.rs`: the read, write and test pools all order by `to_lowercase()`. `needle_tests.rs`: `is_number`.
  - Checked by mutation: reading the Songs year raw fails Library and Favorites, and an ASCII-only collation fails the pools test, both Songs keysets, the playlist arms and the artist and genre cards. The backfill's race guard (`AND search_text = ''` on the `UPDATE`) has no deterministic test.
- [x] Doc edits: `ui-patterns.md` Filtering and its Settings memo line, `library-data.md` (the keyset beside the retained-row paragraph, `entity_ids` taking `detail_order`, a ninth fts column being three edits, and two new prohibitions: writing a searchable column without `search_text`, and naming `UNICODE_LOWER` in schema), the `row_match` line in `slint-pitfalls.md`, and `needle` in the root `CLAUDE.md`'s `utils/` entry.

Memory: none at runtime. Disk: `search_text` holds 3,889,324 B over 51,012 tracks, 76.2 B per track (`SUM(LENGTH(CAST(search_text AS BLOB)))`, 2026-10-18). The backfill filled all 51,012 in about a second on its first launch.

## Phase 2: `PagedRows` plus Songs and Favorites Songs

Status: done (2026-10-18), `3327af38` and `cfc224e2`. Passed by hand in debug and release; fmt, clippy and the full suite green. Tests and one fix landed after a whole-branch validation.

- [x] `PagedRows<UiTrackListRow>`, installed once per list. `Tracks.rows` and Favorites' rows go through it.
- [x] The Songs and Favorites fetch paths keep the keyset only. Play and Shuffle read it. Each load takes a ticket when it reads its sort and filter, and only the newest is shown, so a keystroke, a header click and a library tick can land in any order.
- [x] Hearts and stars patch the cached row. A Favorites unfavourite drops the ids from that keyset at once. The Now Playing mirror reaches the same patch. The band's artist and album spread comes from `get_favorite_stats`, which retired the Songs fold.
- [x] Favorites ▸ Songs keeps its model across a tab pick; leaving it filtered marks it dirty, the pick clearing the shared needle.
- [x] `install_library_changed_refresher` refetches the keyset, and its stale comment is fixed.
- [x] Deleted `TrackListCache`, `TrackListModel`, their tests, `get_favorite_tracks_for_list` and its two store tests.
- [x] After the first hand test, three fixes and one speed-up:
  - one fetch in flight per list, run on tokio, landing only inside the window (decision 4's amendment), which ended the pool's 2 s waits;
  - a load seeds the rows where the list is scrolled;
  - the row menu's Clear selection shows wherever the list holds a set, the picks on a 51k list sitting far out of view;
  - `track_ids` reads its ids back as one `group_concat` row, ordered by a subquery. sqlx hands each row across from its worker thread on its own, and that hand-off was most of a sort's time. Checked by mutation: dropping the subquery's `ORDER BY` fails all five `cross_tier` keyset tests.
- [x] Tried and dropped: storing lowered `artist`, `album` and `genre` keys, so those sorts compare bytes instead of calling the collation. In release they went from 46–57 ms to 31–41 ms, and in the debug build from 287–353 ms to 73–83 ms. Not worth three permanent columns, their write-through and a backfill, with release already instant.
- [x] Doc edits: `CLAUDE.md`'s "Models from background", `library-data.md`'s narrowing and keyset paragraphs, and `ui-patterns.md`'s paged-list entry, shared-helpers line, Favorites delta and the Clear selection rule.
- [x] After the validation, one fix: a sort or keystroke overtaking a library tick's refresh took the newer ticket, and the refresh's re-read, count and selection prune went with it, leaving edited rows on their old text until the next change. `KeysetTickets` now keeps a refresh owed until one shows, and a requery issued meanwhile runs as one, decided under the same lock that issues its ticket. An overtaken refresh also skips its cover prewarm. Passed by hand.
- [x] Tests:
  - `paged_rows/tests/paged_rows_tests.rs`: the `window` boundaries; one fetch in flight; a landing asks nearest the row read last; a page scrolled past is never fetched; an absent row is not re-asked until a new keyset; a failed fetch waits for the next read; a landing after `clear` stores nothing; an empty list; the flag on a placeholder; the scroll point; the repaints (a patch, an uncached patch, an unchanged and a one-row-changed show, a late fetch against a patch, one and two removals); and the `KeysetTickets` decision table. Selection surviving a swap and a clear was already pinned in `list_selection_tests.rs`.
  - `melodia-core`'s `track_search_tests.rs`: the packed format, four separators for five empty fields, NUL as a space, no match across two fields, and the packed text agreeing with the fields over six partitions.
  - Store: an empty and a one-id keyset; the favourite spread as written-out counts (an untagged track is the unknown artist, so it counts as one).
  - `favorites_tests.rs`: which tab and needle leave Songs owing a reload. `list_selection_tests.rs`: the Clear selection gate.
  - `cross_tier`'s parity fixture gains lengths on and either side of both ends of the Songs clamp: moving the bound passed every test before.
  - Checked by mutation, each failing only its own pins: the in-flight guard, the `absent` filter, the read-stamp arm, the owed-reload settle and takeover, the separator, `DISTINCT`, the menu gate, the Favorites tab check, and both ends of the length clamp. A repaint outside the window is deliberately not pinned: Slint ignores a row it doesn't hold, so it costs time, never a wrong row.

Memory, Songs open as the first view on the 51k library, release, settled 3 minutes (2026-10-18). This is not the Appendix B path (playlist idle, then Songs); [Targets](#targets) is filled by the full comparison once the plan is done:

| Build | Anonymous | Live heap |
|---|---|---|
| `3327af38` | 37.5 MiB | 17.14 MiB |
| `cfc224e2`, two launches | 36.1 and 34.8 MiB | 17.37 and 16.95 MiB |

The keyset is one 409,584 B chunk, 8.0 B per track, and no main-arena size class holds 0.8 × 51,012 live chunks, so nothing is kept per row. Cover pixels were 0.59 MiB, 87 covers. The dropped stored keys left the live heap where it was and moved Anonymous between 40.5 and 50.2 MiB on two launches. A `malloc_trim` handed 11.1 MiB of the 50.2 back, so that spread was the allocator keeping freed pages, not data.

Sorts, release, the `track_ids` median on the 51k dev database (row-per-id at `3327af38`, then packed):

| Column | `3327af38` | `cfc224e2` |
|---|---|---|
| Title asc / desc | 33 / 35 ms | 9.8 / 12.9 ms |
| Artist | 46–49 ms | 28–31 ms |
| Album | 53 ms | 34 ms |
| Genre | 53–57 ms | 31 ms |
| Year | 40–41 ms | 17 ms |
| Length | 39–41 ms | 16.5 ms |
| Track number | 45 ms | 21 ms |

## Phase 3: The four details

Status: implemented, hand-checked and measured (2026-10-18), `00bab609`, its new tests landed and the retimed boot trims measured (2026-10-19). fmt, clippy and the full suite green; four fixes landed since that commit await their hand check, so the phase stays open.

- [x] Each detail lists its entity as a `TrackRows` installed once per global. `DetailCache` keeps no rows: the entity, the sort and needle its loads run under, its `KeysetTickets` and the scroll point. `seat_unfiltered`, both row `Vec`s, the selection shadow and the `VecModel` went.
- [x] `open_*_with` fetches the ids under the view's `resolve_view_sort` arm through `track_detail::load::open`, and its closure seats them (`Opening::seat`). `cross_tier`'s `DETAIL_DEFAULTS` still reads those literals in each `detail.rs`.
  - An open overtaken by a load asked for on the page being left asks again under its own query; where a library change overtook it, that requery runs as the reload it owes.
  - A header click and a keystroke requery in SQL; a library tick, a playlist edit and a play-count flush reload, re-reading the rows the list holds and, unfiltered, pruning the selection (`track_rows::settle_reload`, now shared with both Songs tabs).
- [x] Hero facts come from SQL: `scope_facts` gives a smart playlist its count and running time and a genre or playlist its artist and album spread; `genre_lines` feeds `dominant_genre`, now a tally over weighted lines. `get_favorite_stats` reads its four numbers through `scope_facts` too.
- [x] The detail model is released on close (`forget_closed`) and on a section leave, both through `track_detail::release_rows`, which also revokes every load still in flight. That revoke is the gate rather than a section-active check: a cross-tab drill seats its list before its section turns active.
- [x] Playlist drag-reorder moves one id in the keyset (`PagedRows::move_row`), and the write's return, Ok or Err, requeries. The rollback is asking SQL what order it holds, which also corrects a refresh that landed mid-write and showed the order from before the drop. `position_order`, `rollback_reorder` and `PlaylistDetailState` went.
- [x] Selection goes through the flat lists' `wire_row_selection`, which already answers a `TrackRows` without fetching a row. `DetailRows` went: hearts and stars, the Now Playing mirror included, patch through `track_detail::patch_row`.
- [x] Deleted as dead once the details stopped reading them: `get_tracks_by_{album,artist,genre}_for_list`, `get_playlist_tracks_for_list`, `get_smart_playlist_tracks`, their five library wrappers, and `model_diff::permute_rows_by_id`. `hero_folds::fold_tracks` stays for Recently Played's capped Songs tab.
- [x] Two visible differences, both deliberate:
  - two genres that each hold an album's majority now resolve to the first in the commonest line's tag order, where an open and a refresh could disagree before;
  - a refresh under a live filter no longer drops picks that left the entity, only an unfiltered one does, as on the Songs tabs.
- [x] Tests the phase broke, fixed in the phase:
  - `cross_tier`: the card tests read the page's rows by id over its own keyset, still sorted by the comparator; the detail parity test takes each page's rows off the whole table.
  - `stats_tests`: the genre-page and guest-credit pins read the keyset, which is now the page.
  - `smart_playlist_tests`: `resolve` stores the criteria as a playlist and reads its keyset, so the evaluator tests go through the page's own path, and the id projection is held to the page.
  - `track_tests`: the three `for_list` order tests went with their functions.
  - App `playlists_tests` and `playlist_files_tests` read titles through the export query and ids through the keyset.
  - `hero_folds_tests` take weighted lines with the same expectations; `playlists_tests`' reorder-guard pin splits at the new end of the guard.
- [x] Doc edits: `CLAUDE.md`'s "Models from background", `ui-patterns.md` (the paged-list, details, selection and hero-facts entries and the shared-helpers line) and `library-data.md` (the keyset paragraph, the card-order paragraph that replaced the `for_list` trap, and the ratings line).
- [x] Hand check, passed by Kenan: every header sort and the playlist's return to its own order, the filter and the artist strip, every selection gesture, Play-row and Shuffle, hearts and stars from a row and the bar, drag-reorder, remove from playlist, a tag edit while open, close and reopen, a tab leave and return, a relaunch onto a persisted detail, a row menu's Go to, and Mouse-4/5, on an album, an artist, a genre and both a manual and a smart playlist. A smart playlist refusing a drag is the `!PlaylistDetail.playlist.is_smart` term `reorder-enabled` has carried since smart playlists landed, not a regression.
- [x] After the validation of `00bab609`, two fixes. Hand check pending.
  - Removing tracks from a filtered playlist left them in the selection, out of sight, for a Ctrl-click to carry into the next batch action: the reload under a filter prunes nothing. The removal now drops its rows and their picks on the tick the write lands (`track_detail::remove_rows`), through the helper Favorites' unfavourite already used, now shared as `track_rows::remove_rows`. A library tick under a filter still prunes nothing, as above.
  - The drag guard read the sort and filter the page asked for, which a requery shows a query later, so a drag released in between wrote the old list's indices as positions. `DetailCache` now records the entity whose own order is on show, unfiltered, and the guard asks it too (`shows_own_order`), which also covers an overtaken open leaving another playlist's rows up under the new id.
- [x] Two more fixes (2026-10-19), `29eeaaed` and `1140fb29`, each with its tests. Hand check pending.
  - A smart playlist capped in Random order drew a new pick on every query its detail ran, so a sort, a keystroke and the hero each described different tracks. The page draws once per open and carries the `Draw` in `TrackListScope::Playlist`, and `Membership` resolves the cap through `pick_random`, so a library tick moves the pick only by a track entering or leaving the rules. `PagedRows::move_row` also refuses a drop at the list's length, as the reorder write already did.
  - A smart playlist capped inside a run of equal values kept whichever tied tracks the query plan reached first, so a new index or a statistics refresh could change what it held. Every cap order now ends in the id, in the direction its plan already broke ties in, and `get_recently_played` ends in `id DESC`, the order its partial index hands back.
- [x] New tests (2026-10-19):
  - `paged_rows_tests.rs`: where `move_row` lands a row, that it re-reads only that row, and a source or drop outside the list and an empty list moving nothing; `revoke` refusing the newest load, leaving no reload owed and letting a later load show; and a requery over a stand-in still owed running as the reload, the ticket half of an overtaken open.
  - `track_detail/tests/cache_tests.rs` (new): `shows_own_order` holds only for the open playlist shown by position, ascending and unfiltered, and not for a descending, title or filtered list, a page that asked for its own order before a load showed it, the playlist left on screen under an overtaken open, or a released list.
  - `list_selection_tests.rs`: `track_rows::remove_rows` under a filter drops the removed rows' picks and keeps a pick the filter hides. `playlists_tests.rs`: the drag guard asks `shows_own_order`, and the removal calls `remove_rows` past the write's early return.
  - `hero_folds_tests.rs`: a weighted tie goes to the first name in the commonest line.
  - Store `scope_facts_tests.rs` (new): a genre counting a track tagged with it second, a playlist counting only its own items, a capped smart playlist counted under its cap, an empty and a deleted playlist stating nothing; `genre_lines` grouping per rendered line, commonest first and ties by lowest id.
  - Checked by mutation, each failing only its own pins (SQL the keysets share failing their pins too): the move's bounds, its notification and where it counts `to`, both halves of `revoke`, the stand-in carrying the owed reload, `shows_own_order`'s three terms, its id check and `forget_shown`, the guard's term, the removal's call and its selection half, the commonest line settling a tie, the deleted-playlist lookup, both `genre_lines` order keys, the playlist filter and the smart cap. Dropping the `COALESCE` in `scope_facts` fails nothing, sqlx decoding a NULL sum as 0, so the empty test rests on the deleted playlist.
  - `Opening::seat`'s overtaken branch has no unit seam, no views test building a window or an `AppState`, so the hand check holds it.

Memory, 51k dev library, release, one launch per build on the dev folder with a 25-track queue, everything restored after (2026-10-18). Before is `main` at `065275c4` (the Phase 0 binary, sha256 `43d06cbc…`, on the pre-migration copy of the database); after is this branch's working tree (sha256 `46b3193b…`). The smart playlist is a seeded "Whole library" with empty rules, matching all 51,012 tracks. MiB:

| Step | Before: Anonymous | Before: live heap | After: Anonymous | After: live heap |
|---|---|---|---|---|
| Idle, Synthwave playlist open | 36.6 | 16.07 | 41.0 | 16.40 |
| Zydeco genre open (1,076 tracks) | 37.5 | 18.41 | 40.1 | 17.11 |
| Zydeco closed, Genres grid | 35.7 | 17.06 | 37.5 | 16.33 |
| Whole library open (51,012 tracks) | 100.8 | 82.22 | 40.1 | 18.77 |
| Whole library closed, Playlists grid | 82.6 | 54.60 | 39.6 | 18.58 |

- **Compare on live heap.** The after run was given one `malloc_trim(0)` by hand after its idle reading, which took Anonymous from 41.0 to 37.7 and left the live heap where it was, so its later Anonymous figures start 3.3 MiB lower than they would have.
- **Idle is level in live data** (+0.33 MiB). The Anonymous gap is freed memory: after the trim the after run still held 5.07 MiB of sub-page fragments against before's 1.53, which no trim returns. Its idle was read once the pool had reaped the connection the Synthwave open made, that open having come after the settle.
- **Whole library:** before held three arrays sized to the library (339.2, 264.0 and 88.0 B per track, 35.3 MB) plus about eleven owned strings per track, 513 row covers (3.46 MiB) and a 301.8 MB peak RSS; after holds one 409,584 B keyset (8.0 B per track) and 105 row covers (0.77 MiB). Closing it frees the keyset; before kept all three arrays after the close, `release_rows` emptying its two `Vec`s without freeing them and the Slint model living until the tab leave.
- **A detail's own cost:** Zydeco added 2.34 MiB of live heap before and 0.71 after; closing it returned after to idle, where before kept 0.99 MiB until the tab leave. Most of the rise to Whole library closed is the Playlists grid's own covers (1.43 MiB of pixels against Genres' 0.33).
- **After's peak RSS was 452 MB at boot,** file pages, not heap: each pooled connection that runs one of the boot's full scans of `tracks` maps nearly the whole 66 MiB file, and RSS counts every mapping (PSS 61 MiB, the file once). Not a regression: before on the same data mapped 244 MiB across four connections against after's 250 across five. Before's run read less only because its older database copy re-ingested at boot, SQLite serving those pages from the WAL.
- **The mid-sized chunks after's census listed and before's didn't are in both builds:** two are `notify`'s inotify maps (6,318 path entries each, one per watched directory) and the third (307,184 B, holding only "Invalid context") is most likely the NVIDIA EGL driver's. The census lists mmapped chunks only, and glibc places a block of that size in the arena or in its own mapping by what the heap looks like at the time.
- **Freed-but-resident at idle was 2.84 MiB trimmable**, past this plan's 2 MiB threshold for adding a trim (Checked and left alone), the one boot trim at 5 s running before the folder rescan and the 120 s pool reap. `tasks::heap_trim` now trims at 10 s and 15 s after the pool's boot connections close (135 s).
- **The retimed trims leave nothing trimmable at idle** (2026-10-19). Release at `a5d201e5` (sha256 `529ded7f…`), launched onto a restored Synthwave detail so its open came before both trims, a 203-track queue, a null sink, state restored after. Sampled once a second from the process's start:
  - the 10 s trim returned 2.41 MiB of Anonymous (41.21 to 38.80);
  - the pool's reap at 120 s took RSS from 367.8 to 175.4 MB and Anonymous from 38.99 to 38.87;
  - the 135 s trim returned 0.84 MiB (38.87 to 38.03).

  The census just past 165 s read 0.00 MiB trimmable, Anonymous 38.0, live heap 16.12 and 6.77 MiB of sub-page fragments (Phase 3's after run: 2.84 trimmable, 41.0 untrimmed, 16.40 live, 5.07 fragments after the hand trim). Pixels were the Synthwave hero, 22 row covers and one 64² proxy, 0.59 MiB. Peak RSS was 453.3 MB, the boot's file pages (Phase 3.5).

## Phase 3.5 (open to discuss): index the boot's whole-table reads

Status: not decided. Kenan investigates and discusses this before anything here is implemented.

**What you see today.** For the first two minutes after launch RSS sits around 370 to 430 MB on the 51k library, in both builds (Phase 3's Memory). It is file pages, not heap: every pooled connection that runs one of the boot's full scans of `tracks` (34.3 MiB) maps nearly the whole 66 MiB database, and RSS counts each mapping. They go when the pool closes the boot's connections at the 120 s reap.

**What would change.** Two partial indexes, so the per-launch checks read an index instead of the table:
- `tracks(artwork_path) WHERE artwork_path IS NOT NULL AND artwork_path <> ''` for the artwork reference set (`queries::artwork::REFERENCED_PATHS` and its library arms), which the boot reads four times: the sweep, the renormalize pass, and the scan repair once per folder.
- `tracks(id) WHERE file_hash IS NULL` for `retroactive_hash`'s per-launch check, the shape `search_text`'s backfill already uses.

No query or Rust code changes. On a copy of the dev database (2026-10-18) both plans switched: the reference set to a covering scan of a 3.66 MiB index, the hash check to an empty partial index. What still reads the whole table is the folder rescan (one connection) and a smart playlist's recount where its rule can't use an index.

**No new migration.** Both indexes go into this branch's own `20261018000000_track_search_text.sql`, which no release has shipped. An existing dev database has already applied it, so sqlx's checksum check refuses the edited file there: reset `Melodia-dev` from `backups/melodia-v20261014000000.db` (or another pre-migration copy) before the first launch, as Phase 1's first run did.

**Estimates, to be replaced by measurement:** boot database mappings from about 250 MiB to about 60 to 120 MiB, so boot RSS around 190 to 300 MB instead of 370 to 430, for those two minutes. Anonymous and live heap unchanged. The four reference-set scans (22 to 24 ms each in the boot log) and the hash check (6.5 ms) become index reads.

**Costs:**
- Disk: +3.66 MiB for the artwork index, which stays for as long as the index exists and grows with the number of tracks holding a cover. The hash index stays at one page once the library is hashed.
- Writes: every insert or update of a track's cover also updates the artwork index; it matters mostly on a large first import.

**Measured when implemented**, on the same data before and after: the boot sampling Phase 3's investigation used (database mappings, RSS and PSS at 3, 8, 30 and 125 s), the boot statements' timings from the sqlx log, Anonymous at idle, and the time a full rescan of the 50.5k folder takes. **Dropped if any of those regress.**

## Phase 4: Queue as ids, plus the queue sheet and Up Next

One commit, because the engine, the app and the two consumers only validate together.

Status: done (2026-10-19), `64a073d0` and the fix `ff51b78c`. Hand-checked, documented and measured before the fix, which the tests below pin; fmt, clippy and the full suite green (4,166 tests).

Decided with Kenan on 2026-10-19, before the work:
1. **Picks are by track.** The sheet selects through `PagedRows`' own id set and the track lists' `compute_click_selection`, so a track queued twice is picked, counted and removed as both rows. A rebuild already re-mapped the old per-row bits by id, so this was the behaviour after any queue change.
2. **At a gapless handover the queue wins.** Where the queue's next moved after the stage (the last `PRELOAD_LEAD_MS`), the handover starts the queue's track, a cut at most a poll into the staged one, instead of seating the staged one. This resolves the staged-gapless risk below.
3. **A restart keeps repeats.** The restore drops only ids whose rows are gone, through `prune_missing`, which also remaps `current_index`, and an index past the end now seats nothing.

- [x] Engine changes:
  - `QueueState` holds ids, its vectors crate-private; the play order's ids are an `Arc<[i64]>` built on first ask after the order moves, so an advance republishes the same allocation;
  - `TrackWindow` on `PlayerState`, filled against `QueueState::window_ids`, the slots `window_slots` names (one before the current, it, three after, wrapping where repeat wraps);
  - every transition starts a queued track through `PlayerState::start_queued`, which defers on a miss (decision 5); the crossfade builder reads its next's record before it advances and bails without one;
  - `evaluate_playing_tick` and `next_needs_reopen` read the next's record from the window, and `PlayingTick.late_preload` carries the record the monitor keeps as `staged`;
  - `advance_gapless` seats `staged` where the advanced slot holds it and starts the queue's track otherwise;
  - `any_tracked` and `sync_track_summaries` gave way to `window_wanted` and `seat_window_records`.
- [x] App changes:
  - `player_play_tracks`, `queue_add_tracks` and `play_next_many` validate ids through `library::keyset::existing_in_order` instead of fetching whole records; `open_as_queue` and the import append pass ids;
  - `player_play_tracks` fetches the window its start opens, `queue_skip_to_index` (now `async`) the target's record, and the prune its successor's, each ahead of the emit;
  - `plan_restore` builds the `QueueState`, prunes the gone ids and fetches its window's records;
  - `tasks::queue_window` (decision 5) is spawned beside `queue_prune`.
- [x] Views:
  - the sheet is a `QueueRows` over the queue's ids. `ShadowEntry`, the index anchor, `links.rs`'s `LinkCache` and `RowLinks::stamp` went; the rows carry their album, artist and genre ids from the projection;
  - a drag moves the row in the model first, so the publish that follows repaints nothing;
  - the favourite toggle patches the cached rows on the UI thread; the worker-side write into `st.queue.tracks` went;
  - Up Next stays a 20-row `VecModel` (decision 9) and fetches its slice by id, showing it and playing the slide in the tick the fetch lands, with a ticket dropping an overtaken fetch; it reads again on a library tick;
  - `latest_qvm` and `bridge::to_slint_queue_vm` read the ids.
- [x] The 350 ms close teardown and its `malloc_trim` stay; the close clears the rows and keeps the picks.
- [x] Tests the phase broke, fixed in it: the engine's queue, state, actions and handlers suites queue ids and seat their records (`queue_up`); `sync_track_summaries`' test went to `seat_window_records`; the restore tests go through `QueueState::restored`, and the out-of-range index test now expects no current slot; the app's queue, playback and prune suites read `play_ids`; `RestorePlan` carries the queue; the sheet's `same_sources` tests went with it, and its row-shape tests moved to `paged_rows/tests/queue_rows_tests.rs`; Up Next's `outgoing_row` tests read the rows on show.
- [x] Hand check, passed by Kenan in the dev data folder: Play all at 51k; Next and Previous at both ends under each repeat mode; shuffle on and off; Play Next and Add to Queue from a row and a card, a track queued twice; in the sheet, remove one, remove the picks (both copies together), drag, clear and a far double-click; an Up Next click; an album gapless and a crossfade; Play Next in the last 1.5 s (the queue's track plays); the end-of-track sleep timer; an upcoming track favourited or rated from Songs showing it once playing; a retag of the playing track and of one in Up Next; deleting the playing file; a station over the queue, then stop; a restart with a repeat queued and a missing row before the current one; five unplayable files in a row, and everything unplayable under repeat-all; the sheet's first frames, covers, Go to and Copy; the miniplayer's Up Next; MPRIS Next and Previous.
- [x] After the hand check, asked by Kenan: a queue row marks its track as a track row does, the heart up for as long as it is a favourite and the star strip on hover. The title, heart and strip moved out of `track-list-row.slint` into `components/track-list/track-title.slint`'s `TrackTitle`, which both rows mount; `QueueRow` gained `rating` and `Queue` a `set-row-rating`, and the queue's heart and stars write through `wire_row_flag!`, the track lists' path, in place of its hand-rolled favourite. Passed by hand.
- [x] Census (Appendix B, states 1 to 5), below. By code reading the queue costs 32 B per track, three vectors and the shared ids, about 1.6 MiB at 51k; Play all measured 1.73.
- [x] New tests (2026-10-19):
  - Engine `queue_tests.rs`: `window_slots` on and either side of both ends of the window and of the queue, wrapping and not, with nothing current, over one track, a queue shorter than the window and an empty one; `window_ids` reaching past either end only under a repeat, repeat-one included, naming a track queued twice once and following the play order; every order mutator publishing its new order through the ids cache, and the index-only moves republishing the allocation already shared.
  - Engine `state_tests.rs`: a Next onto a track the window lacks stopping the deck, owing the start and leaving nothing on the deck under `Loading`; Next, Previous, an end of stream, Play with nothing on the deck and a stale gapless handover each owing the start of the track it moved onto; the start running once the record lands, a paused skip coming back paused, and the start refused after a Stop, a Next that started its track and a station connecting; a remove above, a shuffle, a prune above and a prune of the awaited track each moving what starts, a cleared queue stopping, and a record that never lands owing nothing again; the crossfade dropped without advancing; the handover onto the queue's next starting nothing and seating the staged record, the queue winning over a stale stage, and a handover past the end stopping; `window_wanted` asking a queue move for only what the window lacks and a library tick for everything it holds plus the playing track a cleared queue no longer names; a record moved only in its saved position not republished, and a retag that moved it too reaching Now Playing.
  - Engine `handlers_tests.rs`: a next the window lacks neither staged nor faded into. `actions_tests.rs`: an owed start kept on its cell with nobody subscribed; a deferred start reaching the backend once its record lands, and one still waiting publishing nothing; the skip count carrying across a deferral, the lap stopping on the fourth of four unplayable tracks where a fresh count runs on to the second.
  - App: `playback_tests.rs`, Play queueing each listed track with a row once, in list order; `queue_tests.rs`, a restore keeping a track queued twice and staying on the copy it was left on past a gone row; `tasks/tests/queue_window_tests.rs` (new), a queue move fetching the window and no further, and only a library tick reading a held record again.
  - Views `queue_sheet/tests/picks_tests.rs` (new): picking a track queued twice marks both of its rows, and removing the picks takes both.
  - One production change, for a seam: the handover's decision moved out of the monitor's closure in `advance_gapless` into `PlayerState::build_gapless_advance_actions`, unchanged, the claim staying in the monitor.
  - Checked by mutation, each failing only its own pins: the window's edges, wrap, empty-queue guard and no-current origin; `window_ids`' dedup, repeat and play-order lookup; the cache reset in `order_changed` and in `unshuffle`, and a reset added to `advance`; `await_record`'s `Stop`, position, re-pause and skip count; `still_awaits`' source and empty-queue terms; `resume_awaited`'s guard and its current-slot id; `start_awaited`'s pre-check, its run and its carried count; the crossfade advancing before its bail; the handover's id guard, seat arm, duration and stop; the tick's eligibility asking the queue rather than the window; both `window_wanted` arms; `differs_beyond_position` both ways; `existing_in_order`'s dedup; the restore folding repeats and skipping its remap; the fill re-reading on a queue move and dropping what it fetched; `picked_positions` keeping one row. Shared lines fail every pin reading them: `send_replace` swapped for `send` also fails the three tests finding their owed start on the cell, and a pick flag read by position the nine `list_selection_tests` and the paged-list one. Two had no pin before: removing the cache reset from `order_changed` failed only the new mutator test, and dropping `existing_in_order`'s dedup only the new Play test.
  - No seam: the window task's loop, its `AppState` unbuildable in a test; the fill and `start_awaited` are each pinned, and the hand check holds the three lines joining them. `apply_picks`' count of rows against its ids once each needs an `AppWindow`, which no views test builds.
- [x] Doc edits: root `CLAUDE.md`'s "Models from background"; `ui-patterns.md` (the paged-list entry and the sheet's picks, a `TrackTitle` entry, the row menu's "Go to" hosts, the sheet's open under `covers-generation`); `audio-stack.md` (the queue as ids and its window, the miss, the queue winning a stale handover, `Loading`'s second use, the gapless stage and the crossfade builder reading the window); `library-data.md` (the `TrackSummary` and `TrackListRow` projections, the queue beside the keyset paragraph, the rating strip's shared cell).

Memory, 51k dev library, release, one launch per build on the dev folder with the 25-track queue and a null sink, everything restored after (2026-10-19). Claude launched both builds at Kenan's ask and Kenan clicked. Before is `main` at `065275c4` (the Phase 0 binary, sha256 `43d06cbc…`, on the pre-migration copy of the database); after is this branch's working tree (sha256 `b579b15b…`). MiB:

| State | Before: Anonymous | Before: live heap | After: Anonymous | After: live heap |
|---|---|---|---|---|
| 1 Idle, Synthwave playlist open, 170 s | 55.9 | 16.22 | 32.9 | 15.55 |
| 2 Songs open | 77.5 | 38.63 | 40.6 | 17.91 |
| 3 Play all | 91.3 | 70.96 | 41.7 | 19.64 |
| 4 Queue sheet open | 114.1 | 97.67 | 43.0 | 21.20 |
| 5 Queue sheet closed | 97.6 | 80.33 | 37.7 | 19.74 |

- **Compare on live heap.** Each step's cost, before against after: Songs 22.41 against 2.36, Play all 32.33 against 1.73, opening the sheet 26.71 against 1.56. The closed sheet keeps 9.37 over Play all before and 0.10 after.
- **Anonymous follows the trims, not the data.** Trimmable pages per state, before / after: idle 17.09 / 0.00, Songs 14.44 / 6.24, Play all 4.48 / 5.60, sheet open 0.46 / 5.46, sheet closed 0.54 / 0.76. Before's one boot trim at 5 s runs ahead of the rescan and the pool reap; after's retimed trims leave idle clean and the sheet's close trim takes the rest. Before's sheet open (114.1) is within 0.3 of Session B's.
- **Songs' row covers:** before 513 at 48² (3.40 MiB), the prewarm filling its cap; after 87 (0.58 MiB). The sheet decoded none, its first rows being Songs' first rows.
- **Opening the sheet holds nothing per queued track.** A heaptrack run of the same binary, the sheet left open and quit through the tray, replayed from just before the open to 12 s into it: +1.33 MiB live, FemtoVG's render target, textures and output 0.60, Slint instantiating and binding the rows on screen about 0.51, the accessibility tree 0.16, the `QueueRows` store 0.016. That is the 0.3 MiB over target. The census's 48 B class (49,839 chunks at sheet open) had sat just under its 0.8-per-track listing floor; the open's ~9,000 dependency nodes, 32 B requests in 48 B chunks, pushed it over and the close took them back.
- **Peak RSS is the boot's file pages** (Phase 3.5): 440.6 MB on the census run against before's 336.8. A second launch of after with `MELODIA_RSS_SAMPLE=1` read 447.0 MB at 1.5 s, 389.0 MB of it file pages and 38.1 MiB anonymous, held until the reap at 121 s took RSS to 186.7 MB and the file pages to 132.7 MB with the anonymous part unchanged. VmHWM was 447.8 MB.
- **RSS after the boot connections close:** 184.1 MB at idle; 201.8 MB at state 5, 144.2 MB of it file pages (before: 299.2).

## Phase 4.1 (dropped): Hand back the cover decode's scratch once it goes quiet

Status: dropped (2026-10-19), after a second replay of Phase 4's heaptrack recording. Nothing was implemented.

**What it was for.** Opening Songs on the 51k library leaves 6.24 MiB of freed heap resident (Phase 4's census) until something trims. The plan was a fourth `event_bridge` producer reporting each cover-decode burst, and one trim in `tasks::heap_trim` once the bursts went quiet.

**Why it went.**
- The memory left resident is five blocks of 128 KiB or more that the Songs burst freed: 3.39 MiB from `jpeg_decoder` (three progressive coefficient planes), 2.32 MiB from `image`'s full decode and 0.25 MiB from `png`. It is not the blocks under the mmap threshold this phase first named. glibc maps a request over `M_MMAP_THRESHOLD` only when no free chunk fits it (`mallopt(3)`), so these land in free chunks whose pages the boot trims handed back, fault them in, and leave them resident when freed. What a burst leaves is bounded by the free space the arenas already hold.
- Blocks that size come only from covers over 512 px. `jpeg_decoder` holds a progressive JPEG's coefficients at full resolution whatever the decode scale, and a PNG decodes whole: a 1.13 MiB plane needs a source about 770 px square, the 2.32 MiB decode one about 900 px square. The dev library's store holds 1,099 such covers of 4,943, all synthetic. The 189 covers behind the `~/Music` tracks are 188 baseline JPEGs and one PNG, and the row tier decodes a baseline JPEG at 1/8 scale.
- So the 6.24 MiB is the dev library's. On real covers a burst is expected to leave under the 2 MiB this plan sets for adding a trim (Checked and left alone), by code reading.
- To confirm: the 512-track reference (Appendix B step 10) records trimmable at Songs and Play all. Above 2 MiB there, the trim comes back with the mechanism above.

## Phase 4.5 (done): the card grids and Browse

Status: done (2026-10-19), a decision rather than a build. Every library grid takes the paged model, built in Phase 5: Albums, Artists, Genres and Playlists, Favorites' Favorite Artists, and both Most Played grids, with `Playlists.rows` becoming a count. Kenan decided on the estimate in Phase 5 rather than a census first; the census runs with the phase. Browse and Radio's station grids stay as they are.

**The question:** Phase 5 gives the two Most Played grids a keyset and a memoized chunk model (decision 8), because they rank tracks and so grow with the library. Should every surface that draws a grid take that model, or only the ones that can grow that large? The answer decides whether Phase 5 builds it generic or for Most Played alone.

- **The surfaces to decide on:**
  - My Library's four grid tabs: Albums, Artists, Genres and Playlists. They grow with the entity count, not the track count: 4,806 albums, 3,098 artists and 497 genres on the 51k dev library (Evidence). Playlists are the user's own.
  - Favorites' Favorite Artists grid.
  - Browse in card mode, and Browse's track list (below).
  - Radio's station grids, likely out of scope: its Browse tab appends pages from a remote directory whose rows have no local id, and its kept tabs hold the user's own stations.
- **Measure first.** No census so far has had a grid tab open. Run Appendix B's census with Albums open, then Artists, against the idle state. The grid cover tier is already bounded by the display (Checked and left alone), so what a run measures is the entity rows, the chunk models and their strings.
- **What paging a grid would take**, by code reading:
  - entity keysets sorted in SQL, the name sorts through `UNICODE_LOWER`;
  - a filter in SQL. The grids match raw fields today (`ui-patterns.md` Filtering), so an accent-folded filter needs a stored folded column per entity table, a migration each, as `search_text` was for tracks;
  - decision 8's chunk model, which returns the same `ModelRc` for the same chunk, so a column-count change becomes index arithmetic over the ids;
  - reworking what reads the whole list today: the first-screenful cover prewarm, the grid signature, the lone-row card count, and card selection over cards not fetched yet.
- **Browse is the odd one out** (decision 9). It shows one directory at a time, and its disk-only rows have no track id, so a keyset of track ids can't hold them; a paged Browse would key by path. Its list and its cards are bounded by the largest directory, so the question is whether a library keeps tens of thousands of files in one folder.
  - **Decided (2026-10-19): Browse stays as it is**, for the two reasons above. The two move together, the cards being the list's own rows (`BrowseCardGrid` reads the list's selection), so paging it would take one path-keyed paged source behind both, never the grid alone.
- **The outcome:** for each surface, keep it in memory as it is, or page it in a phase of its own.

Memory: estimated in Phase 5 (2026-10-19), measured there.

## Phase 5: The card grids and Most Played

Status: built (2026-10-19), hand check, tests and docs done. The census waits for Kenan's go. The scope is Phase 4.5's outcome.

**What you see today.** Opening a card grid loads every entity it can show twice: as the stats rows its filter and sort walk in memory, and again as Slint cards. A section leave hands both back, so idle doesn't move, but each grid's peak grows with the library. Estimated on the dev library (2026-10-19), row data only, covers unchanged:

| Grid | Rows | Held while open | Held paged | Saved |
|---|---|---|---|---|
| Albums | 4,841 | 4.17 MiB | 0.18 MiB | 4.0 MiB |
| Artists | 3,116 | 1.18 MiB | 0.09 MiB | 1.1 MiB |
| Genres | 171 | 52 KiB | 29 KiB | 23 KiB |
| Playlists | 7 | 6 KiB | 2 KiB | 4 KiB |
| Favorite Artists | 15 | 7 KiB | 4 KiB | 2 KiB |
| Most Played, per 1,000 ranked tracks | | 690 KiB | 153 KiB | 0.5 MiB |

- **An estimate, to be replaced by the census below.** The dev database's own rows, read-only, times what the code allocates per row: the struct layouts, Slint's 24 B string header plus its NUL, and glibc's chunk rounding. At 6 columns; 4 to 9 moves it about 2%. The paged column assumes up to 512 cached cards, as `PagedRows` keeps.
- **Only one grid is open at a time**, so the savings don't add up. The worst case today is Albums.
- **Most Played grows with listening, not with the library.** The dev library ranks 21 tracks, so its two grids save nothing there yet.

**Why every grid and not only the large ones** (Kenan's call, 2026-10-19):
1. **Genres.** Size alone doesn't justify it (171 today, 497 in Evidence). The reason is structure: `genres/grid.rs` and `genres/state.rs` hold the same in-memory grid code as Albums, Artists and Playlists. Left out, all of it stays alive for one tab beside the paged path; brought in, it is deleted. The cost is one more folded column in the migration.
2. **Favorite Artists** (`favorites/grids/`, `get_favorite_artists`). It lists every artist with at least one favorite track, so it grows with favorites, up to the full artist count (3,116 today). It shares the Favorites page and `EntityStripRow` with the Most Played grid this phase pages; left out, it would be the one grid there still holding everything.
3. **The Add to Playlist list** (`Playlists.rows`, `globals/playlists.slint:10`). A second full copy of every playlist, rebuilt beside the grid (`playlists/mod.rs:237`, `playlists/grid.rs:162`), so paging the grid without touching it would leave every playlist in memory anyway. **It becomes a count** (amended 2026-10-19 from reading the paged source or fetching on open): the list is read for its length alone, by the row menu's Add to Playlist gate (`track-context-menu.slint:147-150` and `:202`), and the picker dialog fetches its own list (`add_picker.rs`). A Playlists fetch is the only thing that fills it, and that runs only while the tab is active, so by code reading the entry is missing from every row menu until the Playlists tab has been opened that session. The count fixes that too.

**Stays as it is:** Artist Detail's albums strip (one artist's albums), Search's strips (capped), Radio's station grids (no local id; the kept tabs are the user's own) and Browse (one directory, decision 9 and Phase 4.5).

- [x] **One migration, `20261019000000_entity_search_name.sql`:** `search_name TEXT NOT NULL DEFAULT ''` on `albums`, `artists`, `genres` and `playlists`, each with a partial index over its unfilled rows. It holds the folded name, packed as `search_text` is (decision 3); an artist's carries its sort name after it. Every statement writing one of those names writes it beside (the scan's upserts, a playlist's create, rename and import), and `search_text_backfill` fills the `''` rows of all four too, decision 3's shape.
- [x] **Filters in SQL** over those columns, matching what each grid matches today:
  - Albums: its own name, its artist's through the join (so a renamed artist reaches its albums with no write), or the year as a number (`albums/grid.rs:120-122`);
  - Artists: the name or the sort name. Genres, Playlists and Favorite Artists: the name.
  - One visible difference, deliberate: Favorite Artists also matches an artist's sort name, the column carrying both, as the Artists grid already does.
- [x] **Keysets in SQL**, each arm today's sort with `UNICODE_LOWER` where it lowercases and the id last (decision 2), descending reversing the whole order as today:
  - Albums: name; `COALESCE(year, 0)` then name; artist then name.
  - Artists: name; track count then name; album count then name.
  - Genres: name; track count then name; duration then name.
  - Playlists: updated; name; track count then name. **A smart playlist's stored count is 0** (the junction triggers'), so the track-count arm takes the ids and counts SQL holds plus `smart_playlists::recount`'s live counts, sorted over the user's own playlists in the library layer.
  - Favorite Artists: favorite count then name; name.
  - Most Played: Phase 1's `most_played_ids` and `most_played_favorite_ids`.
- [x] **Rows by id** for each kind, in the order asked through the in-order helper `lookup.rs` shares; a page's smart playlists are recounted as they land. `get_all_{albums,artists,genres}` and the whole-list `get_favorite_artists` go; `get_all_playlists` stays for the export and add-to-playlist pickers, which read it on open.
- [x] **`PagedGrid<R, G>`** (`paged_rows/grid.rs`, decision 8), installed once per grid global: `ceil(n / columns)` chunk rows over a `PagedRows<R>` store, so a column change is index arithmetic and keeps the cache. A chunk's `ModelRc` is memoized per keyset generation in a small LRU and patched in place as its cards land, so the repeater keeps its pointer; an unfetched card is a placeholder carrying its id. Each card type (`AlbumRow`, `ArtistRow`, `GenreRow`, `PlaylistRow`, `EntityStripRow`) implements `PagedRow` beside its converter, as `ui-patterns.md` asks of a list over another row type.
- [x] **What reads the whole list today reads the keyset or the first page** (Phase 4.5's list):
  - each grid's first-screenful cover prewarm takes its paths off the first page as it lands;
  - Favorites' and Recently Played's `grid_signature` compares keysets rather than hashing rows;
  - `card_selection::visible_ids` reads the seven library scopes' keysets rather than flattening their models, which would build every chunk; Radio's three keep `flatten`;
  - `total-count` stays the unfiltered count: the keyset's length unfiltered, a count query under a filter;
  - the Most Played hero totals (`hero_folds::fold_most_played`) become an aggregate beside the ranking, as decision 7 did for the details;
  - a Most Played card plays the ids its keyset holds (`FavoritesUi::most_played_track_ids` and Recently Played's twin), so `most_played_matches` goes with the in-memory filter;
  - the Playlists stats refresher asks SQL which smart playlists read play stats (`smart_criteria_for_playlists`) and re-reads those it holds, retiring `GridData::stat_dependent` and `row_stats_by_id`;
  - the lone-row card count needs nothing: a chunk holds a placeholder per unfetched card, so `rows[0].<field>.length` still counts what is drawn.
- [x] **`Playlists.rows` becomes `Playlists.count`**, an `int` Rust keeps current from boot on (a count at launch, after every playlist write and on a library tick), whatever tab the session has visited. `update_flat_rows` and the flat model go; `visible-playlist-count` reads the count.
- [x] **Deleted:** each slice's `GridData`, sort keys, `GridIndexCache`, `compute_indices` and `sort_*_indices`; `grid_rows::chunk_rows`; `favorites/grids/sort.rs`; `FavoritesUiState::{most_played, fav_artists}` and `RecentlyPlayedUiState::most_played`; one of the two `to_slint_most_played_row` copies.
**Amended while building (2026-10-19).** Each overrides the bullet above it names.
1. **The Albums filter matches the artist line the card draws**, `COALESCE(artist_credit, artists.name)`, packed into the album's own `search_name`, not "its artist's through the join": the join reaches only the primary artist, so a needle naming the guest on a shared credit would have stopped matching. The album is refilled right after `replace_album_credits`, the one statement writing `artist_credit`, through the SELECT the backfill pages over.
2. **Every packed field ends with the separator** (`name\n`, `name\nsort_name\n`), so a one-field kind is never `''` once filled and `''` keeps meaning "not filled yet".
3. **Most Played reads back packed ids**, not `(id, play_count)` pairs (decision 8): a reload re-reads every card the grid holds, the badge's count with it, and the band's totals are an aggregate (`most_played_totals`, `most_played_favorite_totals`).
4. **The curated pages keep each grid's model across a tab pick**, as Songs does, and release it on the section leave. Favorites takes one dirty flag per tab, and leaving any tab filtered marks it dirty (`shows_filtered`). No grid tab rewinds its count on a pick any more, there being no synchronous apply to write a wrong one.
5. **`grid_signature` is retired**, not moved onto keysets: `PagedRows::show` already repaints only the cards that changed. `last_grid_signature`, both `warm.rs` files and Recently Played's `filter_generation` go with it.
6. **A card converts on the UI thread as it lands**, Favorite Artists' subtitle being a translated plural only a Slint callback resolves. Track rows still convert on the runtime.
7. **The Playlists stats refresher** reads `smart_criteria_for_playlists` over the keyset plus the open detail and keeps those whose rules read play stats; with none it does nothing, otherwise it reloads the ids with a seed of just those, recounted.
8. **`Playlists.count` is written at launch and by every playlist write**, all of which already funnel into the slice's refresh, so no tick subscriber is needed.
9. **`my_library::filter::rewind_grid_count` goes**: a requery writes no count, so a dispatch into a released grid no longer writes the `0` it existed to undo.
10. **`MostPlayedFavorite` narrows to what the card draws.** The four search-only fields, `duration_ms`, `SearchFields`' conversion from it and `most_played_matches` go.
11. **`grid_rows::chunk_entity_rows` goes too**, and the six `GRID_PREWARM_AHEAD` copies become `grid_prewarm`'s one. `PagedGrid` takes one parameter, the chunk row (`GridRow`), its card type being the row's, and its memo keeps whatever chunks Slint holds rather than an LRU. The seven grids share one load path, `ui::card_grid`'s `CardGrid`.
12. **The Playlists grid keys on `updated_at` alone.** Nothing writes `Playlists.sort-field`, the view having no sort row, so the name and track-count arms were unreachable and go, with the live-count sort behind the second. `Playlists.sort-dir` goes too, nothing writing it either, so the grid's query is the needle alone and its order newest change first.

- [x] **Hand check** on each grid: every sort both ways, an accented filter, a click, a range and Select all, the card menu's Play, Play Next, Add to Queue and Go to, the Playlists hover trio, a resize across column counts, a fast scroll to the end, a scan and a playlist write landing while open, a tab leave and return. Also a smart playlist's count after a play, and Add to Playlist in a row menu right after a launch onto Songs.
  - Done (2026-10-19), every check passing. A first look the same day found one thing: scrolled fast, the Albums grid paints its covers with a visible delay where the Songs list's are there at once. A card's rows fetch in well under a millisecond on the dev library (128 albums by id through `album_stats`), so the database is not where the time goes; what is left to trace is the cover path between a card landing and its decode reaching the screen.
- [x] **Tests** (2026-10-19):
  - each SQL arm and filter against today's in-memory sort and `Needle` match, the sorters kept in the test as the oracle (`cross_tier.rs`'s shape), over a fixture of the partitions the two could answer differently: case, accents, ties on every key, a missing year, a smart playlist's live count (a rows-by-id case since amendment 12, no arm sorting on it);
  - each `search_name`'s write-through and backfill;
  - `PagedGrid`: chunk arithmetic where the column count divides the keyset and where it doesn't, the memo handing back the same pointer, a landing patching in place, a column change keeping the cache;
  - `Playlists.count` after a create and a delete with the Playlists tab never opened.
- [x] **Doc edits** (2026-10-19): `ui-patterns.md` (Grids, strips, cards: the paged grids, `card_grid` and `grid_rows`' remaining callers; Filtering: the card grids match `search_name` in SQL; the hero's Most Played totals; the Most Played play path; the tabbed-page contract without the pick rewind or the signature, and both curated deltas), `slint-pitfalls.md` (the filter-box pattern and `clear_mounted`'s guard), `library-data.md` (the card grids' keysets, `search_name` beside the `search_text` write rule, the smart playlists' stats refresh), and the root `CLAUDE.md`'s "Models from background".
- [ ] **Census**, Appendix B states 1 to 7, the grid states being new:
  - this phase against the branch before it, recorded below;
  - **the release comparison:** `v0.19.0` (`5946f2e9`, 22 commits behind the base the Before column measured) against this phase's commit, every state on both builds, `v0.19.0` run on the pre-migration copy `backups/melodia-v20261014000000.db` as Phase 0 ran its base. Its after column fills [Targets](#targets).

Memory: estimated above; the census replaces it.

## Phase 6: Docs

Status: not started.

- [ ] README Footprint: re-run both tables, and add the Songs / Play all / Queue sheet rows the old protocol never exercised.
- [x] `.claude/rules/ui-patterns.md`: the "Lists and playback" and "Covers" entries naming `TrackListModel` and the queue sheet's globals (Phases 2 to 4).
- [x] `.claude/rules/audio-stack.md`: the queue holds ids and the window (Phase 4).

## Targets

51k library, same machine and protocol. "After" is filled from census runs, never estimated.

| State | Before (2026-10-18) | Target | After |
|---|---|---|---|
| Idle (playlist detail open), Anonymous | 37.0 MiB | within ±1 MiB of before | 32.9 MiB; live heap 15.55 against before's 16.22 on the same run (Phase 4) |
| Songs open, Anonymous | 61.3 MiB | ≤ idle + 2 MiB + row-tier covers (≤ 3.4 MiB) | 40.6 MiB, idle + 7.7 of which 6.24 trimmable, left by covers over 512 px (Phase 4.1); live heap idle + 2.36, covers 0.44 |
| Play all, Anonymous | 87.0 MiB | ≤ Songs + 2 MiB | 41.7 MiB, Songs + 1.1 |
| Queue sheet open, Anonymous | 113.8 MiB | ≤ Play all + 1 MiB | 43.0 MiB, Play all + 1.3, the open sheet's rows on screen and their rendering (Phase 4) |
| Queue sheet closed, Anonymous | Play all + 10.7 MiB (Phase 0 run: 96.9 against its 86.2) | = Play all | 37.7 MiB after the close trim; live heap Play all + 0.10 |
| Albums open (state 6), Anonymous | not run | ≤ state 5 + 1 MiB + grid-tier covers | |
| Artists open (state 7), Anonymous | not run | ≤ state 5 + 1 MiB + grid-tier covers | |
| Peak RSS over the run | 385 MB | under 200 MB once the DB connections close | 440.6 MB at boot, file pages (Phase 3.5); 184.1 MB at idle |
| 512-track library, every state | not run | equal to the 51k row within ~1 MiB | |

## Evidence (measured 2026-10-17 and 2026-10-18)

**Setup.**
- **Build:** release `target/release/Melodia` at `6d7e7648`.
- **System:** Fedora 44 (kernel 7.2.10, then 7.2.11), KDE Plasma on Wayland, RTX 3080 on driver 615.78.08, two 2560×1440 outputs at scale 1.
- **Data:** the dev data folder `~/.local/share/Melodia-dev`, holding 51,012 tracks in two folders (`~/Music`, 512; `melodia-test-library-50500`, 50,500).
- **Library shape:**
  - 4,941 distinct covers, 4,806 albums, 3,098 artists, 1,570 album artists, 497 genres;
  - 4,778 track directories, 6,318 inotify watches;
  - average title 14.1 chars, path 116.1, file name 23.4.
- **Who did what:** Kenan drove the UI; the heap was walked read-only after each step.

**Session A**, first launch, playlist "Synthwave" detail open, stopped, 190 s in:
- RSS 192 MB, PSS 102 MB.
- Anonymous 39.5 MiB: `[heap]` 18.2, other anonymous maps 8.8, and 12.0 inside library mappings (relocated pages of the NVIDIA and Mesa drivers, LLVM and the binary's own relro).
- Live heap 15.4 MiB.
- Freed but resident 7.75 MiB: 5.48 in whole pages `malloc_trim` could return, 2.87 in fragments.
- Cover pixels 0.58 MiB: one 384² hero plus 21 row thumbnails at 48², which is exactly the playlist's 21 covers.

**Session B**, after a relaunch:

| State | Anonymous ([heap] / other / in libs) | PSS | RSS | Live heap | Pixels | Free (trimmable + fragments) |
|---|---|---|---|---|---|---|
| Idle, playlist detail open, 6.5 min | 37.0 (14.1 / 10.3 / 11.8) | 96.9 | 188 | 13.87 | 0.15 | 2.32 + 2.89 |
| Songs open | 61.3 (28.1 / 21.4 / 11.8) | 179.7 | 275 | 38.53 | 3.84 | 1.70 + 4.83 |
| Play all | 87.0 (45.3 / 29.9 / 11.8) | 206.1 | 363 | 70.40 | 3.83 | 0.30 + 0.33 |
| Queue sheet open | 113.8 (59.8 / 42.2 / 11.8) | 233.0 | 385 | 97.58 | 3.83 | 0.37 + 0.23 |

All figures are MiB except RSS, which is MB.

**Attribution.**
- **Songs:**
  - Six column arrays sized exactly 51,012 × width: 88, 32, 24, 16, 8 and 8 bytes per track, together 8.57 MiB (the display rows, sort keys, search keys and three smaller per-row columns).
  - About 12.4 MiB of per-row strings.
  - 513 row thumbnails, which is the prewarm filling the 512 cap.
- **Queue:**
  - 51,129 chunks of 256 B, 12.48 MiB: one `Arc<TrackSummary>` per entry.
  - Their owned strings.
- **Queue sheet:**
  - one 8,523,760 B hash table (65,536 slots of 130 B, 8.13 MiB; most likely `LinkCache` grown by `extend`'s reserve);
  - the `QueueRow` model at 56 B per track (2.73 MiB);
  - the shadow at 24 B per track (1.17 MiB);
  - about 120k new 48 B and 73k new 112 B chunks (row strings).
- **Watcher:** about 11,755 directory-path strings on the heap, two per inotify watch, inside notify's own maps.
- **Database connections:** when they closed, RSS fell from 275 MB to 214 MB while `RssAnon` stayed at 62.7 MiB. That is mapped file cache, not memory, so the connections are left alone.
- **README Footprint for comparison:** idle 30.07 MiB at 512 tracks and 36.02 at 51,012, measured with a playlist detail open.

## Checked and left alone

- **Cover tiers.**
  - Row tier: at most 512 × 48² (3.4 MiB).
  - Grid tier: 91 × 224² (13.1 MiB) while a grid is on screen at this window, about 1 MiB of 64² proxies after a leave.
  - Both are bounded by the display, not the library.
- **Database connection reaping:** see the attribution above.
- **The ~12 MiB inside driver mappings.** Mesa's gallium and LLVM load next to NVIDIA's libraries and account for about 3.8 MiB of it. That is environment, not Melodia.
- **Freed-but-resident heap:** with paging, nothing library-sized is freed any more. Idle was re-checked in Phase 3 after the boot trims moved: nothing trimmable. Re-check the other states in the final comparison. Add a trim at a teardown site only if more than 2 MiB is trimmable at idle there.

## Risks / notes

- **Placeholder rows.** A fast scrollbar drag into pages not yet fetched shows placeholder rows until they land, typically one database read of 64 rows by id. Prefetch hides it for ordinary scrolling. Kenan judges it by hand in Phase 2, and if it reads as lag the page size and prefetch are the knobs, not a resident cache.
- **Backfill window.** Right after the upgrade, a filter typed before `search_text_backfill` finishes sees rows whose column is still empty. They match every needle rather than none (decision 3), so the window widens a list and never hides a row.
- **Snapshot staleness.** The keyset is a snapshot (the SQLite thread's own caveat): a row deleted after the fetch comes back as missing from rows-by-ids. `PagedRows` drops it from the visible page as a placeholder until the `library_changed` refetch lands. Never panic on a short page.
- **Staged gapless.** `advance_gapless` seated whatever `queue.advance()` returned, so an edit inside the last `PRELOAD_LEAD_MS` could seat a track other than the staged one. Found by code reading, not reproduced. Phase 4 compares the two and lets the queue win (its decision 2), passed by hand and pinned by the handover cases in `state_tests.rs`.
- **A deferred start shows `Loading`.** A transition whose record the window lacks leaves the deck empty under `Loading` until the fetch lands, which an OS media panel reads as stopped. It needs a transition racing a queue edit, the async start sites fetching ahead; judge by hand whether a slow read pool makes it visible.
- **Collation contract.** The collation must be a total order (sqlx's documented contract: symmetric and transitive), and every connection that runs a keyset query must have it registered, writer included.

## Follow-ups (not in this plan)

- **Watcher on macOS and Windows.** `notify-debouncer-full`'s `RecommendedCache` is `FileIdMap` there, a `HashMap<PathBuf, FileId>` of every path under the roots. It is unmeasured, estimated about 11 MB at 51k by code reading. Measure on Windows first. `apply_watch_batch`'s hash fallback already re-points moves that arrive as remove plus create.
- **Linux inotify maps:** about 1.5 to 1.8 MiB at 6,318 watches, inside notify. Not ours to shrink without a custom watcher.

## Appendix A: `heap-census.py`

Landed as `scripts/heap-census.py` in Phase 0; its docstring carries the method. Read-only, and it needs `ptrace_scope` 0 (or the same uid under a scope that allows the attach). `--per-track N` gives the two views that named each per-track structure above: mmapped chunks as bytes per track, and every main-arena size class holding at least 0.8 × N live chunks.

## Appendix B: Measurement protocol

Same method as 2026-10-18, so a later run compares like with like.

1. **Builds.** Build both commits in release, each in a detached worktree under a `target/` dir, and `cmp` the binaries' provenance before measuring. Compare builds on **Anonymous and the census**, never PSS or RSS: the binary's page cache and the database mappings move those by several MiB between launches.
2. **State to back up.** Before each launch, back up `settings.json`, `views.json` and `queue.json` from `~/.local/share/Melodia-dev`, and restore them byte for byte after. Every quit rewrites them.
3. **Launch** (Kenan launches; Claude never does). Open the Synthwave playlist detail, the README's idle state (Settings reads several MiB lower), then wait **at least 150 s**: the boot's database connections close at the reaper's 120 s tick. Then:

   ```sh
   P=$(pgrep -x Melodia); python3 -I scripts/heap-census.py "$P" --per-track 51012
   ```

   That is state 1, idle.
4. **State 2:** the Songs tab in My Library's tab bar. Wait 30 s and census.
5. **State 3:** activate the first row, which queues the whole displayed list. Wait 30 s and census.
6. **State 4:** open the queue sheet (`Q`). Wait 10 s and census.
7. **State 5:** close the queue sheet. Wait 10 s (the teardown runs 350 ms after close) and census.
8. **State 6:** My Library ▸ Albums. Wait 30 s and census, passing `--per-track 4841` to name what is held per album, and read it against state 5.
9. **State 7:** the Artists tab. Wait 30 s and census with `--per-track 3116`. Genres, Playlists, Favorite Artists and Most Played are too small on the dev library for a census to read (Phase 5's estimate).
10. **The 512-track reference.** Repeat with a data dir holding only `~/Music` (`MELODIA_DATA_DIR=<dir>`), passing `--per-track 512`.
11. **Record** each state's Anonymous, live heap, pixels and free figures in [Targets](#targets). Name each per-track structure from the `--per-track` lines (a column array is an mmapped chunk whose bytes-per-track is a whole number; a per-entry object is a size class at about 1.00 per track).
