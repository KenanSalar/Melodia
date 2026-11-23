# Plan: drag queue rows into an open playlist (#181)

Status: **approved** · Owner: Kenan · Created: 2026-11-22 · Issue: #181 · Branch:
`feat/queue-drag-to-playlist`

## What you see today

- **The queue panel sits beside an open playlist.** A playlist's rows drag into the queue: the
  queue's header turns into "Add 3 tracks to the queue", a line shows the slot, and the tracks land
  there.
- **The other direction does nothing.** A queue row you pick up only reorders the queue. Carried
  over the playlist, the chip follows the pointer, but no line appears, the cursor refuses, and
  letting go does nothing.
- **The only route is the row menu's Add to Playlist.** It opens a dialog where you find the
  playlist by name, shows a toast, and always puts the tracks at the end.

## Why

Three things keep a queue drag inside the queue, and the page has nothing that would take it.
Phase 1 handles the first two:

1. **Queue rows carry no tracks.** A press arms the drag with `src.arm(idx)` (`queue-panel.slint`,
   the row's `pressed`), so `Drag.payload.items.kind` is `""`. Every target that takes another
   list's drag requires a kind: `ListDropTarget.items-drag`, the bar's `QueueButton`, and the
   `DragGhost` mount in `app-window.slint`.
2. **The queue's source only allows a move.** Its `DragSource` sets `allow-move: true` and nothing
   else. Slint turns a target's `copy` answer into `none` when the source doesn't allow copy.
3. **The playlist's drop target only takes its own rows.** In `track-list.slint` the
   `ListDropTarget` is `enabled: root.moves-rows` and never sets `takes-items`.

The store can only append, too: `queries::playlist::append_items` starts at `MAX(position) + 1`.

Two hazards appear as soon as queue rows carry ids, and Phase 1 handles both:

- **The picked ids can change mid-drag.** `Queue.selected-ids` is a single `VecModel`.
  `list_selection::write_selection_ids` rewrites it in place each time `publish_picks` runs, which
  happens whenever the queue moves. A payload holding that model would change under the drag, and
  the issue requires the tracks that were picked up.
- **The queue would re-add its own rows.** `Drag.revoke` resets `origin` to `none` and keeps the
  items, so a revoked queue drag would read as another list's drag, and the queue would insert
  copies of its own rows.

## What you'll see after

- **Dropping queue rows on an open playlist adds them there.** A drag from a picked row takes every
  pick, in queue order, and a track queued twice is added once. The queue and what's playing don't
  change.
- **In the playlist's own order** (sorted by position ascending, no filter), the reorder line shows
  where the tracks will land, and letting go below the last row adds them to the end.
- **Sorted by another column or filtered**, no line is drawn and the tracks go to the end.
- **While the drag is over the list, the column header row reads "Add 3 tracks to this playlist"**,
  the same way the queue's header changes for drags going the other way.
- **Tracks the playlist already holds are skipped.** A smart playlist refuses the drop, and an
  empty playlist takes it.
- **Narrow windows refuse it.** The queue floats over a dimmed page there, and that page takes
  nothing.

## Decisions

1. **The track list takes the drop, empty state included, and the hero band doesn't.** Over the
   band there is no line to aim at, and the band is `LibraryTabBand`, shared by every My Library
   tab. Making it a target adds a fourth `DropArea` file, which `drag_mounts.rs` holds at three. The
   label sits on the column header row, directly above the rows the line runs between. File drops
   keep covering the whole page, since an OS drop has no slot to point at.
2. **Notify only when the result doesn't show itself.** When every dragged track lands at the line,
   the rows appearing there are the confirmation. Otherwise, Add to Playlist's completion toast
   appears (`Settings.add-to-playlist-title` / `-message`): a Warning when some tracks were already
   in the playlist, a Success when a sorted or filtered list took them at the end. As with Add to
   Playlist, there is no Undo, and the added rows can be removed like any others.
3. **The dragged ids are copied at the press.** A new `Queue.picked-ids()` callback returns a fresh
   model built from `QueueRows::picked_tracks`, deduped by track and in queue order. A single row
   carries `[row.id]`, which is fresh already.
4. **A revoked drag keeps its origin.** `revoke` moves `index` to `-1` and clears the picks, but
   keeps `origin` and the items. A list then still recognises its own drag, and an own drag whose
   index is `-1` moves nothing. That matches today's outcome on every list, and it stops the queue
   (or a sorted playlist) from taking its own revoked rows as an insert.
5. **Rust decides between the slot and the end.** Slint draws the line only while the list is in
   its own order. The handler checks the same guard that `apply_optimistic_move` checks, plus
   `shows_own_order`, which Slint can't see, and appends wherever that guard fails.

Prior art: neither Strawberry nor fooyin supports queue-to-playlist dragging. Their queues put bare
row numbers in the drag, which their playlists refuse. What carries over is the playlist side.
Strawberry splices a drop into its list in one insert (`playlist/playlist.cpp`
`InsertItemsWithoutUndo`). fooyin carries track values captured at drag start
(`playlistmodel.cpp` `mimeData`). Not to copy: both queues resolve rows by index at drop time, and
Strawberry draws a line under auto-sort and then places the rows elsewhere.

## Structure

| File | Owns after this |
|---|---|
| `ui/globals/drag.slint` | the payload; `revoke` keeping the origin |
| `ui/components/list-drop-target.slint` | own-row moves gated on `in-order` and an armed index; `draws-line` for the line, `aims` for the edge scroll |
| `ui/components/drop-label.slint` (new) | `DropLabel`: the icon and elided text naming what letting go does |
| `ui/views/queue-panel/queue-panel.slint`, `queue-row.slint` | rows arming their tracks; `allow-copy`; the header mounting `DropLabel` |
| `ui/globals/queue.slint` | `picked-ids()` |
| `ui/components/now-playing/queue-button.slint` | refusing a drag that came from the queue |
| `ui/components/track-list/track-list.slint` | `takes-items`, `drop-label`, `dropped-items`, `holds-items`; the header slot showing `DropLabel` while it holds items |
| `ui/views/my-library/playlist-detail.slint`, `ui/globals/playlists.slint` | the gate (queue origin, not smart, not floating), the label text, the empty state giving way, `PlaylistDetail.drop-in` |
| `melodia-views/src/ui/queue_panel/callbacks.rs` | `on_picked_ids` |
| `melodia-views/src/ui/playlists/detail.rs` | `slots_are_positions`, extracted from `apply_optimistic_move` |
| `melodia-views/src/ui/playlists/callbacks/detail.rs` | `wire_drop_in`: the smart gate, choosing the landing (`Landing`), the write, the refresh, the toast |
| `melodia-views/src/ui/playlists/callbacks/mod.rs` | wiring `wire_drop_in` from `wire_files`, which holds the `NotificationsUi` that `ViewCtx` lacks |
| `melodia-app/src/library/playlists.rs` | `insert_into_playlist` beside `add_to_playlist` |
| `melodia-store/src/database/queries/playlist.rs` | `place_items`, which both the append and the insert go through; `items_in_order` and `position_writes`, shared with `move_items` |
| `packaging/install-linux.sh` | a closing uninstall hint that names only `uninstall-linux.sh` |
| `ui/components/progress-trace.slint` (new) | `ProgressTrace`: a share of the way round the pill inscribed in its box, a circle in a square one |
| `ui/components/now-playing/play-button.slint` | the hold's ring, through `ProgressTrace` |
| `ui/components/settings/section-button.slint` | an `@children` layer over the button; `ink` read out |
| `ui/components/dialog/notification-card.slint` | the share of its clock left; the trace round a timed card's button |

UI paths are under `crates/melodia-ui/`. Every file stays well under 800 lines.

**Duplication collapsed first, each step in the phase that adds its second use:**

- **The queue's drop label becomes `DropLabel`** (`queue-panel.slint`, the `if drop.holds-items`
  row), mounted in the queue before the track list mounts it too.
- **`append_items`' MAX-and-renumber becomes `place_items`.** It reads the playlist's rows once,
  splices the new tracks in at the end or before a row, and writes only the positions that change.
  The append and the insert are then one routine, so the issue's requirement that a large drop be as
  fast as Add to Playlist holds by construction.
- **`apply_optimistic_move`'s order guard becomes `slots_are_positions`,** used by both the reorder
  and the drop.
- **The play button's hold ring becomes `ProgressTrace`**, mounted there before the Undo button
  mounts it too.

## Phase 1: queue rows carry their tracks **[done]**

**Keeps working:** reordering the queue by dragging one row or a set of picks; dragging tracks and
cards from the rest of the app into the queue; Up Next's reorder; reordering a playlist's own rows;
file drops. A queue drag interrupted by a queue change (the track ending, a row removed) still moves
nothing when dropped back on the queue.

Then the swap:

- **`drag.slint`:** `revoke` keeps `origin` and `items` and resets `index` and the picks. The
  `DragPayload.index` doc says `-1` once revoked.
- **`list-drop-target.slint`:** an own drag counts as a reorder only while `index >= 0`, so
  `can-drop` refuses a revoked one. `items-drag` still excludes the list's own origin.
- **`queue.slint` + `queue_panel/callbacks.rs`:** `callback picked-ids() -> [int]`. Its doc gives
  the reason: `selected-ids` is rewritten in place under a live drag. Rust answers with
  `ModelRc::new(VecModel::from(...))` over `rows.picked_tracks()`.
- **`queue-row.slint` / `queue-panel.slint`:** the row builds its `DragItems` the way
  `TrackListRowItem.drag-items()` does: kind `"track"`, the picks' copied ids or `[row.id]`, the
  title, and the cover from `Queue.request-cover`. The press calls `src.arm-items`, and `src` gains
  `allow-copy: true`.
- **`queue-button.slint`:** refuses a payload whose origin is the queue. Otherwise it would append
  a second copy of rows already queued. The header comment says so.

**What you'll see:** a queue row dragged out of the panel now shows the chip with its cover, plus
a count for picks, as other lists' drags do. Nothing takes the drop yet, and the bar's queue button
ignores it.

**Gate:** `cargo fmt --all`, the workspace clippy gate, `cargo test --locked --workspace`, then the
`code-style.md` comment audit.

## Phase 2: the store inserts at a slot **[done]**

**Keeps working:** Add to Playlist from any row menu, file drops onto a playlist, and New
Playlist… (`create_playlist_with_tracks`, left untouched). Tracks still land at the end; a track
already in the playlist, or lost from the library, is skipped; repeats land once; a playlist that
takes nothing keeps its `updated_at`; the toast counts as before.

Then the swap:

- **`queries::playlist::place_items(tx, playlist_id, track_ids, before)`:**
  - Reads the playlist's rows in `(position, id)` order.
  - Drops what `present_in_order` drops, plus tracks already in the playlist and repeats after the
    first.
  - Returns before any write when nothing is left.
  - Otherwise splices the rest in at `before`, clamped to the row count, or at the end. It writes
    the moved rows through `batch_update_positions_pairs`, the new ones through `insert_items_tx`,
    then stamps.
  - Positions come out contiguous, which closes a gap a deleted track left, as the renumber did.
- **`append_items`:** goes, `add_tracks_to_playlist` calling `place_items` at the end. Its
  `MAX(position)` read and its renumber go with it; `renumber_playlist_positions_tx` stays for its
  other callers. The rows read moves into `items_in_order`, which `move_items` shares.
- **New `insert_tracks_into_playlist(db, playlist_id, track_ids, before: usize)`** beside
  `add_tracks_to_playlist`, plus `library::playlists::insert_into_playlist`, which mirrors
  `add_to_playlist`.
- **One transaction, so the playlist is never seen half shifted.** The page reads it back once,
  after the commit.

**What you'll see:** nothing new; every add behaves as before.

**Gate:** as Phase 1. The existing append tests in `playlist_tests.rs` must pass unchanged, and any
that break get fixed in this phase.

## Phase 3: the playlist takes the drop **[done]**

- **Extract `DropLabel`** from the queue header's drop row and mount it there first (see Structure).
- **`list-drop-target.slint`:**
  - New `in property <bool> in-order: true`. An own drag moves only while it holds.
  - New `draws-line`: `moves || (items-drag && in-order)`, read by the line.
  - New `aims`: `reorders || (items-drag && in-order)`, read by the edge-scroll `Timer`, so a list
    without a line doesn't scroll toward a slot nothing will use. It isn't `draws-line`, which is
    false inside an own drag's lifted run: a run taller than the list would stop scrolling.
- **`track-list.slint`:**
  - New `takes-items`, `drop-label`, `callback dropped-items(int)` and `out holds-items`.
  - The drop target gets `enabled: moves-rows || takes-items` and `in-order: moves-rows`.
  - The header slot hides `TrackListHeader` and shows `DropLabel` while `drop.holds-items`, the way
    the queue does: hidden rather than unmounted, so nothing rebuilds as a drag passes through.
  - The header comment covers the new inputs.
- **`playlist-detail.slint`:**
  - Gate: `takes-items: Drag.payload.origin == DragOrigin.queue && !PlaylistDetail.playlist.is_smart && !Queue.floating`.
  - Label: `@tr("Add {n} track to this playlist" | "Add {n} tracks to this playlist" % ...)` over
    `Drag.payload.items.ids.length`, with a `// Translators:` line above it.
  - `dropped-items` calls `PlaylistDetail.drop-in(ids, slot)`.
  - The empty state hides while `tl.holds-items`: it is opaque and covers the header row the label
    takes.
  - The `reorder-enabled` binding stays word for word, since `playlists_tests.rs` pins it.
  - `!Queue.floating` is needed because the scrim's `TouchArea` passes drag events through to the
    dimmed page.
- **Rust:**
  - Extract `slots_are_positions` in `playlists/detail.rs`. `playlists_tests.rs`' two guard pins
    follow it there, and check that `apply_optimistic_move` still asks it.
  - `wire_drop_in` in `playlists/callbacks/detail.rs`, wired from `callbacks::wire_files` for the
    `NotificationsUi` the toast needs:
    - Refuses when no playlist is open or it's smart. That is the drop's Rust-side `is_smart` gate;
      the Slint one is `takes-items`.
    - Calls `insert_into_playlist` at the slot where `slots_are_positions` holds, and
      `add_to_playlist` otherwise (`Landing`).
    - Awaits the write in `slint::spawn_local(Compat)`, as `add_picker` does.
    - Then runs `refresh_after_edit` and shows the toast where Decision 2 calls for it
      (`Landing::notice`). A drop of tracks the playlist all holds writes nothing and toasts the
      Warning with Add to Playlist's zero counts.
  - Update the file's `//!` header.
- **Translations:** run `scripts/update-translations.py`, then fill all 15 catalogs, plural forms
  included: no empty `msgstr` and no `#, fuzzy`. `DropLabel` elides, so longer labels fit.
- **Docs:**
  - `.claude/rules/library-data.md`: the `is_smart` gate list gains the queue drop (Slint
    `playlist-detail.slint`, Rust `wire_drop_in`).
  - `.claude/rules/ui-patterns.md`: the Lists catalogue entry gains `DropLabel`.
  - `README.md`'s drag-and-drop feature line says queue rows drag into a playlist.
- **`packaging/install-linux.sh`, unrelated to the drag:** its closing hint offers
  `rm -rf $INSTALL_DIR` as an alternative to `./uninstall-linux.sh`. `$INSTALL_DIR` defaults to
  `$XDG_DATA_HOME/Melodia`, which is the data folder, so that command deletes the library along
  with the program. The hint names only the uninstall script, which keeps the data and prints how
  to wipe it on purpose.

**What you'll see:** everything under "What you'll see after", and the tarball installer's last
lines offering only `./uninstall-linux.sh`.

**Gate:** as Phase 1, plus `translations.rs`, `msgfmt -c` and `bash -n packaging/install-linux.sh`
passing.

## Phase 3.5: the Undo button counts down **[done]**

Unrelated to the drag, shipped on this branch.

**What you see today:** a removal raises a notice with Undo and ✕ at its right, and nothing says
how long it stays. It goes after the time set in Settings ▸ Accessibility ▸ Undo, taking Undo with
it. Its clock stands still while the pointer is over the card, while the card is dragged and while
the window is in the background, and runs the whole time again when it resumes. A second removal
merged into the card starts it again too.

**Decisions:**

1. **A trace inside the button's edge, as the play button's ring sits inside its disc, and no
   seconds.** A ring ahead of the label would widen the button by 22 px, taken from the card's
   message, and read as an icon. The trace takes no room, which seconds would.
2. **The trace shows the clock as it is: full whenever the clock stands still.** A resume runs the
   whole interval again, so a trace frozen part way would promise time the clock doesn't keep.
3. **One `ProgressTrace`, extracted from the play button, holding the drawing and no timing.** It
   traces the pill inscribed in its box, a circle when the box is square, so the hold's ring and
   the Undo button's trace are one drawing. The hold fills it after waiting out a click; the card's
   clock empties it, stopping and restarting. Each host keeps its own timing.
4. **Stepped by a `Timer`, not bound to `animation-tick()`.** The window redraws at the step rather
   than every frame for the notice's whole life, and a leaving card's trace holds where it stood
   with nothing to freeze it: a binding on the clock would snap it full mid-fade. The step is the
   Equalizer overlay's 33 ms, which the hand test read as a glide at the shortest setting. It can't
   follow the button's width: a `Timer`'s `interval` may not read a layout property.
5. **Any timed card's button gets it, not the undo kind.** Undo is the only one today, every other
   card with a button being sticky, and a timed one's button goes with its card for the same reason.

**Keeps working:** the play button's hold ring in the bar and the miniplayer, drawn where it was;
every other `SectionButton`, which passes no children; the card's enter, lift, swipe and leave, its
clock's pauses, and Ctrl+Z. `mount_fade_tests.rs` pins the card's `init` to its three faded layers,
so the clock's start is seeded from `enter` and nothing new is animated.

Then the swap:

- **`progress-trace.slint` (new):** `ProgressTrace`, taking `progress`, `stroke` and
  `stroke-width` (2 px). The play button's `if progress > 0: Path` moves into it, its arc grown
  into the pill inscribed in the box: clockwise from the top centre through five pieces (top-right
  straight, right cap, bottom, left cap, top-left straight), each drawn once the trace reaches it.
  In a square box the straights are empty and it is the play button's circle. No arc meets its own
  start, so the cap short of a full turn goes. Slint fits a path inside its stroke
  (`fitted_path_events`, `items/path.rs`), so the box is the trace's outer edge.
- **`play-button.slint`:** mounts it in the disc, sized `(disc − stroke) × 0.76 + stroke`: the
  radius-38 arc sat at 38 % of the disc less the stroke, so the ring draws where it did.
  `ring-delay` and `ring-progress` stay.
- **`section-button.slint`:** `@children` after the label row, over the whole button, each child
  setting its own `x` and `y` to stay out of the button's size. `ink` becomes an `out property`, so
  a child draws in the button's ink, fade and dim included.
- **`notification-card.slint`:**
  - The clock's `running` expression becomes a `ticking` property, read by the clock, the step and
    one `changed` handler.
  - `clock-from`, the tick the clock started at, and `clock-left`, the share left, 1 at rest.
  - A `drain` Timer at 33 ms, running while `ticking`, sets `clock-left` from `animation-tick()`
    against `clock-from` and the clock's interval, clamped.
  - `changed ticking` re-seeds `clock-from` and refills `clock-left` unless the card is leaving.
  - The clock's `triggered` empties the trace before it dismisses. A bump's `restart()` re-seeds
    and refills. `enter` seeds `clock-from` at mount, where `changed` doesn't fire.
  - The action button takes an id and, on a timed card, a `ProgressTrace` in its `ink`, inset 4 px:
    whole pixels, so its straights don't blur across a pixel row.
- **Docs:** `.claude/rules/ui-patterns.md`'s Inputs entry gains `ProgressTrace` beside
  `HoldTouchArea`. `README.md`'s undo line says the trace round the Undo button counts down.

No Rust and no `@tr` string, so no catalog work.

**What you'll see:** a line traced just inside the Undo button's edge, starting as a full outline
and shrinking back, clockwise, to the top centre over the notice's time; the notice goes as it
empties, and the button keeps the width of its label. Over the card, dragging it, or with the
window in the background, the trace sits full, and a merged removal fills it again. A card closed
by Undo, Ctrl+Z or ✕ fades with its trace where it stood. The play button's hold ring looks as it
does today.

**Gate:** as Phase 1.

## Phase 4: Tests **[pending]**

The hand tests for Phase 3 and Phase 3.5 passed on 2026-11-23. The tests wait for your go, naming
them.

## Verification

- **Static gates every phase:** fmt, clippy over the workspace, the whole test suite, and the
  comment audit. `drag_mounts.rs` should still pass untouched: no new `DropArea` and no new
  origin.
- **The hand test is yours** (`cargo run -p melodia`), or through the melodia-ui MCP tools when
  `cargo mcp` is up. Phase 3's list, from the issue:
  - one row
  - picks spread across the queue
  - the same track queued twice
  - tracks the playlist already has
  - an empty playlist
  - a playlist sorted by title, and one filtered
  - a smart playlist
  - a long playlist scrolling while the drag sits at its edge
  - the playing track changing mid-drag
  - a narrow window with the floating queue
  - a locale with longer labels (German)
  - a few thousand picks against Add to Playlist with the same set
- **Phase 3.5's list:**
  - the shortest and the longest setting, the trace's end gliding rather than stepping
  - a locale with a long Undo label (German), the pill and its trace widening together
  - the pointer resting on the card, then leaving it
  - a drag that eases back, and a fling
  - the window losing focus mid-count
  - a second removal merging into the card
  - Undo, Ctrl+Z and ✕ mid-count
  - the play button's hold in the bar and the miniplayer
- **Re-check the "keeps working" list after each phase.**

## Risks

- **Every drag-out now shows the chip** over the rest of the window, as track-list drags already do.
- **The `revoke` change touches the queue, Up Next and the playlist.** The armed-index guard is
  what keeps their outcomes as they are today.
- **`QueueButton` is the one target that ignores origin.** Phase 1 gives it its check. A future
  drop target that takes items owes the same check.
- **An undo notice redraws the window at the trace's step while its clock runs**, where a resting
  card drew nothing. The step trades those redraws against how far the trace's end jumps at each.
