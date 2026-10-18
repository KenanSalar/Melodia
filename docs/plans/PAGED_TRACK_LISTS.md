# Plan: Paged track lists (every library-sized list holds ids, not rows)

Status: **proposed** · Owner: Kenan · Created: 2026-10-18

> **Validated 2026-10-18** against `6d7e7648` (`feat/settings-sidebar`), three read-only code traces of the list views, the queue and the index/boot/measurement side, and live heap walks of the release build on the 51,012-track dev library (method in Appendix A, protocol in Appendix B).

Working doc. Delete when every phase has shipped, passed by hand, and the "after" column of [Targets](#targets) is filled from a census run.

## Context

**What you see today.** On the 51,012-track library, Anonymous memory (the memory Melodia allocated itself) climbs with every library-sized surface you open and none of it comes back while the session lasts:

| Step (one process, 2026-10-18) | Anonymous | Live heap | Added |
|---|---|---|---|
| Settings, idle | 37.0 MiB | 13.9 MiB | |
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
     - **Year:** NULLs first. Songs and Favorites also conflate year 0 with NULL, pinned at `track_list_cache_tests.rs:339`.
     - **Length.**
     - **Tie-breakers:** Songs and Favorites break ties on `sort_key`, the details on lowercased title, playlists on position.
     - **Descending:** reverses the ascending order.
   - SQL appends `id` as the last key so the order is total, which is what makes `DESC` an exact reverse. Rows tied on every key today come back in whatever order the fetch gave them; they will now come back in id order. That is the only visible difference and it is deliberate.
   - `track_sort` stays, because the bounded lists still sort in memory, and it becomes the oracle the SQL parity tests compare against.
3. **Filter runs in SQL against a stored folded column, `tracks.search_text`.** It is packed exactly like `RowSearchKey::from_row` (`track_list_cache.rs:48-66`): `fold::push_folded` over `search_fields` (title, artist, album_artist, album, genre), joined by `\0`.
   - The match is `instr(search_text, ?) > 0`. When the needle is all digits, it also accepts `instr(CAST(year AS TEXT), ?) > 0`, which is `Needle::matches_number` (`row_match.rs:199`).
   - `search_fields` and the packing move to `melodia-core`, so the store writes the column with the same function the bounded lists match with.
   - sqlx exposes collations but no scalar functions, so a stored column is the only way to keep today's accent-folding semantics without `unsafe`.
4. **One Slint model type for every paged list: `ui::paged_rows::PagedRows<R>`.** It holds:
   - the keyset as an `Arc<[i64]>`;
   - an LRU of pages: 64 rows each, 8 pages cap, prefetch 2 pages either side of what Slint last asked for (512 rows, the Android rule);
   - the selected-id set, which `TrackListModel` already keeps as a `HashSet<i32>`;
   - a generation.

   `row_data` answers from a cached page, or returns a placeholder row (id, `selected`, empty text) and schedules that page plus its prefetch. When the page lands, `row_changed` is fired for its range. That is the contract the cover tiers already run (`get_or_schedule_opt` plus a generation repaint, `ui-patterns.md` "Covers").

   Two things must hold:
   - **`selected` never depends on the page.** `track-list.slint:116-117` and `queue-sheet.slint:297-298` read the neighbours' `.selected`.
   - **The `ModelRc` is installed once per list.** A new keyset swaps inside it and calls `reset`, because `Repeater` compares models by pointer.
5. **The queue holds ids. The engine reads a prefetched window, and a miss defers rather than blocks.**
   - **Shape:** `QueueState.tracks` becomes `Vec<i64>`. A new `TrackWindow` holds at most six `Arc<TrackSummary>` keyed by id: previous, current, next under the repeat mode, next under manual Next (they differ under repeat-one, `queue.rs:261` vs `:304`), and the two after that for the auto-skip chain (`actions.rs:276-285`).
   - **Filling it:** an app task, `tasks::queue_window`, watches the queue version and fetches missing ids with `get_track_summaries_by_ids`.
   - **A miss:** every synchronous transition reads the window. On a miss it returns one new action, `PlayerAction::StartWhenResolved { id, mode }`, which `execute_actions` turns into a fetch followed by `emit_and_execute` if that id is still the slot it was. The gapless and crossfade gates treat a missing next as "no next yet" and re-ask every 500 ms tick, so a transition loses gapless only if the window is still empty in its final `PRELOAD_LEAD_MS`.
   - **Publishing:** `QueueViewModel` carries the ids as an `Arc<[i64]>` shared copy-on-write with `QueueState`. Publishing on every advance becomes O(1), where `state/mod.rs:240-244` rebuilds a whole `Vec<Arc<_>>` under the lock today.
   - **Duplicates and missing rows** keep today's answer: `get_track_summaries_by_ids` drops both (`lookup.rs:98-101`), so `player_play_tracks` dedups and drops through `existing_ids`.
6. **Lists refetch their keyset, never their rows.** A `library_changed` tick refetches ids (one indexed query) and drops the page cache. A heart or star click patches the cached row and refetches only the keysets whose membership it moves (Favorites, smart playlists).

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
| `crates/melodia-core/src/entities/track_search.rs` (new) | `search_fields` (moved from `views/ui/row_match.rs:33-41`) and `pack_search_text` (moved from `RowSearchKey::from_row`) |
| `migrations/2026MMDD000000_track_search_text.sql` (new) | `search_text TEXT NOT NULL DEFAULT ''` |
| `crates/melodia-store/src/database/collation.rs` (new) | the collation names and comparators; `database/mod.rs` registers them on both pools (`:262-271`, `:340-347`) |
| `crates/melodia-store/src/database/queries/track/keyset.rs` (new) | id queries per scope (library, favorites, album, artist, genre, playlist, most played) with the sort arm and needle, and rows-by-ids that preserve input order |
| `crates/melodia-store/src/database/queries/scan/mutations.rs` | writes `search_text` at both inserts (`:188`, `:248`) and in `update_track_metadata` (`:331`) |
| `crates/melodia-app/src/library/keyset.rs` (new) | the library API over it (`ListScope`, sort, needle) |
| `crates/melodia-app/src/tasks/search_text_backfill.rs` (new) | a `one_shot::spawn` sweep filling `search_text` on existing installs |
| `crates/melodia-app/src/tasks/queue_window.rs` (new) | the window prefetch |
| `crates/melodia-engine/src/player/engine/queue.rs` (510) | ids and orders |
| `crates/melodia-engine/src/player/engine/queue_window.rs` (new) | `TrackWindow`. Keeps `handlers.rs` (666) from growing. `backend/mod.rs` (795) is not touched. |
| `crates/melodia-views/src/ui/paged_rows.rs` (new) | `PagedRows<R>`: the model, page cache, fetch scheduling and selection |
| `crates/melodia-views/src/ui/track_list_cache.rs` (498), `track_list_model.rs` (127) | deleted |
| `crates/melodia-views/src/ui/track_detail/cache.rs` (132) | the keyset and needle only |
| `crates/melodia-views/src/ui/queue_sheet/links.rs` (106) | deleted; the album, artist and genre ids arrive with the row fetch |
| `crates/melodia-views/src/ui/{favorites,recently_played}/rows.rs` | one `to_slint_most_played_row`, not two identical copies (`favorites/rows.rs:37`, `recently_played/rows.rs:34`) |

## Every phase

- [ ] Say what keeps working before saying what moved. Describe the change at the call site, not as a diff.
- [ ] `cargo fmt --all --check` and `cargo clippy --all-targets --locked --workspace -- -D warnings` are clean. Tests the phase breaks are fixed in the phase. New tests and doc edits wait for Kenan's go after his manual check.
- [ ] No production Rust file past 800 lines. The closest today are `backend/mod.rs` 795 (untouched), `handlers.rs` 666, `library/playback.rs` 587 and `now_playing/mod.rs` 542.
- [ ] No `unwrap`; logged errors go through `error::describe`; no `Result<_, String>`.
- [ ] A census run (Appendix B) for the states the phase touches, recorded in the phase's `Memory:` line.

## Phase 0: Measurement tooling and the "before" column

Status: not started.

- [ ] Land Appendix A as `scripts/heap-census.py`, so the method outlives a scratchpad. A reboot wiped the session copy once already.
- [ ] Build the branch's merge base in a detached worktree under a `target/` dir, and run Appendix B on the 51k library. That fills the "before" column, including the state not measured on 2026-10-18 (queue sheet closed).
- [ ] Run the same five states on a 512-track data dir, the flatness reference.

## Phase 1: SQL side (no UI change)

Status: not started.

- [ ] Add the `search_text` migration, the ingest writes and the backfill sweep. A one-shot marker records completion and `OnFailure` keeps the marker unset.
- [ ] Register the collation(s) on both pools.
- [ ] Write the keyset queries, plus rows-by-ids for `TrackListRow`, `QueueRow` fields (with the album, artist and genre ids) and Most Played (with `play_count`).
- [ ] Parity, after Kenan's go:
  - extend `crates/melodia/tests/cross_tier.rs` so every field × direction × arm (Songs, Favorites, details, playlist) compares SQL order against `track_sort`, over fixtures built from the partitions they could answer differently (accents, case, NULL and 0 years, missing track numbers, exact ties);
  - compare `instr` matching against `RowSearchKey::matches` over the `row_match_tests` corpus.

Memory: none at runtime. Disk grows by roughly the folded text, about 70 B per track.

## Phase 2: `PagedRows` plus Songs and Favorites Songs

Status: not started.

- [ ] `PagedRows<UiTrackListRow>`, installed once per list. `Tracks.rows` and Favorites' rows go through it.
- [ ] The Songs and Favorites fetch paths keep the keyset only:
  - `fetch_and_apply`, `refilter` and `resort_and_apply` (`tracks/fetch.rs:25,64,76`);
  - `refresh_tracks`, `build_filtered_tracks` and `write_filtered_tracks` (`favorites/songs.rs:75-242`).

  Play and Shuffle read the keyset (`current_ids_filtered`, `filtered_track_ids`).
- [ ] Hearts and stars patch the cached row. A Favorites unfavourite removes the id from that keyset, as `TrackListCache::remove` does today. The Now Playing mirror (`CurrentTrackMirrors`) reaches the same patch.
- [ ] `install_library_changed_refresher` refetches the keyset. Fix its stale comment (`subscribers.rs:19-22` still says play-count flushes bump this channel).
- [ ] Delete `TrackListCache`, `TrackListModel` and their tests. Port the pins that still mean something: repaint, refetch diff and selection survival to `paged_rows` tests.
- [ ] Update `CLAUDE.md` (the "Models from background" sentence naming `TrackListModel::installed`) and `library-data.md` (the "retain and permute" paragraph this plan reverses).

Memory: Songs open stays within the [Targets](#targets).

## Phase 3: The four details

Status: not started.

- [ ] `DetailCache` keeps the keyset and needle. `seat_unfiltered`'s second copy (`cache.rs:79-84`) and the `VecModel` go.
- [ ] `open_*_with` fetches ids sorted by the view's `resolve_view_sort` arm. `cross_tier.rs:347` keeps reading those literals.
- [ ] Hero folds come from SQL aggregates.
- [ ] Release the detail model on close.
- [ ] Playlist drag-reorder and rollback work on the keyset.

Memory: a 20k-track genre detail costs the keyset, not about 28 MB (estimate by code reading).

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
| Settings idle, Anonymous | 37.0 MiB | within ±1 MiB of before | |
| Songs open, Anonymous | 61.3 MiB | ≤ idle + 2 MiB + row-tier covers (≤ 3.4 MiB) | |
| Play all, Anonymous | 87.0 MiB | ≤ Songs + 2 MiB | |
| Queue sheet open, Anonymous | 113.8 MiB | ≤ Play all + 1 MiB | |
| Queue sheet closed, Anonymous | not measured | = Play all | |
| Peak RSS over the run | 385 MB | under 200 MB once the DB connections close | |
| 512-track library, every state | (Phase 0) | equal to the 51k row within ~1 MiB | |

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
| Settings, 6.5 min | 37.0 (14.1 / 10.3 / 11.8) | 96.9 | 188 | 13.87 | 0.15 | 2.32 + 2.89 |
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
- **Backfill window.** Right after the upgrade, a filter typed before `search_text_backfill` finishes can miss rows whose column is still empty. Gate SQL filtering on the backfill marker, falling back to no filter rather than wrong matches.
- **Snapshot staleness.** The keyset is a snapshot (the SQLite thread's own caveat): a row deleted after the fetch comes back as missing from rows-by-ids. `PagedRows` drops it from the visible page as a placeholder until the `library_changed` refetch lands. Never panic on a short page.
- **Staged gapless.** `advance_gapless` (`handlers.rs:484`) seats whatever `queue.advance()` returns, so an edit inside the last `PRELOAD_LEAD_MS` may seat a track other than the staged one. Found by code reading, not reproduced. Phase 4 seats the staged id; verify by hand.
- **Collation contract.** The collation must be a total order (sqlx's documented contract: symmetric and transitive), and every connection that runs a keyset query must have it registered, writer included.

## Follow-ups (not in this plan)

- **Watcher on macOS and Windows.** `notify-debouncer-full`'s `RecommendedCache` is `FileIdMap` there, a `HashMap<PathBuf, FileId>` of every path under the roots. It is unmeasured, estimated about 11 MB at 51k by code reading. Measure on Windows first. `apply_watch_batch`'s hash fallback already re-points moves that arrive as remove plus create.
- **Linux inotify maps:** about 1.5 to 1.8 MiB at 6,318 watches, inside notify. Not ours to shrink without a custom watcher.

## Appendix A: `heap-census.py`

Read-only. It needs `ptrace_scope` 0 (or the same uid with an attach-capable scope); nothing is stopped or injected. It walks glibc's main arena, each 64 MiB-aligned secondary arena and every mmapped chunk through `/proc/<pid>/mem`, reads residency from `/proc/<pid>/pagemap`, and identifies slint pixel buffers by their `SharedVector` header (refcount, `len == cap`, chunk = 24 + bpp × len). `--per-track N` also lists mmapped chunks as bytes per track and every main-arena size class holding at least 0.8 × N live chunks: the two views that named each per-track structure above.

```python
#!/usr/bin/env python3
"""Read-only census of a live glibc process heap (see the plan's Appendix A)."""
import argparse
import collections
import struct

PAGE = 4096
HEADER = 24  # slint SharedVectorHeader: refcount, len, capacity
MIB = 1048576


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("pid", type=int)
    parser.add_argument("--per-track", type=int, metavar="N", help="track count to divide by")
    args = parser.parse_args()
    census = Census(args.pid)
    census.print_split()
    census.walk()
    census.print_summary()
    if args.per_track:
        census.print_per_track(args.per_track)


class Census:
    def __init__(self, pid):
        self.pid = pid
        self.mem = open(f"/proc/{pid}/mem", "rb", buffering=0)
        self.pagemap = open(f"/proc/{pid}/pagemap", "rb", buffering=0)
        self.maps = []
        for line in open(f"/proc/{pid}/maps"):
            parts = line.split()
            lo, hi = (int(x, 16) for x in parts[0].split("-"))
            self.maps.append((lo, hi, parts[1], parts[5] if len(parts) > 5 else ""))
        self.live = 0
        self.free_fragments = 0
        self.free_trimmable = 0
        self.pixels = collections.defaultdict(lambda: [0, 0])
        self.main_sizes = collections.Counter()
        self.mmapped = []

    def read(self, addr, n):
        self.mem.seek(addr)
        return self.mem.read(n)

    def u64(self, addr):
        return struct.unpack("<Q", self.read(addr, 8))[0]

    def present_pages(self, lo, hi):
        first, last = -(-lo // PAGE), hi // PAGE
        if last <= first:
            return 0
        self.pagemap.seek(first * 8)
        raw = self.pagemap.read((last - first) * 8)
        return sum(1 for (e,) in struct.iter_unpack("<Q", raw) if e >> 63 & 1)

    def print_split(self):
        heap = anon = libs = pss = 0
        name = ""
        for line in open(f"/proc/{self.pid}/smaps"):
            fields = line.split()
            if "-" in fields[0] and len(fields) >= 5 and not fields[0].endswith(":"):
                name = fields[5] if len(fields) >= 6 else "[anon]"
            elif fields[0] == "Pss:":
                pss += int(fields[1])
            elif fields[0] == "Anonymous:":
                kib = int(fields[1])
                if name == "[heap]":
                    heap += kib
                elif name == "[anon]":
                    anon += kib
                else:
                    libs += kib
        status = {}
        for line in open(f"/proc/{self.pid}/status"):
            key, _, value = line.partition(":")
            status[key] = value.strip()
        print(f"VmRSS {status['VmRSS']}  RssAnon {status['RssAnon']}  RssFile {status['RssFile']}  "
              f"RssShmem {status['RssShmem']}  VmHWM {status['VmHWM']}")
        print(f"PSS {pss / 1024:.1f} MiB; Anonymous {(heap + anon + libs) / 1024:.1f} = [heap] "
              f"{heap / 1024:.1f} + other anon {anon / 1024:.1f} + inside library mappings {libs / 1024:.1f} MiB")

    def walk_chain(self, start, end):
        p = start
        while p + 16 <= end:
            size = self.u64(p + 8) & ~7
            if size < 32 or p + size > end:
                return
            nxt = p + size
            if nxt + 16 > end:
                yield p, size, False
                return
            yield p, size, bool(self.u64(nxt + 8) & 1)
            p = nxt

    def pixel_tag(self, addr, usable, slack):
        if usable < HEADER + 3 * 64:
            return None
        refcount, length, capacity = struct.unpack("<qQQ", self.read(addr + 16, 24))
        if not (0 < refcount < 4096 and length == capacity and length >= 64):
            return None
        for bpp, tag in ((3, "rgb8"), (4, "rgba8")):
            need = HEADER + bpp * length
            if need <= usable < need + slack:
                side = int(round(length ** 0.5))
                return f"{tag} {side}x{side}" if side * side == length else f"{tag} {length}px"
        return None

    def record_live(self, addr, usable, resident, slack):
        self.live += resident
        tag = self.pixel_tag(addr, usable, slack)
        if tag:
            self.pixels[tag][0] += 1
            self.pixels[tag][1] += resident

    def walk(self):
        heap = next(m for m in self.maps if m[3] == "[heap]")
        arenas = [(heap[0], heap[1], "main")]
        for lo, hi, perm, name in self.maps:
            if name == "" and perm == "rw-p" and lo % (64 << 20) == 0:
                used = self.u64(lo + 16)  # heap_info.size
                if 0 < used <= hi - lo:
                    arenas.append((lo, lo + used, "arena"))
        for lo, hi, kind in arenas:
            start = lo if kind == "main" else self.first_chunk(lo, hi)
            if start is None:
                print(f"no chunk chain found in arena at {lo:#x}")
                continue
            for addr, size, used in self.walk_chain(start, hi):
                if used:
                    if kind == "main":
                        self.main_sizes[size] += 1
                    resident = size if size < 64 * 1024 else self.present_pages(addr, addr + size) * PAGE
                    self.record_live(addr, size - 8, resident, 32)
                elif size < 2 * PAGE:
                    self.free_fragments += size
                else:
                    self.free_trimmable += self.present_pages(addr + 48, addr + size) * PAGE
        arena_starts = {a[0] for a in arenas}
        for lo, hi, perm, name in self.maps:
            if name or perm != "rw-p" or lo in arena_starts:
                continue
            p = lo
            while p + 16 <= hi:
                try:
                    raw = self.u64(p + 8)
                except OSError:
                    break  # driver-backed VMA, not malloc's
                size = raw & ~7
                span = -(-size // PAGE) * PAGE
                if not raw & 2 or size == 0 or p + span > hi:
                    break
                resident = self.present_pages(p, p + span) * PAGE
                self.mmapped.append((size, resident))
                self.record_live(p, size - 16, resident, PAGE + 32)
                p += span

    def first_chunk(self, lo, hi):
        """A secondary arena's first chunk follows heap_info and malloc_state; find the offset
        whose chain ends exactly on the top chunk."""
        for off in range(0x30, 0x1400, 16):
            last = None
            for last in self.walk_chain(lo + off, hi):
                pass
            if last and last[0] + last[1] == hi and not last[2]:
                return lo + off
        return None

    def print_summary(self):
        print(f"live heap (resident): {self.live / MIB:.2f} MiB, {len(self.mmapped)} mmapped chunks")
        print(f"free but resident: {self.free_trimmable / MIB:.2f} MiB in whole pages "
              f"(malloc_trim can return), ~{self.free_fragments / MIB:.2f} MiB in sub-page fragments")
        total = 0
        for tag, (count, resident) in sorted(self.pixels.items(), key=lambda kv: -kv[1][1]):
            total += resident
            print(f"  pixels {tag:16s} x{count:4d}  {resident / MIB:6.2f} MiB")
        print(f"  pixels total {total / MIB:.2f} MiB")

    def print_per_track(self, tracks):
        print(f"\nmmapped chunks, largest first (bytes per track at {tracks}):")
        for size, resident in sorted(self.mmapped, reverse=True)[:16]:
            print(f"  {size:10d} B  resident {resident / MIB:6.2f} MiB  {size / tracks:6.1f} B per track")
        print(f"main-arena size classes holding >= {int(tracks * 0.8)} live chunks:")
        for size, count in sorted(self.main_sizes.items()):
            if count >= tracks * 0.8:
                print(f"  {size:5d} B x{count:6d} = {size * count / MIB:6.2f} MiB  ({count / tracks:.2f} per track)")


if __name__ == "__main__":
    main()
```

## Appendix B: Measurement protocol

Same method as 2026-10-18, so a later run compares like with like.

1. **Builds.** Build both commits in release, each in a detached worktree under a `target/` dir, and `cmp` the binaries' provenance before measuring. Compare builds on **Anonymous and the census**, never PSS or RSS: the binary's page cache and the database mappings move those by several MiB between launches.
2. **State to back up.** Before each launch, back up `settings.json`, `views.json` and `queue.json` from `~/.local/share/Melodia-dev`, and restore them byte for byte after. Every quit rewrites them.
3. **Launch** (Kenan launches; Claude never does). Open Settings, then wait **at least 150 s**: the boot's database connections close at the reaper's 120 s tick. Then:

   ```sh
   P=$(pgrep -x Melodia); python3 -I scripts/heap-census.py "$P" --per-track 51012
   ```

   That is state 1, Settings idle.
4. **State 2:** My Library ▸ Songs. Wait 30 s and census.
5. **State 3:** activate the first row, which queues the whole displayed list. Wait 30 s and census.
6. **State 4:** open the queue sheet (`Q`). Wait 10 s and census.
7. **State 5:** close the queue sheet. Wait 10 s (the teardown runs 350 ms after close) and census.
8. **The 512-track reference.** Repeat with a data dir holding only `~/Music` (`MELODIA_DATA_DIR=<dir>`), passing `--per-track 512`.
9. **Record** each state's Anonymous, live heap, pixels and free figures in [Targets](#targets). Name each per-track structure from the `--per-track` lines (a column array is an mmapped chunk whose bytes-per-track is a whole number; a per-entry object is a size class at about 1.00 per track).
