# Plan: Queue panel (the queue docks on the right and takes drops)

Status: **in progress**, Phases 1 to 7 done (Phase 6's touch checks deferred, Phase 7 still to validate on Windows), Phases 7.5 and 7.6 implemented and waiting on their hand passes, Phase 8 next · Owner: Kenan · Created: 2026-10-21 · Issue #136, branch `feat/queue-panel`

> **Validated 2026-10-21** against `72a0e74f` (`feat/queue-panel`). Three read-only traces went into it: the queue sheet, the left sidebar's resize and fold, and drag and drop with Add to Queue. They were checked against:
> - the Slint 1.18.1 sources for `DragArea`, `DropArea` and the in-window drag (`i-slint-core/items/drag_n_drop.rs`, `window.rs`);
> - the vendored winit fork beside upstream's positioned drag API;
> - two open-source desktop players' queue panels, as prior art.

Working doc. Delete when every phase has shipped and passed by hand.

## Context

**What you see today.**
- **The queue covers the page.** It opens as a sheet that slides up into the middle of the content panel and dims the page behind it (`views/queue-sheet.slint:153-188`). A click anywhere outside it closes it (`:158-164`). Checking something in the library means closing the queue, browsing, and opening it again. You can't watch what's coming up while you look around.
- **Nothing can be dragged into the queue.** The right-click menu is the only way in, and nothing in the app can be dragged out of its own list.
- **A file drop ignores where it lands.** A file dragged in from the file manager goes to the queue whenever the sheet is open, wherever it is let go (`window_chrome/drop_coalescer.rs:104-139`).

**Why.**
- **The sheet is an overlay inside the content panel** (`app-window.slint:599-604`), and everything around it assumes it covers the page:
  - It owns Escape and Ctrl+A while it is open (`layout/shortcut-scope.slint:87, 106-114, 189-192`).
  - Mouse-4/5 history and the composite views' wheel routing stand down while it is open (`nav_history::overlay_open`, `nav_history.rs:509-513`).
  - Go to Album, Go to Artist and Go to Genre close it (`callbacks/cross_tab_nav.rs:105-109`).
  - The file drop routes on its open flag alone (`drop_coalescer.rs:28-40`).
- **The miniplayer keeps the sheet's rows and covers resident while they are hidden.** The sheet unmounts with the full UI, but `Queue.open` stays true (`components/mini-player-switch.slint:85-104`).
- **The app has two drags, and both are hand-written copies confined to their own list.** The queue reorder (`views/queue-row-item.slint:58-238`) and the playlist reorder (`components/track-list/track-list-row.slint:80-114, 238-299`) are separate `TouchArea` gestures. Each one's comments say "keep the two in step".
- **Ctrl+A exists only for the queue.** Every other surface's Ctrl+A is rejected (`shortcut-scope.slint:213`), which the issue assumed otherwise.

**Outcome.**
- **The queue becomes a right-hand panel in the same row as the sidebar and the content.**
  - It pushes the content over with the sidebar's animation and timing. Closed means gone, with no rail.
  - Its width is set by dragging its inner edge, and a double-click resets it. The width is remembered across restarts, and so is whether it was open.
- **The sidebar, the content and the player bar all stay live while it is open.**
- **Tracks and cards drag in.**
  - Tracks come from any track list; a picked row brings the whole pick with it.
  - Albums, playlists, artists and genres come from the grids, as their tracks, in Add to Queue's order.
  - A drop lands where it is let go, at the same line reordering shows today. Below the last row means the end.
  - While something hovers over it, the panel says what letting go will do.
- **Files from the file manager land where they are let go.** Over the panel they go to the queue; over an open playlist they go to that playlist.
- **Under Now Playing the panel steps aside**, and Up Next there gains the queue's right-click menu and drag reorder.
- **Kept:** everything the sheet does today.
  - The header: title, track count, and Clear queue, which still asks first.
  - Double-click to play; the playing row stays highlighted.
  - Drag reorder with edge scrolling.
  - Picks with click, Ctrl, Shift and Ctrl+A; the right-click menu acts on what is picked.
  - The favourite heart on each title, and the row's remove button, which shows in the duration's place on hover. Rating moves to the right-click menu (Phase 3.5).
  - Opening scrolled to the playing track.
  - The empty state.
  - Go to Album, Artist and Genre.
  - What the queue holds and how playback walks it are unchanged.
- **Removed:**
  - the scrim, the outside-click close and the slide-up card;
  - `Queue.dismiss()` and the empty `close` callback;
  - the four `Queue.open` gates that existed because the sheet covered the page;
  - both hand-rolled reorder gestures, replaced by one drag mechanism;
  - the k-times `Vec::insert` loop behind Play Next of k tracks.

## Decisions settled here

1. **The panel sits in the sidebar's row, after the content** (`app-window.slint:493-622`).
   - **Width.** It is bound as `min-width: 0` / `preferred-width` / `max-width`, never as a bare `width`. A component root's bound width is reported as both min and max, and an eased one would drag the window's own floor along with it (`slint-pitfalls.md`, the animated-width entry; the sidebar's `width` at `layout/sidebar.slint:63-67` is that shape, masked by the window's explicit `min-width`).
   - **Animation.** It eases on `Theme.dur-med` `ease-in-out`, as the sidebar does, and the ease is off while the edge is being dragged.
   - **Look.** It is painted in `WindowChrome.mantle` like every chrome surface (`desktop-shell.md`). That keeps `RoundedFrame`'s corner masks correct, since what now shows beside the content panel is still the mantle.
2. **Now Playing: the panel steps aside** (Kenan, 2026-10-21).
   - **One flag decides whether it shows.** `Queue.shown = open && full UI mounted && !Nav.now-playing-open`, and that flag also drives the rows, the covers and the drop routing.
   - **The queue button and Q while Now Playing is up close Now Playing**, so the panel appears beside the library. The button reads `shown`.
   - **Up Next in Now Playing gains the queue's row menu and drag reorder.** It reuses `Queue`'s existing callbacks, as its click already reuses `Queue.skip-to` (`components/now-playing/up-next-list.slint:173-179`).
   - **The miniplayer's Up Next stays click-to-skip**, since the issue keeps the miniplayer without a queue. One `editable` input on `UpNextList` decides it.

   **Amended in Phase 7.6: the miniplayer's Up Next reorders too.** `menu-enabled` replaces `editable` and keeps only its menu off, the sheet having no room for one.
3. **The panel opens again on launch if it was open at exit** (Kenan). Fresh installs start closed. The state lives in `settings.json` beside the sidebar's, written at exit (`crates/melodia/src/shutdown.rs:33-52`).
4. **Escape and Ctrl+A** (Kenan).
   - **Escape clears the queue's picks when the queue was clicked last, and never closes the panel.** Esc anywhere else is unchanged: close Now Playing, then clear the live selection.
   - **Ctrl+A selects the whole queue when the queue was clicked last.** Anywhere else it stays rejected, as today.
5. **"Clicked last" is a press inside the panel, read at the press by the winit filter.**
   - **Where it comes from.** A hover sentinel over the panel publishes `Queue.pointer-in`; the `CompositeScroll.hovered` sentinel is the same shape (`views/browse-view.slint:138-141`).
   - **What reads it.** The filter's `MouseInput { Pressed }` arm writes `Queue.keys-owned` from it, and the focus regrab follows `keys-owned` rather than `open` (`app-window.slint:280-283`).
   - **Why not Slint alone.** A `TouchArea` never sees a press its child took, and `ShortcutScope` has to stay the only `FocusScope` (`ui-patterns.md:1152-1157`).
6. **One drag mechanism: Slint's `DragArea` and `DropArea`.** The source settles the following:
   - **It is stable, not behind an experimental gate.** `typeregister.rs:552-558` strips only `ComponentContainer`, and `check_drag_area.rs` is a lint pass, not a gate.
   - **The drag stays inside the window.** No backend `start_drag` exists in `i-slint-backend-winit-1.18.1`.
   - **A drop survives its source row leaving the view.** `drag_data` is held by the window, and the source's weak ref feeds only the drag image and `drag-finished` (`window.rs:870-932, 1888-1919`).
   - **A `DropArea` lets clicks and wheel through, and one answering `none` passes the event on** (`drag_n_drop.rs:330-366`).
   - **The runtime rewrites the release into a `Drop` or an `Exit`**, so no stray click lands after a drag.

   Extending the hand-rolled gesture was the alternative. Every target would need the source's geometry and its own no-click guarantee, both of which the runtime already gives.

   The cost is that a `DragArea` takes the grab at 8 px (`flickable::DISTANCE_THRESHOLD`) and sends its children a cancel. So a row can't host both mechanisms, and the two reorders move onto it, which deletes both copies. Two side effects:
   - `TouchArea`s freeze hover during a drag (`input_items.rs:94-97`).
   - A `Flickable` ignores `DragMove` (`flickable.rs:256, 791`), so edge scrolling becomes a `Timer` in the target. That also fixes today's auto-scroll, which stops when the pointer rests (`queue-row-item.slint:228-235` nudges only on a move).

   **Amended in Phase 4: the `DragArea` sits on the list or grid host, round its `ListView`, and never in a row.**
   - **Why not in a row.** The runtime reports `drag-finished` only to a source that still exists, and a `ListView` destroys a row rather than recycling it. So a source in a row misses the end of any drag whose row scrolled out, was reset under it, or was moved by the reorder itself.
   - **What follows.** The rows and cards report which of them was pressed. The cost above holds per list rather than per row.
   - **A `DropArea` answering `none` passes moves on, but not a drop.** One drawn over another takes its drops (Phase 4, fact 3).
7. **The drag payload is what Add to Queue takes: `(kind, ids)`.**
   - **Where it lives.** It sits in a `Drag` global, written by the source when its `dragging` rises. The `data-transfer` is a token Rust sets once, because Slint can only construct one through the host (`builtin_elements.rs:1815-1817`).
   - **Which ids.** The menu's rule decides them: `row-selected && count > 1 ? selection : [row]` (`track-context-menu.slint:50, 143-144`; `grid/card-cell.slint:54-55`). The rule is lifted into one pure function that both menus and both drag sources call.
   - **How a drop resolves.** It calls `card_actions::spawn_with_track_ids` (`callbacks/card_actions.rs:147-179`), so a dropped card resolves exactly as Add to Queue on it, in the order each kind already uses (`queries/track/entity_ids.rs`).
8. **Insert at a slot is one splice, following `insert_next`'s rule** (`engine/queue.rs:165-173`).
   - **The rule.** Play order takes the block at the slot, original order takes it at the end, and the current index shifts when the slot is at or before it.
   - **Play Next joins it.** `queue_play_next_many` (`library/queue.rs:117-143`, k `Vec::insert`s under the state lock) becomes one `insert_at(current + 1)`.
   - **The only visible difference.** Unshuffling restores a Play Next block in its own order rather than reversed.
9. **A file drop is routed by where it is let go, which needs the pointer during an OS drag.**
   - **What winit reports.** winit 0.30 reports none on any backend. Upstream's next major reports a position on enter and on every move, on all three (`winit-win32/src/drop_handler.rs` `DragEnter`/`DragOver`; `winit-x11/src/event_processor.rs` `DragMoved`).
   - **The fork patch.** The fork gains the same events on Wayland (its own data-device code discards `Motion`'s x/y, `winit/src/platform_impl/linux/wayland/seat/data_device/mod.rs:212`), X11 and Win32. They are named as upstream names them, so retiring the fork stays a deletion.
   - **Slint still builds against it.** Slint's backend matches `WindowEvent` with a wildcard (`i-slint-backend-winit-1.18.1/winitwindowadapter.rs:1599`).

   **Amended in Phase 7: the names are `0.31.0-beta`'s, not `master`'s.** Upstream `master` has since replaced these events with ID-based ones whose paths are fetched separately (PR #4571), so no copy keeps retiring the fork a deletion. The fork takes the shape PR #4079 shipped in the betas, one event per set of files.
10. **Narrow windows give way in a fixed order, decided by one constant, the content floor.**
    1. The sidebar renders as its rail. This is not persisted, and `Nav.sidebar-collapsed` stays the user's.
    2. The panel narrows toward its minimum.
    3. The panel steps aside, through `shown`, exactly as under Now Playing.

    **Amended in Phase 3.5: below that step the panel opens as an overlay** over the content's right edge, and its controls stay live. Narrowing hides a docked panel rather than turning it into an overlay mid-resize (Kenan, 2026-10-22: the overlay dims the page and takes the keys, which a resize shouldn't raise unasked), and widening with the overlay open docks it. Phase 3's amendment standing the bar button, its overflow row and Q down on `fits` is superseded.

    **Amended in Phase 3.6: the floor holds without the panel too.** The sidebar folds once the window alone would leave the content under it, which makes the floor the narrowest content panel every page has to seat. The two rows that couldn't seat it now fit it: My Library's header folds its filter to a glyph, and the band drops its pills' labels onto their tooltips.

    The constant and the panel widths are settled on a real window. The track lists and grids already fit to their own width (`track-list.slint:32-33`, `GridGeometry` off `body.width`).
11. **The lifecycle follows `shown`, not `open`.** Rows and covers are released 350 ms after the panel stops being shown (`TEARDOWN_DELAY`, `queue_sheet/callbacks.rs:40`). That covers a close, Now Playing, the miniplayer and a narrow window alike. A long-open panel costs what the sheet cost while it was up: the same `QueueRows` window, the same private cover tier and the same 24-cover prewarm.

**Prior art** (two open-source desktop players' source; repo convention keeps their names out):
- **A right-hand queue panel hides below a window breakpoint rather than floating.** It reserves its width before the left sidebar measures, so the left sidebar folds first and stays folded when the panel hides in turn. Its width is clamped between a minimum and a maximum and capped to leave the content a floor. That shaped decision 10.
- **Corrected by a wider survey (2026-10-21):** two desktop players that follow their platform's guidelines overlay the pane below their breakpoint and keep its toggle working, and so do all five design systems checked (Material, Fluent/WinUI, GNOME, KDE, Apple). None greys the toggle out. That is Phase 3.5's overlay.
- **A drag carries ids and resolves them on the drop.** The drop gap is the upper or lower half of the row under the pointer, plus a few pixels of slack. The ghost is a small chip (a 20 px cover and a one-line label, at most 240 px wide, offset 8 px from the cursor). That shaped decision 7 and the ghost in Phase 6.
- **Not copied:** writing the panel width to settings on every drag move. Melodia flushes chrome state at exit, as it does for the sidebar.

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-ui/ui/views/queue-panel/queue-panel.slint` (was `views/queue-sheet.slint`, 395) | The panel: width, edge handle mount, header, list host, empty state, file and drag banners, seat-on-open. Docked or floating as an overlay (Phase 3.5) |
| `crates/melodia-ui/ui/views/queue-panel/queue-row.slint` (was `views/queue-row-item.slint`, 407) | One queue row and its menu: number, cover, title, and the duration that turns into the remove button on hover (Phase 3.5). The hand-rolled drag goes, and the row reports a press to the panel's `DragSource` (Phase 5) |
| `crates/melodia-ui/ui/components/track-list/track-list-row.slint` | Loses its hand-rolled drag and reports a press to its list's `DragSource` (Phase 5). `drag-items()`: what a pressed row carries (Phase 6) |
| `crates/melodia-ui/ui/components/grid/card-cell.slint` | `drag-items(title, cover)`: what a pressed card carries, its kind and its menu's set (Phase 6) |
| `crates/melodia-ui/ui/components/grid/entity-card.slint` | `pressed`, a left press on the card body (Phase 6) |
| `crates/melodia-ui/ui/components/grid/{album,artist,genre,playlist,entity-card}-grid.slint` | A copy-only `DragSource` round each grid's `ListView` (Phase 6) |
| `crates/melodia-ui/ui/components/track-list/track-list.slint` | A copy-only `DragSource` round its `ListView`, eight of the nine surfaces in one file (Phase 6) |
| `crates/melodia-ui/ui/components/drag-ghost.slint` (new, Phase 6) | The chip under the pointer: cover, title, and a count when several ride along |
| `crates/melodia-ui/ui/components/now-playing/queue-button.slint` (new, Phase 6) | The bar's queue button as a drop target (Add to Queue) and its 500 ms hover-to-open |
| `crates/melodia-ui/ui/views/queue-panel/queue-scrim.slint` (new, Phase 3.5) | The overlay's scrim and its outside-click close, mounted inside the content panel |
| `crates/melodia-ui/ui/components/rating-menu-row.slint` (new, Phase 3.5) | The rating row both menu families mount, lifted from `overflow-menu.slint:259-298`; sized by its host |
| `crates/melodia-ui/ui/components/track-list/track-context-menu.slint` | Gains the rating entry, `row-rating` and `rate([int], int)` (Phase 3.5) |
| `crates/melodia-ui/ui/components/now-playing/overflow-menu.slint` | Mounts `RatingMenuRow` in place of its inline copy (Phase 3.5) |
| `crates/melodia-ui/ui/components/edge-resize-handle.slint` (new, Phase 1) | The inner-edge drag, its clamp and its double-click reset, for whichever side grows. Lifted from `sidebar.slint:195-234`, which mounts it |
| `crates/melodia-ui/ui/components/drag-source.slint` (new, Phase 5) | A `DragArea` round a list's or grid's `ListView`, one per host (Phase 4). It carries `Drag.token`, arms the payload from the pressed row through `arm(index)`, and says which row it lifted through `lifts(index)`. Phase 6: `arm-items`, the `live` mirror, and `active` beside the touch gate. Phase 7.6: `active` goes, Up Next having been its only host |
| `crates/melodia-ui/ui/components/list-drop-target.slint` (new, Phase 5) | A `DropArea` over a list: slot from the pointer, the drop line, the edge-scroll `Timer`, and the ghost's position while it holds the drag (Phase 6). With `takes-items`, another list's items as a copy at the slot (Phase 6). Mounted by the queue panel, the playlist detail and Up Next. Phase 7.6: the edge band shrinks to a quarter of a short list |
| `crates/melodia-ui/ui/globals/drag.slint` (new, Phase 5) | `DragOrigin`, `DragPayload { origin, index }` and `Drag`: the token, `out payload`, `arm`/`disarm`/`revoke`. Phase 6 adds `DragItems { kind, ids, title, cover }` as the payload's `items`, `arm-items`, `live`, `touching`, and the follow point the chip reads; `revoke` keeps the items. Phase 7 adds `FileDrag` beside it: the two target rects and `target`, with `FileDropTarget` and `WindowRect` |
| `crates/melodia-views/src/ui/drag.rs` (new, Phase 5) | The token install. The press disarm is the winit filter's, saying whether a touch made it (Phase 6), and each list that rebuilds its rows revokes its own origin |
| `crates/melodia-ui/ui/globals/selection.slint` | Gains `effective-ids(...)`, the one statement of what a picked row or card stands for (Phase 5) |
| `crates/melodia-ui/ui/globals/queue.slint` | `Queue` gains `panel-width` (0 = default), `shown`, `pointer-in`, `keys-owned`, `toggle()` (handled in Rust), `close()`, `shown-changed`, `drop-in(kind, ids, slot)`. It loses `dismiss()`, the `close` callback and `open-changed`; `drag-source`/`drop-slot` give way to the `Drag` payload. Phase 3.5 adds `overlay-open` and `floating`, and `close()` closes whichever mode is up. Phase 7 drops `is-drop-hovered`, as `PlaylistDetail` does, both banners reading `FileDrag.target` |
| `crates/melodia-ui/ui/theme.slint` | `queue-panel-{min,default,max}-w` and `content-min-w`, beside `sidebar-*-w` (`:101-104`) |
| `crates/melodia-ui/ui/app-window.slint` (746) | The row mount, the `shown` mirror and the narrow-window derivations. The sheet's mount at `:599-604` goes. Phase 3.5: the `floating` mirror, the dock hand-off and the scrim mount. Phase 6: the ghost, and the window-sized `DropArea` at the back of the root's stack that follows the pointer outside the targets |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint` | `queue-keys` replaces `queue-sheet-up`. Phase 3.5: Delete and Backspace, the overlay's Escape arm, Q off `fits` |
| `crates/melodia-ui/ui/components/now-playing/up-next-list.slint` | `editable`, the row menu and the drop target (Phase 5). Phase 7.6: `menu-enabled` replaces `editable` and gates the menu alone, the miniplayer's mount passing it off; the `DragSource` only as tall as the rows |
| `crates/melodia-views/src/ui/now_playing/up_next.rs` | `NowPlaying.reorder`'s handler and `play_order_move`, the row-to-play-order map with the wrap refusal (Phase 5) |
| `crates/melodia-views/src/ui/queue_panel/{mod,callbacks,rows}.rs` (was `queue_sheet/`, 135/409/114) | As today, plus `drop-in` (Phase 6). `install` hands nothing back once the file drop stops reading its atomic (Phase 7) |
| `crates/melodia-views/src/ui/queue_panel/lifecycle.rs` (new, Phase 1) | Open, seat and teardown on `shown`, lifted from `callbacks.rs:244-409`. That file then holds the row actions alone. Phase 3.5: the toggle opens whichever mode fits |
| `crates/melodia-views/src/ui/nav_history.rs`, `callbacks/cross_tab_nav.rs` | Phase 3.5: Mouse-4/5 and the composite wheel stand down under the overlay, and Go to closes it, as they did for the sheet |
| `crates/melodia-views/src/ui/window_chrome/winit_filter.rs` (470) | The press arm writes `keys-owned` (Phase 2). The press and touch arms raise and lower `Drag.touching` (Phase 6). The four positioned drag arms write `FileDrag.target` and start a drop's import, sharing the point conversion with `CursorMoved` (Phase 7) |
| `crates/melodia-views/src/ui/window_chrome/file_drop.rs` (was `drop_coalescer.rs`, 142) | Reads the two targets, `route`s a point onto one, and starts the import a drop asks for (Phase 7). The batching, the 50 ms wait, `QUEUE_PANEL_SHOWN` and `CURRENT_PLAYLIST_ID` go |
| `crates/melodia-ui/ui/components/window-rect-report.slint` (new, Phase 7) | A host's window rect, reported for a hit test outside Slint. Mounted in the content panel and the queue panel's body, never inside an `if` |
| `crates/melodia-views/src/ui/playlists/detail.rs` | `detail_on_screen`: the playlist a file dropped on the page lands in (Phase 7) |
| `crates/melodia-views/src/ui/callbacks/card_actions.rs` (209) | `spawn_with_track_ids` becomes `pub(in crate::ui)` (Phase 6) |
| `crates/melodia-engine/src/player/engine/queue.rs` (586) | `insert_at`, with `insert_next(&[i64])` written over it (Phase 6) |
| `crates/melodia-app/src/library/queue.rs` (433) | `queue_insert_at`; `queue_play_next_many` becomes one call (Phase 6) |
| `crates/melodia-ui/ui/globals/nav.slint` | `sidebar-expanded-w` (the one clamp of the stored width), `sidebar-yields` (written by `AppWindow`) and `sidebar-rail`, which every part of the sidebar draws from (Phase 3) |
| `crates/melodia-app/src/services/settings/{interface,data}.rs` | `QueuePanelLayout { queue_panel_open, queue_panel_width: Option<f64> }`, flattened with `#[serde(default)]`. `LayoutFlags` sits at clippy's bool cap (`interface.rs:166-170`) (Phase 3) |
| `crates/melodia/src/shutdown.rs`, `crates/melodia/src/boot/ui_setup/hydrate.rs` | Written at exit and hydrated before show, beside the sidebar (Phase 3) |
| `winit/src/platform_impl/{linux/wayland,linux/x11,windows}/…`, `winit/src/event.rs` | `DragEntered`/`DragMoved`/`DragDropped`/`DragLeft` in place of the per-file events (Phase 7) |

Every production Rust file in the blast radius stays well under the 800-line cap. The largest are `engine/queue.rs` at 586 and `paged_rows/mod.rs` at 704, which this plan does not grow.

## Phases

### Phase 1: Groundwork, no visible change
Status: done 2026-10-21, as `86310699` (the rename) and `ac581216` (the handle and the lifecycle split). Its hand check rode Phase 2's.

1. **Rename the sheet to the panel in both trees.** That covers the Slint files into `views/queue-panel/`, `ui/queue_sheet/` → `ui/queue_panel/`, and the identifiers that say "sheet". The pins that name it change with it:
   - `CALLBACK_HOMES` (`melodia-testkit/src/lib.rs:70-86`);
   - `TIER_HOMES` and `GENERATION_WRITERS` (`crates/melodia/tests/cover_generation.rs:130-186`);
   - the `include_str!` in `playlists_tests.rs:9`.
   `queue-sheet-up` keeps its name until Phase 2 changes what it means.
2. **Extract `EdgeResizeHandle` from `sidebar.slint:195-234`** with an `edge` input (which side grows) and two-way `width`/`dragging`. The sidebar mounts it and its inline copy is deleted.
3. **Lift the lifecycle half of `queue_panel/callbacks.rs` (`:244-409`) into `lifecycle.rs`**, so Phase 2 rewires one file.

**Gate:** fmt, clippy, test. **By hand:** the sidebar drag, double-click and fold behave as before; the sheet behaves as before.

**Amended while implementing:**
- `EdgeResizeHandle` takes `grows: GrowEdge` and `drawn-width` and reports through `resized(length)` / `reset()` rather than a two-way `width`, since the panel's width is 0 = default. A drag now starts from the width drawn, not the stored one.
- Identifiers whose meaning Phase 2 changes kept their names until then, as `queue-sheet-up` did: the drop coalescer's `QUEUE_SHEET_OPEN` pair, `+QS` and the handles' `is_open`.

### Phase 2: The panel beside the content
Status: implemented and passed by hand 2026-10-21, with small follow-ups still open. Its tests wait for Phase 8.

1. **Mount `QueuePanel` after the content panel in the row**, with the width shape from decision 1.
   - **Tokens.** `Theme.queue-panel-{min,default,max}-w` start at 300 / 360 / 560 px, numbers to settle by hand. The sheet's card floor was 400 px (`queue-sheet.slint:169-179`), and rows must fit at the minimum.
   - **Default width.** `Queue.panel-width: 0px` means the default, so the default is spelled once in `Theme` and the double-click writes 0.
2. **Delete the overlay:** the `t` slide (`:35-36`), the scrim, `backdrop-touch` and the card (`:153-188`), and `Queue.dismiss()` with the empty `close` callback (`callbacks.rs:151`).
3. **One open path.**
   - `Queue.toggle()` serves the bar (`now-playing-bar.slint:363-371`), the overflow menu (`overflow-menu.slint:327-332`) and Q (`shortcut-scope.slint:227-230`). Under Now Playing it closes Now Playing and opens the panel.
   - `Queue.close()` serves the header ✕.
   - Closing clears the picks. Today only `dismiss` did, and the toggles left invisible picks behind.
4. **`Queue.shown` is mirrored at `app-window.slint`** rather than inside the panel, which unmounts with the miniplayer.
   - `open-changed` becomes `shown-changed`.
   - `lifecycle.rs` and the drop coalescer's flag follow it (decision 11).
   - The bar and overflow `active` states read it.
5. **The overlay's gates go.**
   - `nav_history::overlay_open` drops `Queue.open`, so Mouse-4/5 and the composite wheel work beside the panel. `an_overlay_releases_the_composite_arm` gets updated with it.
   - `cross_tab_nav::land` stops calling `invoke_dismiss`; it still closes Now Playing.
6. **Keys** (decisions 4 and 5).
   - The hover sentinel and the filter's press arm write `keys-owned`.
   - `queue-keys = Queue.shown && Queue.keys-owned` replaces `queue-sheet-up` in the Ctrl+A and Escape arms.
   - The Escape arm only clears picks.
   - The literal in `card_selection.rs:216-233` follows the rename.
7. **Content width.**
   - `grid_prewarm::BODY_CHROME_W` (`grid_prewarm.rs:220`) adds the panel's maximum, argued at the const: under-subtracting is the failure that upscales.
   - Audit the tooltip hosts within half a pill of the content panel's right edge. The fix is the rule's own (`align-end`, `ui-patterns.md` Tooltips), not a new fit rect.
8. **`view_tag`'s `+QS` becomes `+QP`.**

**By hand:**
- open and close by bar, overflow, Q and ✕;
- resize the window with the panel open;
- drag the edge while Songs fits its columns, and double-click it;
- Now Playing opens, the panel steps aside and comes back; Q under Now Playing;
- miniplayer and back with the panel open;
- Go to Album keeps the panel;
- Mouse-4/5 with the panel open;
- Ctrl+A and Escape after clicking the queue and after clicking a list;
- a locale with longer labels;
- idle RSS with the panel open, against today's sheet open.

**Amended while implementing:**
- **`Queue.toggle()` is a callback with its one handler in Rust** (`queue_panel/lifecycle.rs`). Under Now Playing it has to close Now Playing, and `globals/queue.slint` importing `Nav` would break the rule that only the dispatchers import sibling globals. `close()` stays a Slint function.
- **No `Queue.panel-dragging`.** The panel eases a float and multiplies its width by it, so an edge drag writes straight through and nothing needs the flag.
- **`close()` also drops `keys-owned`**, so a panel opened again takes no keys until it is pressed.
- **The focus regrab on `keys-owned` fires on the rising edge only.** On both edges it would steal focus from a field the user had just pressed.
- **The panel paints no fill.** The shell's mantle shows through, the same colour with one fill fewer.
- **The tooltip audit changed nothing.** The content panel's clip already sat inside the window's fit rect, so every host within half a pill of that edge was already end-aligned.
- **The `queue-sheet-up` literal** lived in `crates/melodia/tests/card_selection.rs`, not the views file.
- **The README's footprint rows keep the sheet's label**, as does `scripts/measure-footprint.py`, until they are measured again with the panel.
- **A row's menu hands the keys back as it closes.** Its popup takes the panel's hover, so the press on an entry dropped them, and Esc after the menu's Select all cleared the page's selection instead of the queue's.
- **Only a shown panel claims the keys.** `pointer-in` follows the pointer, so a panel closed under a still one claimed the next press on the content.
- **The edge drag starts from `root.width`**, not `open-w`, which a window too narrow for both squeezes the panel under.
- **The hover sentinel takes the wheel its children leave** (found in validation, 2026-10-23). Slint clears a `TouchArea`'s hover on a wheel event it doesn't take, so a tick over a queue too short to scroll left `pointer-in` down until the pointer moved, and a click in place after it claimed no keys.

**By hand, after validation:**
- a queue too short to scroll: wheel over it, click a row without moving the pointer, then Ctrl+A, Delete and Escape act on the queue;
- the wheel still scrolls a long queue.

### Phase 3: Remembered, and the narrow window
Status: done, implemented 2026-10-21 and passed by hand 2026-10-22. Its tests wait for Phase 8.

1. **Persistence.** `QueuePanelLayout` is written in `save_state_on_exit` beside the sidebar and hydrated beside `apply_sidebar_*` (`hydrate.rs:94-114`) before `app.show()`. A seeded open runs the lifecycle's open explicitly, since no `changed` fires on a seeded value. Older files load closed at the default width.
2. **Narrow windows** (decision 10). `Theme.content-min-w` and the three derived steps live in `app-window.slint`, and the sidebar's width binding adds the "yields" term.

**By hand:**
- restart with the panel open, closed and resized; a first launch;
- the launch must not ease the panel in from 0. That is the same concern as the sidebar's, unverified today;
- shrink and widen the window through every step, with the sidebar expanded and folded.

**Amended while implementing:**
- **Under a forced rail the sidebar's fold toggle and Ctrl+B stand down** (Kenan). This settles open question 1. They also stand down over the user's own fold while expanding wouldn't fit beside the shown panel, where a press would only flip the persisted fold with nothing on screen to show for it.
- **While the panel can't fit, the queue button, its overflow row and Q stand down** (Kenan; superseded by Phase 3.5's overlay). `AppWindow` publishes `Queue.fits` beside `shown`. It covers the miniplayer too, so Q's own `!MiniPlayer.active` gate went into it.
- **The sidebar comes back once the panel has stepped aside.** The fold is keyed on `shown`, not `open`, which is where this departs from the prior art. With both sets of controls standing down, a panel too wide to show would otherwise hold the sidebar folded with nothing left to press.
- **A seeded open needs no explicit call into the lifecycle.** `queue-shown`'s tracker takes its baseline inside `AppWindow::new()`, before hydration, so the first change-handler pass reads the seeded open as an edge.
- **`Nav.sidebar-yields` and `Queue.fits` are written from `changed` mirrors, not bound.** Bound to the window's width, they would restart the sidebar's width ease and `IconButton`'s colour ease on every resize move (`slint-pitfalls.md`, the structural-dirt entry).
- **The panel's room doesn't depend on `shown`.** So it keeps its width while it eases out from under Now Playing, rather than narrowing as the sidebar comes back.
- **The edge drag stops at the room.** It can't store a width the window won't draw, or fold the sidebar to make one.
- **So does the sidebar's, while the panel is shown.** Dragged past the room, the sidebar would yield to its rail mid-drag, unmounting its handle under the pointer and leaving `Nav.sidebar-dragging` up.
- **`Theme.content-min-w` starts at 452 px**: the narrowest body that still packs two grid columns, plus the page's insets. At the default widths the sidebar folds below a 996 px window, the panel narrows from 862 px to its minimum at 802 px, and it steps aside below that.
- **The settings keys carry the prefix their flattened siblings do**: `queue_panel_open`, `queue_panel_width`.

### Phase 3.5: Titles get the room, rating in the menu, Delete, and an overlay
Status: done, implemented and passed by hand 2026-10-22. Its tests wait for Phase 8.

**What you see before it.**
- **Titles barely fit.** A row's fixed cells take 246 px of the panel, so at the default 360 px a title gets 114 px, and 22 px while the row is hovered: the hover star strip opens beside it (92 px) and the remove button keeps a column of its own. At the 300 px minimum the title gets 54 px and disappears on hover.
- **Below the docked threshold the queue can't be opened at all.** The bar button and its overflow row grey out and Q does nothing.
- **Picked rows have no Delete key**, only the menu and the hover button.
- **Only the playing track can be rated from a menu**, through the overflow menu's inline stars. The row menu has Favourite and no rating.

**What keeps working.** The docked panel, its width, edge drag, persistence, and the order a narrowing window gives way in. The 300 / 360 / 560 widths stay, so the freed room goes to titles: 90 / 150 / 350 px, hovered or not. The favourite heart on a title. The overflow menu's rating looks and behaves as before.

**A. Slimmer rows** (`queue-row.slint`).
- The duration cell shows the remove button on hover, right-aligned where the duration ends. The hover heart goes, and so does the button's own column with its gap.
- `TrackTitle` keeps `is-favorite` and stops being handed the star strip. The track list rows keep theirs.

**B. One rating row for both menus.**
- `RatingMenuRow` (`components/rating-menu-row.slint`) is the overflow menu's inline row lifted out: glyph, label, stars, dimming through the brushes. Its host passes the row height, icon and star sizes and the idle icon colour, so the overflow menu keeps its look. It closes nothing; the overflow menu stays open after a star, the row menu closes.
- It is mounted in every track menu, right under Favourite: track lists, the queue and the bar's track card (Kenan). `TrackContextMenu` gains `row-rating` and `rate([int], int)`. A track list routes it to its own `set-row-rating`, the queue to `Queue.set-row-rating` and the bar to `Player.set-current-rating`.
- Over a selection it shows the clicked row's stars and applies the pick to the whole set, so clicking the lit star clears every pick. That is Favourite's rule; nothing in the tree has a mixed state to reuse. Its label is "Rating", or "Rating for {}" over a selection, the one new string.
- No Rust: `library::ratings::set_rating` already takes many ids, and 0 clears.

**C. Delete.** Delete and Backspace remove the queue's picks while the queue holds the keys (Backspace is the Mac's delete key). One arm after the dialog guard, through `Queue.remove-selected()`.

**D. The overlay** (decision 10, amended).
- **Two states.** `Queue.open` stays the docked panel's and the persisted one. A session-only `Queue.overlay-open` is the overlay's, so a narrow launch with the panel saved open shows nothing until the window widens or the user opens it.
- **What `AppWindow` derives.** `shown` is the docked panel fitting and open, or the overlay open, in the full UI and outside Now Playing. `floating` is `shown` without the docked room, written beside `shown` so the two never disagree for a frame. When `fits` rises with the overlay open, the overlay docks (`open` set, `overlay-open` cleared) with `shown` held through it.
- **One `QueuePanel` for both modes.** A second instance would share the rows' fetch anchor, the drag globals and `pointer-in`, and couldn't sit behind an `if` (its tracker on `Queue.shown`). Floating, the root takes no room in the row, stops clipping (a clipping item intercepts presses outside its box, `items.rs:757-766`), and slides its body in over the content panel's right edge, inset so the panel's rounded corners stay clear. The edge handle stands down.
- **The scrim** (`queue-scrim.slint`) mounts inside the content panel before `RoundedFrame`, where the sheet sat, closes on a click and takes the wheel.
- **Keys.** An overlay owns them without a press. Escape clears picks first and then closes it. Q stands down only in the miniplayer.
- **The sidebar folds only for the docked panel.** An overlay takes no room in the row.
- **Rust.** The toggle opens whichever mode fits. Mouse-4/5 and the composite wheel stand down under the overlay, and Go to closes it, as they did for the sheet. File drops, `claim_queue_keys`, the content-width estimate and persistence are unchanged.

**By hand:**
- A: hover rows at 300, 360 and 560 px; a favourite's heart; the button removes; the row stays lit under its menu.
- B: the overflow menu's rating, unchanged; rate a queue row, a track list row, the bar's card and a multi-selection; a Browse row outside the library can't be rated; clear by the lit star; every list showing the track repaints.
- C: Delete and Backspace with picks after clicking the queue; nothing after clicking a list, in a search field, or under a dialog.
- D: narrow with the panel docked (it hides, the sidebar comes back); open by button, overflow and Q; close by ✕, Escape, the scrim, the button and Q; Escape with picks; Delete and Ctrl+A in it; widen with it open (it docks); Go to Album from it; Mouse-4/5 under it; Now Playing and the miniplayer and back; a narrow launch with the panel saved open; the Clear queue confirm over it; the content panel's corners.

**Gate:** fmt, clippy, test, and `scripts/update-translations.py` with every catalog filled for the new string. A to C land together and D as its own commit.

**Amended while implementing:**
- **`Queue.close()` always closes the overlay, and the docked panel only where it fits.** A narrow window's ✕ leaves the docked panel open for when the room comes back, and a close landing in the dock hand-off's one tick can't leave the overlay standing.
- **The floating inset is the panel's own `float-inset`**, which `AppWindow` reads for `float-w`, so the inset is spelled once.
- **The rating row elides its label**, as `MenuItem` does, for a long translation in a menu at its width ceiling. The overflow menu sizes itself to the row's preferred width, so nothing there elides.
- **"Rating for {}" follows each catalog's own "Edit Tags for {}"**, so the two read alike in every language.

### Phase 3.6: The content floor holds without the panel
Status: done, implemented and passed by hand 2026-10-22. Its tests wait for Phase 8.

**What you see before it.**
- With the panel closed, an expanded sidebar keeps its width however narrow the window gets, so the content panel shrinks well below `Theme.content-min-w`.
- With a detail open, My Library's header needs more than the floor: the filter draws past its cell and the band's clip cuts it off, panel or not.
- A detail's action pills slide left over the cover once the row is wider than the text column.

**What keeps working.** The user's fold, persisted and untouched by a forced rail. The order a narrowing window gives way in beside a docked panel. Every header and pill row exactly as drawn wherever it already fits.

**A. The sidebar** (`app-window.slint`). `sidebar-yields` also rises once an expanded sidebar would leave the bare row under the floor, and the sidebar's edge drag stops at that room with no panel docked.

**B. The filter folds** (`tab-search-header.slint`, `search-bar.slint`). Past the row that seats the compact bar beside the input's floor, the box folds to its glyph. A click opens it across the row over the bar, where it stays while it holds focus or text, and ✕ clears and folds it (Kenan). One `SearchBar` throughout, so a resize hands no focus across.

**C. The pills drop their labels** (`library-tab-band.slint`, `action-pill.slint`, both pill rows, both pages). The slot spans only the room beside the cover; short of it, every labelled pill shows its glyph with the label as its tooltip, on every row in the band (Kenan). The labelled width is measured off a hidden copy of the row. Radio's Favorites row keeps its labels, its cell clipping an in-tree tooltip.

**By hand:**
- A: narrow with the panel closed and the sidebar expanded (it folds before the content drops under the floor, and comes back on widening); the toggle and Ctrl+B under the forced rail; a relaunch keeps the user's fold, and one at a narrow width opens on the rail; the edge drag on a narrow window.
- B: a playlist open in My Library, narrowed until the filter folds; open by a click, type, click away with text and without, ✕, widen while open.
- C: a playlist detail, smart and not, and a Radio station page, narrowed until the labels drop; their tooltips; no flicker at the threshold; a longer locale.
- The miniplayer and back.

**Gate:** fmt, clippy, test. One pin moved with the change, `placeholder_tests::the_search_bar_negotiates_its_width`. No new strings or icons.

**Amended after the first hand pass:**
- **Every fold eases the way a compact tab does** (Kenan): the filter onto its glyph and back, the field across the row and the tabs out from under it, and each pill's label onto its glyph. The header and the band each own the floats, written from `changed` and eased on the tab bar's curve; `SearchBar.fold-t` and `PillButton.compact-t` only draw them. The pills land rather than ease while the band morphs, so a row arriving with a detail takes the shape it has room for. A second pin moved with it, `tab_search_header_tests::every_painted_brush_is_an_input`, which now reads the bar's brushes through their fade.
- **A narrow launch forces the rail too.** The window's term in `sidebar-yields` waits for a measured row: there is none at construction, where its `changed` mirror takes its baseline, so a row read as narrow there left a launch that really was narrow with no edge to write, and the sidebar drew expanded under the floor.

### Phase 4: Prove the drag primitives (a spike; nothing lands)
Status: done 2026-10-22, on a headless harness rather than a hand pass (Kenan). The harness was a throwaway crate outside the repo:
- **What it mounted.** The real `TrackListRowItem` and `QueueRowItem`, compiled from this tree, under `DragArea` and `DropArea` on a `MinimalSoftwareWindow`.
- **How it drove them.** Synthetic pointer and key events with a hand-advanced clock.
- **How it judged.** 31 checks. Fact 1 compared each gesture's callbacks against an unwrapped control list.

How a drag feels, and a drag that leaves the window, move into Phase 5's hand pass.

Confirm each fact the next two phases stand on:
1. **A `DragArea` around a track row in a pan-disabled `ListView` keeps every row interaction:** click, Ctrl/Shift pick, double-click, right-click menu, checkbox, hover heart and stars. It starts a drag past 8 px.
2. **A `DropArea` over the queue list gets `can-drop` on every move** with list-local positions, and lets clicks and wheel through.
3. **A window-sized `DropArea` answering `none` still lets the one beneath accept.** If it does, the ghost is a themed Slint chip following `Drag.x/y`. If not, it is `drag-image` set to the source's row-tier cover.
4. **A drop lands and `drag-finished` still fires after the source row has scrolled out of view.** This depends on whether `ListView` recycles or destroys the instance.
5. **A `data-transfer` property on a global, set once from Rust, keeps a `DragArea` armed.**

**Answers** (Slint 1.18.1):
1. **Yes, wherever the `DragArea` sits.** These gestures fired the same callbacks with a `DragArea` in each row, with one round the whole list, and with none:
   - a click, and the Shift and Ctrl picks;
   - a double-click;
   - the right-click menu;
   - the hover-mounted checkbox, star strip and heart;
   - a 6 px wobble;
   - the wheel.

   Past 8 px (strictly more, `drag_n_drop.rs:95-103`) the drag starts, the row gets a cancel, and no click lands.
2. **Yes.**
   - `can-drop` ran on every move over the queue's `DropArea`, in the area's own coordinates.
   - A double-click, a hover-mounted remove button and the wheel all reached the queue rows through it.
3. **No, and the ghost's source of positions changes around it.**
   - **In front, it takes the drop.** A window-sized leaf in front sees every move, but it also takes the drop. The runtime turns the release into a `Drop` and dispatches it by hit test, not to the area that accepted the hover, and a `DropArea` takes any `Drop` it receives (`window.rs:877-891`, `drag_n_drop.rs:350-359`).
   - **At the back, it works.** The same leaf at the back of the root's stack sees exactly the moves no target took, and the target keeps its drop.
   - **So the ghost is still a themed Slint chip.** That back leaf positions it outside the targets, and each target's own `can-drop` positions it over them. `drag-image` isn't needed.
   - **Any `DropArea` drawn over another takes its drops.**
4. **The drop lands, but `drag-finished` reaches only a source that still exists.**
   - **The drop always lands.** The payload lives in the window.
   - **The source is held weakly.** The window keeps only a weak reference to it, and the drag image is drawn through the same reference (`window.rs:1118-1131`, `:1894-1897`).
   - **Rows are destroyed, not recycled** (`model/repeater.rs:408-417`).
   - **What a `DragArea` in a row missed.** The end of a drag whose row had scrolled out, had been reset under it, or had been removed inside `dropped`, which is what both `on_reorder` handlers do through `move_row`. It also missed a cancel after a scroll-out. One round the list heard all four.
5. **Yes.**
   - An `in property <data-transfer>` on a global, set once through `slint::DataTransfer`, arms every `DragArea` bound to it.
   - Left at its default, those `DragArea`s stay inert and clicks still land.
   - A token holding only `set_user_data` carries no native data, so the drag stays in-window even once a backend implements `start_drag` (`input.rs:1497-1519`).

**Found beside them:**
- **A held Ctrl or Shift changes `proposed-action`**, to copy or move (`drag_n_drop.rs:430-457`). The rows read the same keys for picks, so a target returns its own action.
- **A pointer leaving the window cancels the drag** (`window.rs:924-929`). Whether winit reports a leave while a button is held differs by platform.
- **No OS file drag reaches a `DropArea`.** The 1.18.1 winit backend has no `HoveredFile` or `DroppedFile` arm.
- **`TrackList`'s `ListView` sets no `mouse-drag-pan-enabled`.** It inherits fluent's `false`.

### Phase 5: One drag mechanism for the reorders
Status: done, implemented and passed by hand 2026-10-22, its validation fixes still to pass by hand. Its tests wait for Phase 8.

1. **The shared pieces:**
   - `DragPayload`;
   - the `Drag` global, with the token set at install as a user-data-only `DataTransfer`;
   - `Selection.effective-ids`, with both menus moved onto it;
   - the `DragSource` and `ListDropTarget` components.

   **What Phase 4 settled about them:**
   - `DragSource` mounts round each list host's `ListView`, and the rows report the press.
   - A target returns the action it means, never `proposed-action`.
   - A `DropArea` sits over nothing it shouldn't take drops from.
2. **The queue panel and the playlist detail reorder through them.** The Rust `on_reorder` handlers keep their `(from, to)`. Deleted:
   - `queue-row.slint`'s drag state, slot maths and ghost timer (was `:58-104, 153-238`);
   - the panel's drop line and nudge guard (was `queue-sheet.slint:62-79, 299-329`);
   - `track-list-row.slint:80-114, 238-299`;
   - `draggable-track-list.slint:44-64, 163-197`;
   - `drag-source`/`drop-slot` on `Queue` and `PlaylistDetail`.
3. **Up Next in Now Playing** (decision 2).
   - It gets `ListDropTarget` over its rows and the row menu with the queue's entries minus the pick entries.
   - Rows map to play indices by the list's own `mod(base-index + idx, queue-length)`, for skip, remove and move. A move that would cross the repeat-all wrap is refused, as the queue's two no-op slots are.
   - The miniplayer host passes `editable: false` (`menu-enabled: false` since Phase 7.6, gating the menu alone).

**Visible:**
- reorders start at 8 px rather than 4;
- edge scrolling keeps going while the pointer rests, at a steady speed (one prior-art player's speeds up a row per tick and is past control within a second);
- a reorder may carry its row out of view, which is refused today (`queue-sheet.slint:299-311`);
- Up Next gains its menu and reorder.

**By hand:**
- both reorders, including one carried out of view by the edge scroll;
- Up Next's reorder and menu;
- a drag out of the window and back, on Wayland and X11, which Phase 4 could only show cancels in Slint.

Lands as one commit: a list cannot host both mechanisms.

**Amended while implementing** (each read in the Slint 1.18.1 sources):
- **The payload is armed by the pressed row at the press, not written as `dragging` rises (amends decision 7).** A `DragArea` starts a drag from any left press it contains, so a press on a gap between rows, a row's own button or the space under the last row drags too. Nothing in Slint sees every press, so the winit filter's press arm disarms `Drag` ahead of Slint's dispatch and the row the press lands on arms it within that dispatch. A drag from anywhere else carries nothing. This holds by construction, where a `changed dragging` write would also land a pass after the press.
- **`DragPayload` carries `origin` and `index` alone, and lives in `globals/drag.slint`.** Rust never names it. Phase 6 adds the rest with the code that reads it.
- **A rebuild under a live drag revokes the payload rather than resetting `drag-source`/`drop-slot`.** The source survives the rebuild, so the drag goes on, but its index no longer names the row it lifted. `Drag.revoke(origin)` is called from `queue_panel::rows::show_queue` when the ids moved, from `PlaylistsUi::rows_moving` (through a weak `Drag` the handle holds, the hook being handed nothing to reach it through), and from Up Next's `show_slice` when its ids or base moved, a library tick reseeding the same slice.
- **Up Next's move maps onto play order in Rust**, through `NowPlaying.reorder(from-row, to-row)` and `up_next::play_order_move`, so the wrap refusal can be pinned. Its row moves on show first, as the panel's does. Skip and remove keep the `Math.mod` the click already spelled, worked out once per row as `play-index`.
- **The edge-scroll `Timer`'s `running` reads no layout property.** All three targets sit inside an `if`, so `can-drop` works out the edge band with the pointer and stores it.
- **`Selection.acts-on-set` and `effective-ids` cover three menus**, the station menu spelling the same rule as the track and card menus.
- **`Theme.fade-lifted` is the one weight a lifted row keeps.** The queue and track rows keep their `opacity`; Up Next's row folds it into its brushes, having no layer today.

**Amended in validation** (2026-10-22, each read in the Slint 1.18.1 sources):
- **A touch's start disarms `Drag` too.** Slint takes a first touch as a left press, so a disarm keyed on `MouseInput` alone let a touch drag from a gap carry the last row pressed. The queue's key claim stays a mouse press's, reading a hover no touch has ahead of it.
- **`ListDropTarget.split-row` keeps a slot across Up Next's wrap from drawing its line.** Up Next binds `queue-length - base-index`, and `play_order_move` still refuses, being the check that holds. The target's slot-to-index step is one `landing` property now.
- **`rows_moving` takes no global**, its one implementer having stopped reading the one it was handed.
- **`slint-pitfalls.md`'s drag-pan entry described a binding older than 1.18.** Every 1.18 style binds its flickable's own `mouse-drag-pan-enabled` and pins it off, Material included, and `interactive` stays on, so a touch still pans. Corrected there, in `station-grid.slint` and in `scrollbars.rs`.

**By hand, after validation:**
- Up Next under repeat-all: a drag across the wrap draws no line and moves nothing;
- ~~a touch drag from a gap between rows moves nothing~~, and ~~a touchscreen swipe on each of the three lists~~: superseded by Phase 6, where a touch starts no drag at all.

### Phase 6: Dragging into the queue
Status: done, implemented 2026-10-22 and passed by hand 2026-10-23, except the touch checks, which are deferred until a touchscreen is to hand: a swipe on a list and on a grid, a mouse drag straight after a touch, and on Windows whether a touch also reports a mouse press. Its tests wait for Phase 8.

1. **Engine and library.**
   - `QueueState::insert_at(slot, &[i64])`, with `insert_next` written over it.
   - `queue_insert_at(state, slot, ids)` beside `queue_add_tracks` (`existing_in_order`, then the splice).
   - `queue_play_next_many` becomes one call (decision 8).
2. **Sources.**
   - Every list on the nine surfaces and every card grid mounts a `DragSource` of `(kind, effective ids)` round its `ListView`, copy-allowed. The pressed row or card names the ids (Phase 4). The playlist's list also allows move while reorder is on.
   - `TrackList`'s `ListView` pins `mouse-drag-pan-enabled: false`, as the click-to-act grids do. It inherits `false` today.
   - A Browse row outside the library (id 0) carries no payload, so nothing takes its drag, matching `collect_nonzero_track_ids`.
3. **The panel as a target.**
   - It takes outside payloads as copies and its own rows as moves. Below the last row, and the whole empty list, mean slot = length.
   - `Queue.drop-in(kind, ids, slot)` goes through `spawn_with_track_ids` into `queue_insert_at`. A slot that went stale during the resolve is clamped by the splice.
4. **What letting go will do.**
   - While a drag hovers the panel, a banner reads the payload: `Add {n} track(s) to the queue` for tracks, `Add “{}” to the queue` for a card.
   - That means new `@tr` strings: run `scripts/update-translations.py` and fill every catalog.
5. **The ghost is a themed Slint chip** (Phase 4). A drag of several items shows their count on it.
   - **Over a target**, it follows that target's own `can-drop` positions.
   - **Elsewhere**, it follows a window-sized `DropArea` at the back of the root's stack, which answers `none`.
   - **That leaf can't move to the front.** In front of the targets, it would take every drop.
6. **The bar's queue button takes drops** (added in Phase 3.5's survey), so a drag needn't open the panel first.
   - A drop on it appends the whole payload, the pick included. A desktop player that takes button drops is reported adding only the first track of a multi-track drop; that is the bug to avoid.
   - Held over it for 500 ms during a drag, it opens the panel, docked or as the overlay, so the drop can be placed at a slot. 500 ms is GTK's own hover-to-switch delay.

**By hand:**
- a drag from each surface;
- a picked row takes the whole pick;
- a large genre onto a long queue, timed against Add to Queue on the same card;
- a drop onto the empty queue;
- a drop between rows under shuffle;
- a drop above the playing row: playback is undisturbed and the highlight follows;
- a drag released outside any target;
- a picked set dropped on the queue button, and a hover over it that opens the panel, docked and as the overlay.
- Added while implementing: a drag from a sorted playlist (it still copies); a drag from a Browse row outside the library (nothing lifts); the chip over the playlist's own reorder (none) and once it leaves; a touchscreen swipe on a list and on a grid (it pans, nothing drags); a mouse drag straight after a touch, and on Windows whether a touch also reports a mouse press that lowers the gate again; several cards' banner in a three-form locale.

**Settled while implementing** (each read in the Slint 1.18.1 sources):
- **A release over no accepting `DropArea` becomes an `Exit`, not a `Drop`** (`window.rs:893-901`), so the leaf behind the targets never sees one and can't be what hides the chip. Every drag ends on its source's `dragging` falling (`finish_drag`, `items/drag_n_drop.rs:279-283`), which `DragSource` mirrors into `Drag.live`. A source unmounted mid-drag never hears the end, and the next press's disarm is the backstop.
- **The payload grows `items: DragItems { kind, ids, title, cover }`** beside `origin` and `index`, and `count` is `ids.length`. The pressed row or card builds it, through `TrackListRowItem.drag-items()` and `CardCell.drag-items(title, cover)`, each calling `Selection.effective-ids`. A disk-only Browse row arms nothing by construction, its body `TouchArea` being disabled, so no id-0 guard was needed.
- **`revoke` drops the lifted index and keeps the items.** The ids survive a rebuild where the index doesn't, so a playlist row whose list rebuilt mid-drag still copies into the queue and simply stops reordering.
- **The banner replaces the panel's header row** while `ListDropTarget.holds-items`, rather than a scrim over the rows like the file banner: the drop lands at the line, so the line has to stay visible. Tracks are counted, one card is named, and several cards are counted by kind (Kenan), six new msgids filled in all 15 catalogs. The single-card msgid quotes with straight quotes, as the tree's other name-quoting strings do, and each catalog takes its own.
- **The chip shows only for a drag that carries items, and not over its own list's reorder target** (`Drag.follow`'s third argument). The queue and Up Next carry none, so their reorders keep the dimmed row and the line.
- **The button's drop is `CardActions.add-to-queue` verbatim**, no new Rust: the panel's rows are gone 350 ms after it hides, so no end slot can be read off them there. Its wrapper sizes itself, a `DropArea` reporting `stretch: 1` (`drag_n_drop.rs:313`). In the overflow menu it takes no drop, the press a drag starts with shutting the menu.
- **A touch never starts a drag** (Kenan). A `DragSource` round every list and grid would compete with every list's touch pan, so `disarm_drag` says which pointer pressed, ahead of Slint's dispatch, and every `DragSource` is `enabled: active && !Drag.touching`. A disabled `DragArea` forwards the gesture untouched (`drag_n_drop.rs:146-149`), so lists always pan by touch and the reorders and drag-to-queue are mouse-only. Up Next's mount moved from `enabled` to `active`, a mount's `enabled` overriding the component's binding. Phase 7.6 drops `active` with its only host's switch, leaving `enabled: !Drag.touching`.
- **A `DragSource` hands back the click its rows don't take** (found in validation, 2026-10-23). A `DragArea` accepts every left press no child takes (`drag_n_drop.rs:162`; its filter ignores any other button, `:80-83`), so a click on a list's or grid's empty space stopped reaching the page's backdrop and no longer gave the filter box up. A `TouchArea` behind the list fires `background-clicked`, each list and grid forwards it as `request-blur-search`, and each host bumps its own page's tick. The queue panel and Up Next leave it unbound, having no filter box. **The `TouchArea` is off with the host's switch** (`enabled: root.active`): a switched-off `DragArea` still hands presses and hover to its children (`drag_n_drop.rs:146-149`), so left on it took the empty space under the miniplayer's Up Next away from the window's drag region. Phase 7.6 sizes Up Next's source to its rows instead, and the switch went with it.
- **`Queue.drop-in` drops nothing at a slot that isn't one** (found in validation, 2026-10-23), bailing as the panel's other handlers do on a negative index rather than landing it at the front. `ListDropTarget` clamps its slot, so only the contract changed.
- **Out of scope, joinable later through the same `DragSource`:** Search's two card strips (no menu, no Add to Queue today), Browse's card mode and folder cards (open question 3) and station cards.

### Phase 7: File drops land where they are let go
Status: done, implemented and passed by hand on Linux 2026-10-23. Windows still needs validation on the Windows machine, and no CI run has built the branch yet, so `clippy-windows` and `test-windows` haven't compiled the fork's Win32 half either. Its tests wait for Phase 8.

1. **The fork reports the pointer during a file drag** on Wayland, X11 and Win32 (decision 9). `winit/README.md` lists the fourth change. The routing stays the filter's: no OS drag reaches a `DropArea` in 1.18.1 (Phase 4).
2. **The filter tracks that position.**
   - The queue banner shows over the panel, and the playlist banner over an open playlist elsewhere.
   - The coalescer routes each batch by where it was released. Over the panel it appends to the queue as now. Over the content it goes to an open playlist. Anywhere else it is discarded.
3. **Windows is checked on the Windows machine.**

**By hand:**
- a file onto the panel with a playlist open beside it;
- a file onto the playlist with the panel open;
- a file with the panel closed;
- an X11 session.
- Added while implementing:
  - the banners swap at the panel's edge as the drag crosses it;
  - a release over the sidebar, the player bar and the titlebar lands nowhere;
  - the overlay: its card takes the drop, the dimmed page doesn't;
  - a playlist left open on the Playlists tab and a drop on Songs lands nowhere, where it used to land in the hidden playlist;
  - a smart playlist open lands nowhere;
  - several files at once arrive in natural order, and a folder fails as before;
  - a drag that leaves the window clears its banner;
  - a fast drop straight onto the panel on Wayland lands every file;
  - a link or an image dragged from a browser on Wayland raises no banner and lands nothing;
  - fractional scaling on Wayland, and on Windows Explorer drags at 100 % and 150 %+;
  - on Windows, the miniplayer swap both ways under the native titlebar with the panel docked, the reports' trackers sitting in the branch the swap drops.

**Amended while implementing:**
- **The events carry every path at once, so there is nothing left to coalesce.** `drop_coalescer.rs` becomes `file_drop.rs`, a router: `DropTargets::read` takes what can take a drop right now, the pure `route` hit-tests a point against it, and `import` starts one import per drop with no wait. `QUEUE_PANEL_SHOWN` and `CURRENT_PLAYLIST_ID` go with the batching, and with the atomic gone `queue_panel::install` hands nothing back.
- **Slint publishes where the targets are, and Rust decides.** `FileDrag.queue-rect` and `content-rect` are written by `WindowRectReport`, mounted in the queue panel's body and in the content panel. The filter routes every move and the release through the same `route`, and writes the one `FileDrag.target` both banners read, which replaces the two `is-drop-hovered` flags that could disagree.
- **The playlist target is the content panel, not the detail's list.** The report carries a tracker on its own geometry, and the list unmounts inside the band's morph, which is the `changed`-in-`if` panic. A drop over the band lands in the playlist too.
- **The page under the overlay takes no drop**, being dimmed and taking no click.
- **"An open playlist" means the detail on screen**: `playlists::detail_on_screen` asks for My Library's Playlists tab with neither Now Playing nor the miniplayer covering it, and a playlist that isn't smart. The old route took any detail left open, on a hidden tab or behind another page.
- **On Wayland a drop that beats the path list waits for it**, and a `Leave` arriving meanwhile leaves the read alone. The old `Drop` arm sent whatever had been read so far. A list naming no file enters nothing, as on X11 and Windows. X11 asks for the list on the first position, as before, and a drop ahead of it still lands nothing.
- **The fork fixes the Win32 `IDropTarget` vtable**, which declared the point as a pointer where COM passes it by value. Harmless while nothing read it.
- **macOS isn't patched.** It isn't a release target, and its backend still sends the per-file events, which the filter no longer reads.

### Phase 7.5: A drag of picked rows moves them all
Status: implemented 2026-10-23, static gates green, waiting on its hand pass. Its tests wait for Phase 8.

**What you saw.** With several rows picked in the queue panel or Playlist Detail, dragging one of them moved that row alone, and only it dimmed. A drag into the queue from another list already carried the whole pick.

1. **One move rule, `melodia_core::utils::block_move`**, which the engine's play order, `PagedRows`' keyset and the store's `playlist_items` all move through. `move_block(items, positions, to)` gathers the items in their order at `to`, counted once they have left, and `landing(positions, slot)` turns a drop slot into that `to`. A single row is a block of one, so `QueueState::move_track`, `PagedRows::move_row`, `reorder_playlist_track` and `queue_panel::picked_positions` went.
2. **The payload says when a press lifted the picks.** A press on a picked row while more than one is picked (`Selection.acts-on-set`) arms as before, then `lift-picks` with where Rust finds them (`Queue.picked-run`, `PlaylistDetail.picked-run`): one run, or scattered. Every picked row dims through `DragSource.lifting-picks`.
3. **`ListDropTarget` keeps its line off the slots that move nothing**, inside or beside a run, and hands a picks drop over as `move-picks(slot)`. Scattered picks move at every slot. `reorder(from, to)` stays for one row, so Up Next is untouched.
4. **The playlist's write is by track id** (`move_playlist_tracks`), a track sitting in a playlist once.
5. **A move refreshes an automatic playlist cover** (found while tracing). A reorder never had, so a different track dragged to the top left the card and hero on the old first track's cover until the next add or remove. `move_playlist_tracks` now ends in `update_playlist_thumbnail_and_timestamp_tx` as add and remove do, and once the write lands the page reads its header again through `playlists::refresh_header` (split out of `refresh_detail`, the rows reading the same after a move) and its grid through `refresh_after_write`. That helper bumps `updated_at` wherever it rewrites the cover, so a reordered playlist rises in the grid as one with a track added does.

**By hand:**
- the queue: a run of three dragged past the playing row both ways, scattered picks gathered at the end, a run hovered inside and beside itself (no line, nothing moves), an unpicked row dragged beside picks (it alone moves), a track queued twice and picked (both copies move), under shuffle, and an edge scroll carrying the picks;
- Playlist Detail: the same pick cases, the order surviving a reopen, and under a Title sort the pick still copying into the queue with every picked row dimmed;
- a playlist on its automatic cover: a track with another cover dragged to the top changes the hero and the card, and a playlist with a picked cover keeps it;
- Up Next's reorder unchanged, its wrap refusal included.

### Phase 7.6: The miniplayer's Up Next reorders
Status: implemented 2026-10-23, static gates green, waiting on its hand pass. Its docs landed with it; its tests wait for Phase 8.

**What you saw.** In the miniplayer's column, Up Next only took a click: dragging a row did nothing, while the same list in Now Playing reordered the queue. The right-click menu stays off there, the sheet having no room for it.

The miniplayer mounts the same `UpNextList`, and its `editable: false` switched off the menu, the `DragSource` and the `ListDropTarget` together. Nothing on the Rust side changes: `up_next::wire_reorder`, `play_order_move` and `show_slice`'s revoke already serve both hosts through the one `NowPlaying` global.

1. **One switch, for the menu alone.** `editable` becomes `menu-enabled` on `UpNextRow` and `UpNextList`, and the reorder gates go: both hosts reorder, so a switch for it would have no `false` caller.
2. **The `DragSource` is only as tall as its rows.** A `DragArea` accepts every left press its children leave, so at full height it took the space under a short queue from the miniplayer's window-move region. The `ListDropTarget` keeps the list's full height, a `DropArea` passing every press and hover through, so a drop under the last row still lands at the end.
3. **`DragSource.active` goes**, Up Next having been its only host. A mount binding `enabled` still drops the touch gate, which the `enabled` line says.
4. **The edge band is at most a quarter of the list** (Kenan: a shared rule rather than a miniplayer-only input). On a list one or two rows tall the two 40 px bands overlapped and scrolled the whole drag. Lists at least 160 px tall are unchanged.

**By hand:**
- the miniplayer column: a row dragged down and up lands where the line says, the line and the dimmed row reading on the plain palette and on the backdrop; a click still skips; a right-click does nothing;
- repeat-all with the wrap in view: no line across it and nothing moves;
- a track ending mid-drag: the drag is revoked and the drop moves nothing wrong;
- a queue of one or two rows: a press under them moves the window, and a drop there sends the row to the end;
- a long queue in a panel one or two rows tall: the list scrolls only at its edges and a drop in the middle lands without it running;
- under a station: the queue beneath reorders;
- Now Playing's Up Next unchanged (menu, drag, a drop under the last row); the queue panel's and Playlist Detail's edge scroll unchanged on a normal window.

### Phase 8: Tests and docs (on Kenan's go)
- **New pins:**
  - `insert_at` at slot 0, at the end, and before, at and after current, under shuffle and not;
  - Play Next of many against the old loop's play order;
  - Up Next's row-to-index mapping across the wrap;
  - `effective-ids` as the only statement of the rule, by a walk;
  - no hand-rolled slot maths left outside `ListDropTarget`, by a walk;
  - no `DragArea` inside a `for`, by a walk (Phase 4), and no `DragArea`/`DropArea` outside `drag-source.slint` and `list-drop-target.slint`;
  - the winit filter's press and touch arms disarming `Drag`, and each list that rebuilds its rows revoking its own origin (Phase 5);
  - `play_order_move` within each side of the repeat-all wrap and refusing across it, at the boundary slot from both sides, with every row past the wrap and on an empty queue, and Up Next's `split-row` refusing the same moves (Phase 5);
  - `min <= default <= max` on the panel tokens;
  - the prewarm test's width table gaining the panel (`grid_prewarm_tests.rs:227-260`);
  - the filter's press arm;
  - the panel's hover sentinel answering the wheel with `accept` (found in validation);
  - `QueuePanelLayout` defaults and an older `settings.json`;
  - drop routing by position;
  - Phase 3.5: the Delete arm's gate; `RatingMenuRow` as the only `StarRating` inside a menu, by a walk; `Queue.floating` written beside `shown`; the overlay's Escape arm above `Selection.any-live()`.
  - Phase 3.6: `sidebar-yields` carrying the bare-row term, the window's term waiting for a measured row; the folded header and the compact detail row seating inside `Theme.content-min-w`, as arithmetic over the published floors; both pages feeding the band from a hidden ruler.
  - Phase 6: `Drag.touching` raised by the touch arm and lowered by the press arm; no `DragSource` mount binding `enabled`, which drops the touch gate, by a walk; `revoke` keeping the items; the queue button dropping through `CardActions.add-to-queue`; `insert_at` clamping a stale slot; every `DragSource` host with a filter box forwarding `background-clicked`, by a walk.
  - Phase 7.5: `move_block` at both ends, scattered, already in place, and refusing unsorted or out-of-range positions; `landing`; `QueueState::move_rows` keeping the current track across a block holding it, passing it, and landing either side of it; `PagedRows::move_rows`' notifications and `picked_rows` over a track listed twice; `lifted_run` for a run, scattered picks and none; `move_playlist_tracks` with scattered and contiguous ids, and moving an automatic cover to the track that now leads while leaving a picked one alone; `lift-picks` called only behind `Selection.acts-on-set`, by a walk.
  - Phase 7.6: Up Next's `DragSource` bounded by its rows while its `ListDropTarget` spans the list, read from `up-next-list.slint` in `up_next_tests.rs`; `ListDropTarget`'s edge band capped at a quarter of its height; the miniplayer's mount passing `menu-enabled: false` and no other Up Next switch.
  - Phase 7: `route` inside each rect, on its edges, with the floating card over the content, with the panel hidden, with no playlist on screen, and `detail_on_screen` refusing a smart playlist, a hidden tab, Now Playing and the miniplayer; no `WindowRectReport` inside an `if` or a `for` other than the full-UI branch, by a walk; `FileDrag.target` written only by the winit filter, by a walk.
- **Docs.** Phases 1 to 3's half landed on 2026-10-21, and Phase 3.5's on 2026-10-22 (`ui-patterns.md`'s row menu, queue block, `TrackTitle` and keys, `library-data.md`'s rating paths, `slint-pitfalls.md`'s `queue-row.slint` example, and the README's shortcut table and feature lines), and Phase 3.6's the same day (`ui-patterns.md`'s narrow-window block and its `PillButton`, `TabBar` and `LibraryTabBand` entries, the last one's stale pill-slot line retired with it, and `slint-pitfalls.md`'s `SearchBar` entry), and Phase 5's the same day (`ui-patterns.md`'s drag entry and the row menu's four hosts, its `"up-next"` context and the `Selection` rule, `slint-pitfalls.md`'s drag-pan entry and its in-window drag entry carrying Phase 4's traps, the README's queue line, and the root `CLAUDE.md`'s globals count), and its validation's the same day (`ui-patterns.md`'s drag entry gaining `split-row` and the touch disarm, `slint-pitfalls.md`'s drag-pan entry corrected to 1.18's binding and its in-window drag entry gaining the touch press), and Phase 7's on 2026-10-23 (`desktop-shell.md`'s fork and drop-routing bullets and its two verbs, `slint-pitfalls.md`'s Wayland file-drop entry, `ui-patterns.md`'s `Queue.shown` bullet and a file-drag entry beside the drag one, the root `CLAUDE.md`'s Known Gaps and globals count, the README's fork paragraph and licence note, `winit/README.md`, the comment above `[patch.crates-io]`, and the fork's entries in `licenses/ATTRIBUTION.txt` and `packaging/debian-copyright`), and Phase 6's and its validation's the same day (`ui-patterns.md`'s drag entry gaining the items, the chip, the queue button, the touch gate, `background-clicked` and `revoke` keeping the items, with pointers from its grid and track-list entries, and the keys bullet the sentinel's wheel; `slint-pitfalls.md`'s in-window drag entry gaining the unclaimed press and the release that becomes an `Exit`, its drag-pan entry trading the touch competition for the touch gate, and its `has-hover` entry the sentinel; the README's queue line), and Phase 7.5's the same day (`ui-patterns.md`'s drag entry gaining `move-picks` and the picks bullet, the README's queue and playlist lines). Phases 1 to 3:
  - the panel's cross-file facts and the keys in `ui-patterns.md`, the narrow window's order and its two flags among them;
  - the sheet's mentions across the rules;
  - `settings.json`'s contents in the root `CLAUDE.md`;
  - the queue's feature line in `README.md`.

  Phase 7.6's landed the same day: `ui-patterns.md`'s drag entry (Up Next reordering in both hosts, the touch bullet without `active`) and its row menu's hosts (Up Next's menu Now Playing's only), `slint-pitfalls.md`'s in-window drag entry (a source sized to its rows rather than switched off), and the README's queue line.

  Still open: `README.md`'s footprint rows, relabelled with `scripts/measure-footprint.py` when they are measured again.

## Cross-cutting
- **`settings.json` compatibility.** The new keys are `#[serde(default)]` (closed, default width), so any older file loads, and an older build ignores the keys.
- **Memory.** No new caches. What the panel holds while shown is what the sheet held while up, released on the same delay. The miniplayer now releases it too. A launch with the panel open fetches its window and prewarms its covers during boot.
- **Commits.** One per phase, with Phase 1's rename as its own commit so the move reads as a move.

## Open questions
1. ~~**The sidebar's fold toggle and Ctrl+B while a narrow window forces the rail**~~: settled in Phase 3, both stand down.
2. ~~**The ghost's shape**~~: settled in Phase 4, a Slint chip placed as Phase 6 item 5 says.
3. **Browse's card mode and folder cards** aren't in the issue's list. They can join later through the same `DragSource`, with folders resolved as `folder_actions` does.
4. **The panel widths and the content floor** (300 / 360 / 560 and 452 px) are settled in the Phase 2 and Phase 3 hand passes. Phase 3.5 keeps the 300 px minimum: once the overlay keeps the queue reachable, the minimum only decides how cramped the docked panel gets, and the slimmer rows give its title 90 px there.

## Verification
- **Every phase:**
  - `cargo fmt --all --check`;
  - `cargo clippy --all-targets --locked --workspace -- -D warnings`;
  - `cargo test --locked --workspace`, with pins the change broke updated in the same phase;
  - `scripts/update-translations.py` whenever a `@tr` string changes.
- **By hand, Kenan's pass:** the checks listed under each phase.
- **RSS gates:** release builds at the end of Phases 2 and 6. Kenan runs `/usr/bin/time -v target/release/Melodia` with the panel open, against today's sheet open.
