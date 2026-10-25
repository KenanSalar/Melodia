# Plan: Tests for the queue panel branch

Status: **planned**, no phase started; each starts on Kenan's go · Owner: Kenan · Created: 2026-10-25 · Branch `feat/queue-panel`, covering the work `QUEUE_PANEL.md` plans

Working doc. Delete once all three phases have landed.

## Context

**What you see today.** The suite is green on a branch whose new behaviour almost nothing exercises.
- **The old tests were renamed, not extended.** `move_track`'s cases now call `move_rows(&[row], …)` and `reorder_playlist_track`'s call `move_playlist_tracks(&[id], …)`, so every move under test is a block of one. No test builds a `QueueEntry`, calls `insert_before` or `skip_to_entry`, moves a block of several rows, routes a file drop, or reads a drag mount.
- **The branch's fixes cluster in three places**, which is where a regression is likeliest next:
  - picks over a track queued twice (`105832ab`, `e00836d6`, `ab1d3725`);
  - where a drop lands once the queue has moved (`a71bea18`, Phase 7.8);
  - who holds the keys and what arms a drag (`055479cc`, `9f9deb35`, `c4f70a9b`, `7720885b`).
- **Some of it can't be checked by hand yet.** The touch gate waits for a touchscreen (Phase 6), and no CI run has built the branch on Windows.

**What keeps working.** Every behaviour the branch shipped. This plan adds tests and changes no visible behaviour. Production code moves in two places only: two decisions lifted out of window reads so a test can reach them (Phase 2), and one lookup arm nothing calls any more, deleted (Found while surveying).

## Decisions settled here

1. **By risk, not by file.** Phase 1 is the orders a user's queue and playlists are written through, Phase 2 the models the user picks and drops through, Phase 3 the cross-tree prohibitions no compile catches.
2. **Real collaborators, no doubles.** Views tests build a real `QueueState` and take its entries (`QueueEntry` has no public constructor, which keeps them honest). Store tests run on `DbPool::test_pool`. Nothing is mocked.
3. **Each question at the lowest level that can answer it.** The Phase 7.8 race (the queue moving while a drop resolves) is reproduced in the engine by reading the entry, applying the edit, then splicing. The library's `await` adds nothing the test needs, and `queue_insert_before` and `queue_skip_to_index` take an `&AppState` no suite can build.
4. **A source walk only for a prohibition with all three of these:**
   - it can be broken from a file other than the one that owns the rule;
   - it fails silently, or only on hardware or a locale the review doesn't have;
   - it has a fixed spelling to search for.

   Layout, feel and one-file wiring stay with the hand passes. A text pin on a layout expression passes every rewrite that keeps the spelling and fails every harmless rename.
5. **Table-driven cases with the expected value written out.** No new dev-dependency or framework (`testing.md`'s toolbox section).
6. **Every new test is checked by breaking what it pins.** Each phase lists the breaks that must fail it, and only it.

## Structure (who owns what afterwards)

| File | Holds |
|---|---|
| `crates/melodia-core/src/utils/tests/block_move_tests.rs` (new, P1) | `move_block` and `landing` |
| `crates/melodia-engine/src/player/engine/tests/queue_entry_tests.rs` (new, P1) | Entries, the splice, `skip_to_entry`, block moves round the current row. Mounted as `entry_tests` beside `tests`: `queue_tests.rs` is already at 815 lines, and `writer_tests`/`probe_tests` are the precedent for a second test module |
| `crates/melodia-store/src/database/queries/tests/playlist_tests.rs` (P1) | Block moves and the automatic cover |
| `crates/melodia-views/src/ui/now_playing/tests/up_next_tests.rs` (P1) | `play_order_move` and its agreement with the engine |
| `crates/melodia-views/src/ui/paged_rows/tests/queue_rows_tests.rs` (P2) | `QueueRows`' picks. Absorbs `queue_panel/tests/picks_tests.rs`, whose two cases test `QueueRows` rather than the panel, so one file states the model's contract |
| `crates/melodia-views/src/ui/paged_rows/tests/paged_rows_tests.rs` (P2) | Block moves, `show`'s kept `Arc`, `picked_rows` |
| `crates/melodia-views/src/ui/tests/drag_tests.rs` (new, P2) | `lifted_run` |
| `crates/melodia-views/src/ui/tests/list_selection_tests.rs` (P2) | The id lists' zero filter on every click |
| `crates/melodia-views/src/ui/window_chrome/file_drop.rs`, `ui/playlists/detail.rs` (P2) | The two seams: the window reads stay, the decisions they feed become pure |
| `crates/melodia-views/src/ui/window_chrome/tests/file_drop_tests.rs` (new, P2) | `route` and the targets' decision |
| `crates/melodia-views/src/ui/playlists/tests/playlists_tests.rs` (P2, P3) | P2: the playlist drop decision. P3: loses the origin pin that walk (d) supersedes |
| `crates/melodia-artwork/src/media/image/tests/cover_thumbs_tests.rs` (P2) | The backed tier's borrow on a scheduling miss, `resize`'s kept `settled` |
| `crates/melodia-app/src/services/tests/{settings,view_state}_tests.rs`, `crates/melodia-views/src/ui/tests/track_list_view_tests.rs` (P2) | `QueuePanelLayout` and `view_marks` against older files |
| `crates/melodia/tests/drag_mounts.rs` (new, P3) | Walks (a) to (e) |
| `crates/melodia/tests/file_drop.rs` (new, P3) | Walks (f) and (g) |
| `crates/melodia-views/src/ui/window_chrome/tests/winit_filter_tests.rs` (P3) | Pin (h) |
| `crates/melodia-views/src/ui/queue_panel/tests/queue_panel_tests.rs` (new, P3) | Pins (i), taking the `#[path]` `picks_tests.rs` gave up in P2 |

Both seams leave their files well under the 800-line cap (`file_drop.rs` 110, `playlists/detail.rs` 308 today).

## Phases

### Phase 1: The orders
Status: not started.

**1. `block_move`.** The one move rule the play order, the shown keyset and `playlist_items` share.
- **`move_block`, one case per partition:**
  - a single row moved down and up;
  - a contiguous run moved down and up;
  - scattered rows gathered at a slot;
  - a block landing at 0, and at the end (`to + n == len`, the last accepted value).

  Each keeps the moved items in their order.
- **Refusals**, each leaving the items untouched and answering `false`: empty positions, unsorted, duplicated, the last position one past the end, `to + n == len + 1`, a block already sitting at `to`.
- **`landing`** with the slot before a run, inside it, after it, and among scattered picks. One table pins the property `ListDropTarget` only draws: every slot inside or beside a run lands the run where it is, so `move_block` moves nothing.

**2. `QueueState`: entries and the splice** (`queue_entry_tests.rs`).
- **Entries.**
  - Two copies of a track get two entries.
  - An entry keeps its value through `move_rows`, `apply_shuffle_order`, `unshuffle`, a removal of other rows and a prune of another track.
  - `clear` never hands one out again: after a clear and a refill no entry equals one from before, and an entry from before finds no row.
- **`insert_next` and `insert_before`, as a decision table on where the block lands against the current row.**
  - Before it, at it and after it: the current index moves only when the block lands at or before it, and the playing id never changes.
  - Nothing current puts it at the front, and an empty queue takes it.
  - An empty block changes nothing, the version included.
  - Under shuffle, the original order takes the block at its end, so Play Next of three followed by unshuffle reads the three in the order handed over.
- **A drop after the queue moved** (Phase 7.8's race). One table, each row reading `entry_at` first, applying an edit, then `insert_before`.
  - The block lands directly ahead of its row after: a removal above it, a prune of another track, a `move_rows`, a shuffle, an unshuffle, an `insert_next`, and a second drop landing first.
  - `None`, and an entry removed, cleared or pruned, all land at the end.
- **`skip_to_entry`** plays its row after an insert above it and after a move, and answers `None` for a row removed, cleared or pruned. `entry_at(len)` is `None`, which is `queue_skip_to_index`'s whole early return.
- **Block moves round the current row:** a block holding it, a block passing it either way, and a block landing right before and right after it. The same row stays current, checked by its entry rather than its id.
- **Unshuffle keeps the playing copy of a track queued twice current**, where finding it by id took the first copy.

**3. `move_playlist_tracks`** (`playlist_tests.rs`).
- **Moves:** a contiguous run down and up, and scattered ids gathered in the playlist's order whatever order they were handed in. Positions read dense from 0 afterwards.
- **Boundaries and bad input:**
  - `to + n == len` is accepted and one past it refused.
  - A duplicated id in the input moves once.
  - One id outside the playlist among valid ones refuses the whole move and writes nothing.
- **The cover.**
  - An automatic cover follows the track that now leads; a custom one stays.
  - A move bumps `updated_at`: the fixture seeds an old value first, so no clock is read.
  - A move that moves nothing writes nothing.

**4. Up Next's `play_order_move`** (`up_next_tests.rs`).
- **A table:**
  - within the run before the repeat-all wrap, and within the run after it;
  - across the wrap from either side;
  - the boundary row, `base + row == len`, from both sides;
  - a queue with no repeat, nothing playing (base 0), and an empty queue.
- **The composition.** On a repeat-all `QueueState`, a move mapped through `play_order_move` and applied through `move_rows` leaves Up Next's slice reading as the shown rows did after `remove(from)` and `insert(to)`. The table pins the mapping; this pins that the mapping and the engine agree on "counted once it has left".

**Breaks that must fail:**
- `slot <= ci` turned into `<` in `insert_at`;
- `clear` no longer advancing `retired`;
- `prune_missing` compacting `tracks` again;
- `past_wrap` comparing with `>`;
- the move's `update_playlist_thumbnail_and_timestamp_tx` removed;
- `move_block` accepting `to + n == len + 1`.

**Gate:** fmt, clippy, test, then the breaks above. One commit.

### Phase 2: What the panel shows and keeps
Status: not started.

**1. `QueueRows`, over a real `QueueState`** (a queue of 1, 2, 1, 3).
- **Picking:**
  - a click lights one copy, and Ctrl on the other lights both;
  - a Shift range runs from the copy clicked (`click_selection` over a key other than `i32`);
  - `picked_tracks` counts rows and names each track once, in queue order;
  - `entry_at(len)` is `None`.
- **A pick stays on its row through every edit that keeps the row.** A table of `QueueState` edits, each shown through `show(play_ids(), play_entries())`: an `insert_before` above it, an `insert_next`, `move_rows` of the picked rows and of others, a shuffle, an unshuffle, a removal of another row, a prune of another track.
- **A pick goes with its row.**
  - Removing the picked row drops it.
  - A clear and a refill with the same tracks unstamps the rows whose ids stayed (`row_data(..).selected` is false).
  - A cleared panel shown against an empty queue keeps nothing.
- **A pick survives a hide:** `clear` followed by the same queue shown again.
- **`move_rows`** carries entries and picks with their rows.

**2. `PagedRows`.**
- A block move rereads only the rows it moved, the multi-row twin of `a_move_rereads_only_the_row_it_moved`.
- `show` with an equal keyset keeps the incoming `Arc` (`Arc::ptr_eq`), which is what `QueueRows::shows` relies on after a drag.
- `picked_rows` answers in display order and skips a selected id that isn't on show.

**3. `drag::lifted_run`:** no rows, one row, a run, a run starting at row 0, scattered rows.

**4. `compute_click_selection`:** no click ever picks a row listed as 0, plain and Ctrl included. The filter used to sit in the Shift branch alone.

**5. File drops.**
- **The seams.** `DropTargets::read` keeps the window reads and hands them to a constructor that decides: no queue target while the panel is hidden, no playlist under the floating card. `detail_on_screen` likewise hands what it reads to a pure function. Behaviour is unchanged.
- **`route`:**
  - inside each rect;
  - on its edges: the left and top edges are in, the right and bottom ones out;
  - over the floating card where both rects hold the point, where the queue wins;
  - a hidden queue lets the point fall to the playlist;
  - no playlist on screen lands nowhere.
- **The playlist decision**, as a decision table over Now Playing open, the miniplayer, the Playlists tab mounted, an id set and a smart playlist. Only the row where all five permit answers the id. The corners worth naming are a smart playlist with everything else open, and a valid id behind a hidden tab, which is where the old route dropped files.

**6. Cover tiers.**
- **A backed tier's `get_or_schedule_opt`:**
  - a cover the backing holds at its size is borrowed, painted on the frame that asked, and queues nothing;
  - one held at a stale size is scheduled;
  - one the backing lacks is scheduled.
- **`resize`:** to the cap the tier already has keeps `settled`, and to another clears it.
- **The dead borrow arm in `get_cached_opt` goes**, with the test that pins only it, and the never-decodes test moves onto an unbacked tier (Found while surveying).

**7. Persistence.**
- **`settings_tests.rs`:**
  - a `settings.json` from before the panel loads it closed at the default width;
  - the two keys sit at the top level as `queue_panel_open` and `queue_panel_width` and survive a round trip.
- **`track_list_view_tests.rs`** (over its `FakeList`):
  - columns saved and no marks, as an install from before the marks, gives every mark the view's default, Favorites' hidden `favorite_mark` included;
  - a marks map missing one mark defaults that one and keeps the saved ones;
  - a snapshot of the marks hydrates back to the same marks.
- **`view_state_tests.rs`:** a `views.json` with no `view_marks` key parses.

**Breaks that must fail:**
- `QueueRows::show` without `keep_queued_picks`;
- `PagedRows::show` assigning the keyset only when it differs;
- `contains` using a closed range;
- `DropTargets` ignoring `floating`;
- `get_or_schedule_opt` without its borrow;
- `resize` clearing `settled` unconditionally;
- `QueuePanelLayout` without `#[serde(default)]`;
- `hydrate` falling the marks back as one set.

**Gate:** fmt, clippy, test, then the breaks above. One commit.

### Phase 3: The cross-tree prohibitions
Status: not started.

Each walk owes the five things the root `CLAUDE.md` lists: a vacuity floor, an equality wherever a vanishing subtree is the failure, unreadable paths collected and asserted empty, comments stripped and scope brace-matched, and every exemption held to an exact count.

**`crates/melodia/tests/drag_mounts.rs`**
- **(a) `DragArea` is spelled only by `drag-source.slint`, and `DropArea` only by `list-drop-target.slint`.** Two exemptions, each held to an exact count: the queue button's, and the window-sized leaf at the back of `app-window.slint`. A `DropArea` drawn over a target eats its drops while answering `none` to every move.
- **(b) No `DragSource` is mounted inside a `for`.** A source in a row misses `drag-finished` once its row scrolls out or is rebuilt (Phase 4).
- **(c) No `DragSource` mount binds `enabled`.** The binding overrides the touch gate, and only a touchscreen would show it.
- **(d) The origins the `.slint` tree arms equal the origins Rust revokes, `none` aside**, each revoked at one site. A new reorderable list that forgets its revoke fails here. It supersedes `playlists_tests::the_playlist_list_names_the_origin_its_revoke_retires`, deleted in the same commit.
- **(e) No row dims its lift through `opacity`.** Each lifted row would become a texture of its own and crop Arabic marks, which no Latin review sees.

**`crates/melodia/tests/file_drop.rs`**
- **(f) `WindowRectReport` is mounted outside every `if` and `for` but the full-UI branch, in exactly two places.** A report in a branch that can drop is the `changed`-in-`if` panic.
- **(g) `FileDrag.target` has one writer, the winit filter:** `set_target` on `FileDrag` in `winit_filter.rs` alone, and no assignment in any `.slint` file. That is what keeps the two banners from disagreeing.

**Pins beside their anchors**
- **(h) `winit_filter_tests.rs`:** every press claims the queue's keys and disarms as a mouse, and a touch's start disarms as a touch, all outside every arm of the `match`, so no arm's early answer skips them. The touch half has no other check until a touchscreen is to hand.
- **(i) `queue_panel_tests.rs`, two pins:**
  - The panel's hover sentinel answers the wheel with `accept`. It reads as redundant, and without it a click in place after a wheel tick claims no keys.
  - Delete and Backspace act below the dialog guard and only on `queue-keys`, being destructive keys. The Escape chain's order is already `card_selection.rs`'s.

**Breaks that must fail:**
- a `DropArea { }` added to a grid;
- `enabled: true` bound on a grid's `DragSource`;
- the revoke removed from `show_queue`;
- a `WindowRectReport` wrapped in an `if`;
- `FileDrag.target` written from `queue-panel.slint`;
- `accept` dropped from the sentinel;
- `disarm_drag` moved into the `MouseInput` arm.

**Gate:** fmt, clippy, test, then the breaks above. One commit.

## Considered and left out

| Candidate | Why not |
|---|---|
| `min <= default <= max` on the panel tokens | A misordered token shows the first time the panel opens |
| Phase 3.6's header and band seating inside `Theme.content-min-w`, as arithmetic | The floors are measured per locale at run time, so a sum over source constants can't honestly say a row seats |
| `sidebar-yields`' terms, `Queue.floating` written beside `shown`, Up Next's source height, the edge band's quarter, the miniplayer's `menu-enabled: false`, the queue button's drop | Layout and one-file wiring a text pin can only restate; the hand passes own them (decision 4) |
| `effective-ids` as the only statement of the pick rule; no slot maths outside `ListDropTarget` | No fixed spelling: a walk would pin one spelling and pass every other |
| `RatingMenuRow` as the only `StarRating` in a menu | One mount per menu family, wrong on sight |
| `read_queue` deriving no entries while the order holds | Held by its two halves: the engine's `moving_the_index_alone_republishes_the_ids_already_shared` and Phase 2's kept-`Arc` pin |
| `queue_insert_before` and `queue_skip_to_index` | They take an `&AppState` no suite can build. Their logic is the engine's (Phase 1), and the read between is `existing_in_order`, which Add to Queue shares |
| `cover_size_for_content` before the panel's first report | A two-line read off the window; a wrong answer is a tier one step large until the first report retunes it |
| `nav_history::overlay_open`, `cross_tab_nav::land`, `view_tag`'s `+QP` | One-line reads off the window, covered by the Phase 2 and 3.5 hand passes |
| The winit fork's drag events | Only a compositor or Explorer can answer, and `winit` is excluded from the workspace, so its own suite never runs in the gate. Phase 7's Linux and Windows passes are its level |
| `PopupFit` and `PopupColumn`, the narrow-window folds | Layout; which popups can outgrow the window is a judgement with no spelling to walk |

## Found while surveying

- **`CoverThumbs::get_cached_opt`'s borrow arm has no caller left; the lookup itself is live.**
  - **Its three callers read tiers that own their size.** `shell::bridge` (twice) reads the row tier boot builds with `CoverThumbs::new()`, and `radio::covers::logo_cover` reads Radio's `with_config` tier on a generation of 0. For both, `borrow_from_backing` returns `None` at its first line, so the arm always falls through to the placeholder.
  - **The one backed tier never takes this lookup.** The queue panel's is built in `queue_panel/mod.rs` and reached only through `get_or_schedule_opt`, `clear` and its notifier.
  - **History.** The arm came in with `ab03e9e1` for the sheet's cold-tier branch (`generation == 0` took `get_cached_opt`). `4c7794df` on this branch replaced that branch with `get_or_schedule_opt`, which borrows first on its own.
  - **Recommendation:** in Phase 2, delete the arm and its doc sentence ("A backed tier still borrows on a miss…"), and delete `a_cache_only_lookup_on_a_backed_tier_borrows_on_a_miss`, which pins only the arm and whose doc still names the queue panel. **Retarget rather than delete `…_still_never_decodes`** onto an unbacked tier. It is the only test that a cache-only miss answers the placeholder without decoding, which the bridge depends on, running on the event loop; every other `get_cached_opt` test reads a warm entry. `borrow_from_backing` stays, the decoding path and the scheduling lookup both calling it, and no corpus walk counts this lookup.

## Verification

- **Every phase:**
  - `cargo fmt --all --check`;
  - `cargo clippy --all-targets --locked --workspace -- -D warnings`;
  - `cargo test --locked --workspace`;
  - each listed break applied by hand, the named test watched to fail alone, then reverted.
- **No release build or RSS gate:** nothing here reaches the binary but the two seams, which move code without changing what it does.
