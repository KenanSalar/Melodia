# Plan: Paged track lists (every library-sized list holds ids, not rows)

Status: **in progress** (Phases 0 to 2 done, Phase 3 hand-checked and measured with its new tests pending, Phase 3.5 open to discuss, Phase 4 next) · Owner: Kenan · Created: 2026-10-18 · Issue #133, branch `perf/paged-track-lists`

> **Validated 2026-10-18** against `6d7e7648` (`feat/settings-sidebar`), three read-only code traces of the list views, the queue and the index/boot/measurement side, and live heap walks of the release build on the 51,012-track dev library (method in Appendix A, protocol in Appendix B).

Working doc. Delete when every phase has shipped, passed by hand, and the "after" column of [Targets](#targets) is filled from a census run.

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
  - the duplicate `to_slint_most_played_row`.
- **Added:** about 4 to 8 bytes per track per open list, plus a few hundred cached rows.

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
   - **Filling it:** an app task, `tasks::queue_window`, watches the queue version and fetches missing ids with `get_track_summaries_by_ids`.
   - **A miss:** every synchronous transition reads the window. On a miss it returns one new action, `PlayerAction::StartWhenResolved { id, mode }`, which `execute_actions` turns into a fetch followed by `emit_and_execute` if that id is still the slot it was. The gapless and crossfade gates treat a missing next as "no next yet" and re-ask every 500 ms tick, so a transition loses gapless only if the window is still empty in its final `PRELOAD_LEAD_MS`.
   - **Publishing:** `QueueViewModel` carries the ids as an `Arc<[i64]>` shared copy-on-write with `QueueState`. Publishing on every advance becomes O(1), where `state/mod.rs:240-244` rebuilds a whole `Vec<Arc<_>>` under the lock today.
   - **Duplicates and missing rows** keep today's answer: `get_track_summaries_by_ids` drops both (`lookup.rs:98-101`), so `player_play_tracks` dedups and drops through `existing_ids`.
6. **Lists refetch their keyset, never their rows.** A `library_changed` tick refetches ids (one indexed query) and re-reads the rows the list holds, those around the viewport first (amended 2026-10-18 from dropping the cache, which would put every row on screen back on placeholders at each tick). A heart or star click patches the cached row and refetches only the keysets whose membership it moves (Favorites, smart playlists). The write still bumps `library_changed`, so a visible Songs tab follows a click with one keyset query and that re-read.

   Today every click refetches and reconverts the whole library while Songs is visible (`subscribers.rs:23-36`; `favorites.rs:57,84`; `ratings.rs:41,83`).
7. **Details** take their membership ids from SQL and sort them there with their own arm.
   - Membership sources: album, artist and genre through the join tables (`list.rs:64-101`); playlists by `position`; smart playlists through `get_smart_playlist_track_ids`.
   - Hero facts come from SQL aggregates over the same membership, not from folding resident rows: count, running time and dominant genre (`hero_folds`).
   - The detail model is released on close. Today it survives until the tab leave (`callbacks/track_detail.rs:41-61`).
   - Playlist drag-reorder still moves one id in the keyset optimistically (`playlists/detail.rs:289-324`).
8. **Most Played** takes `(id, play_count)` pairs in `MOST_PLAYED_ORDER` (`ranking.rs:20`). Its outer grid model builds each chunk's `EntityGridRow` from the page cache and memoizes chunk `ModelRc`s per generation in a small LRU, so re-reading a chunk returns the same pointer (`slint-pitfalls.md`, the repeater-by-pointer entry).
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
| `crates/melodia-store/src/database/queries/track/membership.rs` (new, Phase 3) | `Membership`, moved out of `keyset.rs`: which tracks a scope holds, as the `FROM` and `WHERE` both the keysets and the scope facts start from |
| `crates/melodia-store/src/database/queries/track/scope_facts.rs` (new, Phase 3) | `scope_facts` (count, running time, artist and album spread in one aggregate, which `get_favorite_stats` reads too) and `genre_lines` (each genre line with its track count, the album hero's genre chip) |
| `crates/melodia-store/src/database/queries/track/list.rs` | `get_all_tracks_for_list`, the comparator's input, and Browse's fetches. The three per-entity `for_list` fetches went in Phase 3, with `get_playlist_tracks_for_list` and `get_smart_playlist_tracks` |
| `crates/melodia-store/src/database/queries/track/ranking.rs` | `most_played_ids` and `most_played_favorite_ids`, beside `MOST_PLAYED_ORDER` |
| `crates/melodia-store/src/database/queries/track/lookup.rs` | `get_track_list_rows_by_ids` (which also serves the queue sheet, every `QueueRow` field being on `TrackListRow`) and `get_most_played_by_ids`, sharing one in-order helper with the four by-id fetches that each spelled it |
| `crates/melodia-store/src/database/queries/track/search_text.rs` (new, Phase 1) | `backfill_search_text`, paged by id, writing only rows still `''` |
| `crates/melodia-store/src/database/queries/scan/mutations.rs` | writes `search_text` through `DerivedColumns`, which also took over the sort key all three writers computed |
| `crates/melodia-app/src/library/keyset.rs` (new, Phase 1) | the library API over it, `facts` and `genre_lines` included (Phase 3) |
| `crates/melodia-app/src/tasks/search_text_backfill.rs` (new, Phase 1) | the per-launch backfill, spawned beside `retroactive_hash` |
| `crates/melodia-app/src/tasks/queue_window.rs` (new) | the window prefetch |
| `crates/melodia-engine/src/player/engine/queue.rs` (510) | ids and orders |
| `crates/melodia-engine/src/player/engine/queue_window.rs` (new) | `TrackWindow`. Keeps `handlers.rs` (666) from growing. `backend/mod.rs` (795) is not touched. |
| `crates/melodia-views/src/ui/paged_rows/mod.rs` (new, Phase 2) | `PagedRows<R>`: the model, the row cache, the window and its one fetch, `ScrollPoint`, the selection, `move_row` for a drag (Phase 3), and the `KeysetTickets` that order a list's loads, keep a library change's re-read owed until one shows, and `revoke` every load out when a list is released (Phase 3) |
| `crates/melodia-views/src/ui/paged_rows/track_rows.rs` (new, Phase 2) | `TrackRows` over `TrackListRow`: the placeholder, the fetcher, the seeded `load`, the cover prewarm, and `settle_reload`, the reload tail Songs, Favorites and the details share (Phase 3) |
| `crates/melodia-views/src/ui/track_list_cache.rs` (498), `track_list_model.rs` (127) | deleted (Phase 2) |
| `crates/melodia-views/src/ui/track_detail/cache.rs` (132) | `DetailCache<G>`: the global's weak handle, the entity, the sort and needle a load runs under, the tickets and the scroll point. No rows (Phase 3) |
| `crates/melodia-views/src/ui/track_detail/load.rs` (new, Phase 3) | the list half of every detail's open (`Opening::seat`), reload and requery |
| `crates/melodia-views/src/ui/track_detail/{filter,selection}.rs` | deleted (Phase 3); the details select through `wire_row_selection` like the flat lists |
| `crates/melodia-views/src/ui/queue_sheet/links.rs` (106) | deleted; the album, artist and genre ids arrive with the row fetch |
| `crates/melodia-views/src/ui/{favorites,recently_played}/rows.rs` | one `to_slint_most_played_row`, not two identical copies (`favorites/rows.rs:37`, `recently_played/rows.rs:34`) |

## Every phase

- [ ] Say what keeps working before saying what moved. Describe the change at the call site, not as a diff.
- [ ] `cargo fmt --all --check` and `cargo clippy --all-targets --locked --workspace -- -D warnings` are clean. Tests the phase breaks are fixed in the phase. New tests and doc edits wait for Kenan's go after his manual check.
- [ ] No production Rust file past 800 lines. The closest today are `backend/mod.rs` 795 (untouched), `handlers.rs` 666, `library/playback.rs` 587 and `now_playing/mod.rs` 542.
- [ ] No `unwrap`; logged errors go through `error::describe`; no `Result<_, String>`.
- [ ] A census run (Appendix B) for the states the phase touches, recorded in the phase's `Memory:` line.

## Phase 0: Measurement tooling and the "before" column

Status: done. The 512-track reference was skipped for time.

- [x] Land Appendix A as `scripts/heap-census.py`, so the method outlives a scratchpad. A reboot wiped the session copy once already.
- [x] Build the branch's merge base in a detached worktree under a `target/` dir, and run Appendix B on the 51k library.
  - Built: `065275c4` at `target/worktrees/base/target/release/Melodia`, sha256 `43d06cbc9a042e325f00194bc93a291a8707ab9a403dcfb4be5fea2b2c7d01d9`.
  - The re-run landed within about 2 MiB of every figure the original measured (idle 35.3, Songs 59.1, Play all 86.2, queue sheet open 113.2 MiB), each step's growth within 1.4 MiB of the original's, so the Before column keeps the original run. **Its idle is the Synthwave playlist detail, not Settings**, which the tables and Appendix B now say; Settings idle reads 31.2 MiB.
  - The one new figure is the queue sheet closed: closing gives back 16.5 of the 26.6 MiB the sheet took. The 56 B-per-track `QueueRow` model and the row strings go; the 8,523,760 B table and a 24 B-per-track array stay, which is what Phase 4's "closed = Play all" target has to remove.
  - Run on the dev data folder with the pre-migration copy `backups/melodia-v20261014000000.db` (a Phase 1 build had already migrated the live one, which the base build refuses), into a null sink, everything restored after.
- Skipped: the same five states on a 512-track data dir, the flatness reference.

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
- [x] After the validation, one fix: a sort or keystroke overtaking a library tick's refresh took the newer ticket, and the refresh's re-read, count and selection prune went with it, leaving edited rows on their old text until the next change. `KeysetTickets` now keeps a refresh owed until one shows, and a requery issued meanwhile runs as one, decided under the same lock that issues its ticket. An overtaken refresh also skips its cover prewarm. Hand check pending.
- [x] Tests:
  - `paged_rows/tests/paged_rows_tests.rs`: the `window` boundaries; one fetch in flight; a landing asks nearest the row read last; a page scrolled past is never fetched; an absent row is not re-asked until a new keyset; a failed fetch waits for the next read; a landing after `clear` stores nothing; an empty list; the flag on a placeholder; the scroll point; the repaints (a patch, an uncached patch, an unchanged and a one-row-changed show, a late fetch against a patch, one and two removals); and the `KeysetTickets` decision table. Selection surviving a swap and a clear was already pinned in `list_selection_tests.rs`.
  - `melodia-core`'s `track_search_tests.rs`: the packed format, four separators for five empty fields, NUL as a space, no match across two fields, and the packed text agreeing with the fields over six partitions.
  - Store: an empty and a one-id keyset; the favourite spread as written-out counts (an untagged track is the unknown artist, so it counts as one).
  - `favorites_tests.rs`: which tab and needle leave Songs owing a reload. `list_selection_tests.rs`: the Clear selection gate.
  - `cross_tier`'s parity fixture gains lengths on and either side of both ends of the Songs clamp: moving the bound passed every test before.
  - Checked by mutation, each failing only its own pins: the in-flight guard, the `absent` filter, the read-stamp arm, the owed-reload settle and takeover, the separator, `DISTINCT`, the menu gate, the Favorites tab check, and both ends of the length clamp. A repaint outside the window is deliberately not pinned: Slint ignores a row it doesn't hold, so it costs time, never a wrong row.

Memory, Songs open as the first view on the 51k library, release, settled 3 minutes (2026-10-18). This is not the Appendix B path (playlist idle, then Songs), so [Targets](#targets) waits for that run:

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

Status: implemented, hand-checked and measured (2026-10-18), not committed. fmt, clippy and the full suite green; new tests wait for Kenan's go, so the phase stays open.

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
- [ ] New tests, after the hand check: `move_row`'s bounds and notifications, `revoke`, `scope_facts` per scope (a smart playlist's limit and an empty scope among them), `genre_lines`' grouping and order, the weighted tie, and an overtaken open requerying.

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
- **Freed-but-resident at idle was 2.84 MiB trimmable**, past this plan's 2 MiB threshold for adding a trim (Checked and left alone), the one boot trim at 5 s running before the folder rescan and the 120 s pool reap. `tasks::heap_trim` now trims at 10 s and 15 s after the pool's boot connections close (135 s), not yet measured.

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

Status: not started.

- [ ] Engine changes:
  - `QueueState` holds ids;
  - add `TrackWindow` and `StartWhenResolved`;
  - every transition reads the window: `begin_track` and the next, previous, end-of-stream, crossfade, auto-skip, `resume_from_stopped` and prune-reseat builders;
  - `evaluate_playing_tick` and `next_needs_reopen` read the window's next;
  - `advance_gapless` seats the id that was actually staged.
- [ ] App changes:
  - `player_play_tracks`, `queue_add_tracks`, `play_next_many`, `open_as_queue` and `queue_skip_to_index` pass ids;
  - `plan_restore` fetches the window only, and remaps `current_index` when ids dropped out. Today the index survives unremapped (`persistence.rs:43`), a pre-existing bug;
  - `sync_track_summaries` patches the window.
- [ ] Views:
  - the sheet becomes `PagedRows<QueueRow>` over the queue keyset; `ShadowEntry` keeps selection only (ids, no records);
  - delete `LinkCache`;
  - the favourite toggle patches the page and the window, not `st.queue.tracks` from a worker (`callbacks.rs:174-186`);
  - Up Next fetches its 20 rows by id;
  - `latest_qvm` holds ids.
- [ ] Keep the 350 ms close teardown and its `malloc_trim`.

Memory: Play all and queue sheet open both within the [Targets](#targets), and closing the sheet returns to the Play-all reading.

## Phase 4.5 (to discuss): the card grids and Browse

Status: not decided. Kenan investigates and discusses this before anything here is planned.

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
- **Browse is the odd one out** (decision 9). It shows one directory at a time, and its disk-only rows have no track id, so a keyset of track ids can't hold them; a paged Browse would key by path. Its list and its cards are bounded by the largest directory, so the question is whether a library keeps tens of thousands of files in one folder. Measure a large flat folder before deciding.
  - **To discuss:** the two move together, the cards being the list's own rows (`BrowseCardGrid` reads the list's selection), so the answer would be one path-keyed paged source behind both, never the grid alone. Leaning towards leaving Browse as it is, for the two reasons above; open until the flat-folder measurement says otherwise.
- **The outcome:** for each surface, keep it in memory as it is, or page it in a phase of its own.

Memory: unmeasured.

## Phase 5: Most Played

Status: not started.

- [ ] Both grids take a keyset of `(id, play_count)` and a memoized chunk model. The two `to_slint_most_played_row` copies become one.

Memory: Most Played on a library with many played tracks costs the keyset.

## Phase 6: Docs

Status: not started.

- [ ] README Footprint: re-run both tables, and add the Songs / Play all / Queue sheet rows the old protocol never exercised.
- [ ] `.claude/rules/ui-patterns.md`: the "Lists and playback" and "Covers" entries naming `TrackListModel` and the queue sheet's globals.
- [ ] `.claude/rules/audio-stack.md`: the queue holds ids and the window.

## Targets

51k library, same machine and protocol. "After" is filled from census runs, never estimated.

| State | Before (2026-10-18) | Target | After |
|---|---|---|---|
| Idle (playlist detail open), Anonymous | 37.0 MiB | within ±1 MiB of before | |
| Songs open, Anonymous | 61.3 MiB | ≤ idle + 2 MiB + row-tier covers (≤ 3.4 MiB) | |
| Play all, Anonymous | 87.0 MiB | ≤ Songs + 2 MiB | |
| Queue sheet open, Anonymous | 113.8 MiB | ≤ Play all + 1 MiB | |
| Queue sheet closed, Anonymous | Play all + 10.7 MiB (Phase 0 run: 96.9 against its 86.2) | = Play all | |
| Peak RSS over the run | 385 MB | under 200 MB once the DB connections close | |
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
- **Freed-but-resident heap:** with paging, nothing library-sized is freed any more. Re-check in Phase 0's after-run. Add a trim at a teardown site only if more than 2 MiB is trimmable at idle there.

## Risks / notes

- **Placeholder rows.** A fast scrollbar drag into pages not yet fetched shows placeholder rows until they land, typically one database read of 64 rows by id. Prefetch hides it for ordinary scrolling. Kenan judges it by hand in Phase 2, and if it reads as lag the page size and prefetch are the knobs, not a resident cache.
- **Backfill window.** Right after the upgrade, a filter typed before `search_text_backfill` finishes sees rows whose column is still empty. They match every needle rather than none (decision 3), so the window widens a list and never hides a row.
- **Snapshot staleness.** The keyset is a snapshot (the SQLite thread's own caveat): a row deleted after the fetch comes back as missing from rows-by-ids. `PagedRows` drops it from the visible page as a placeholder until the `library_changed` refetch lands. Never panic on a short page.
- **Staged gapless.** `advance_gapless` (`handlers.rs:484`) seats whatever `queue.advance()` returns, so an edit inside the last `PRELOAD_LEAD_MS` may seat a track other than the staged one. Found by code reading, not reproduced. Phase 4 seats the staged id; verify by hand.
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
8. **The 512-track reference.** Repeat with a data dir holding only `~/Music` (`MELODIA_DATA_DIR=<dir>`), passing `--per-track 512`.
9. **Record** each state's Anonymous, live heap, pixels and free figures in [Targets](#targets). Name each per-track structure from the `--per-track` lines (a column array is an mmapped chunk whose bytes-per-track is a whole number; a per-entry object is a size class at about 1.00 per track).
