# Plan: Queue panel (the queue docks on the right and takes drops)

Status: **in progress**, Phases 1 and 2 done, Phase 3 implemented · Owner: Kenan · Created: 2026-10-21 · Issue #136, branch `feat/queue-panel`

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
  - The heart and stars on each row, and its remove button.
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
10. **Narrow windows give way in a fixed order, decided by one constant, the content floor.**
    1. The sidebar renders as its rail. This is not persisted, and `Nav.sidebar-collapsed` stays the user's.
    2. The panel narrows toward its minimum.
    3. The panel steps aside, through `shown`, exactly as under Now Playing.

    The constant and the panel widths are settled on a real window. The track lists and grids already fit to their own width (`track-list.slint:32-33`, `GridGeometry` off `body.width`).
11. **The lifecycle follows `shown`, not `open`.** Rows and covers are released 350 ms after the panel stops being shown (`TEARDOWN_DELAY`, `queue_sheet/callbacks.rs:40`). That covers a close, Now Playing, the miniplayer and a narrow window alike. A long-open panel costs what the sheet cost while it was up: the same `QueueRows` window, the same private cover tier and the same 24-cover prewarm.

**Prior art** (two open-source desktop players' source; repo convention keeps their names out):
- **A right-hand queue panel hides below a window breakpoint rather than floating.** It reserves its width before the left sidebar measures, so the left sidebar folds first and stays folded when the panel hides in turn. Its width is clamped between a minimum and a maximum and capped to leave the content a floor. That shaped decision 10.
- **A drag carries ids and resolves them on the drop.** The drop gap is the upper or lower half of the row under the pointer, plus a few pixels of slack. The ghost is a small chip (a 20 px cover and a one-line label, at most 240 px wide, offset 8 px from the cursor). That shaped decision 7 and the ghost in Phase 6.
- **Not copied:** writing the panel width to settings on every drag move. Melodia flushes chrome state at exit, as it does for the sidebar.

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `crates/melodia-ui/ui/views/queue-panel/queue-panel.slint` (was `views/queue-sheet.slint`, 395) | The panel: width, edge handle mount, header, list host, empty state, file and drag banners, seat-on-open |
| `crates/melodia-ui/ui/views/queue-panel/queue-row.slint` (was `views/queue-row-item.slint`, 407) | One queue row, its menu and its drag source. The hand-rolled drag goes (Phase 5) |
| `crates/melodia-ui/ui/components/edge-resize-handle.slint` (new, Phase 1) | The inner-edge drag, its clamp and its double-click reset, for whichever side grows. Lifted from `sidebar.slint:195-234`, which mounts it |
| `crates/melodia-ui/ui/components/drag-source.slint` (new, Phase 5) | A `DragArea` that writes the `Drag` payload as its drag starts and clears it as it ends |
| `crates/melodia-ui/ui/components/list-drop-target.slint` (new, Phase 5) | A `DropArea` over a list: slot from the pointer, the drop line, the edge-scroll `Timer`. Mounted by the queue panel, the playlist detail and Up Next |
| `crates/melodia-ui/ui/globals/drag.slint` (new, Phase 5) | `Drag`: the in-flight payload, `active`, the token, `begin`/`end` |
| `crates/melodia-ui/ui/models.slint` | `DragPayload { kind, ids, origin, index, title, count, cover }` (Phase 5) |
| `crates/melodia-ui/ui/globals/selection.slint` | Gains `effective-ids(...)`, the one statement of what a picked row or card stands for (Phase 5) |
| `crates/melodia-ui/ui/globals/queue.slint` | `Queue` gains `panel-width` (0 = default), `shown`, `pointer-in`, `keys-owned`, `toggle()` (handled in Rust), `close()`, `shown-changed`, `drop-in(kind, ids, slot)`. It loses `dismiss()`, the `close` callback and `open-changed`; `drag-source`/`drop-slot` give way to the `Drag` payload |
| `crates/melodia-ui/ui/theme.slint` | `queue-panel-{min,default,max}-w` and `content-min-w`, beside `sidebar-*-w` (`:101-104`) |
| `crates/melodia-ui/ui/app-window.slint` (746) | The row mount, the `shown` mirror and the narrow-window derivations. The sheet's mount at `:599-604` goes |
| `crates/melodia-ui/ui/layout/shortcut-scope.slint` | `queue-keys` replaces `queue-sheet-up` |
| `crates/melodia-ui/ui/components/now-playing/up-next-list.slint` | `editable`, the row menu and the drop target (Phase 5) |
| `crates/melodia-views/src/ui/queue_panel/{mod,callbacks,rows}.rs` (was `queue_sheet/`, 135/409/114) | As today, plus `drop-in` (Phase 6) |
| `crates/melodia-views/src/ui/queue_panel/lifecycle.rs` (new, Phase 1) | Open, seat and teardown on `shown`, lifted from `callbacks.rs:244-409`. That file then holds the row actions alone |
| `crates/melodia-views/src/ui/window_chrome/winit_filter.rs` (421) | The press arm writes `keys-owned` (Phase 2). The positioned drag arms move the file-drop banners (Phase 7) |
| `crates/melodia-views/src/ui/window_chrome/drop_coalescer.rs` (142) | Routes a batch by where it was let go; `QUEUE_SHEET_OPEN` goes (Phase 7) |
| `crates/melodia-views/src/ui/callbacks/card_actions.rs` (208) | `spawn_with_track_ids` becomes `pub(crate)` (Phase 6) |
| `crates/melodia-engine/src/player/engine/queue.rs` (566) | `insert_at`, with `insert_next` written over it (Phase 6) |
| `crates/melodia-app/src/library/queue.rs` (421) | `queue_insert_at`; `queue_play_next_many` becomes one call (Phase 6) |
| `crates/melodia-ui/ui/globals/nav.slint` | `sidebar-expanded-w` (the one clamp of the stored width), `sidebar-yields` (written by `AppWindow`) and `sidebar-rail`, which every part of the sidebar draws from (Phase 3) |
| `crates/melodia-app/src/services/settings/{interface,data}.rs` | `QueuePanelLayout { queue_panel_open, queue_panel_width: Option<f64> }`, flattened with `#[serde(default)]`. `LayoutFlags` sits at clippy's bool cap (`interface.rs:166-170`) (Phase 3) |
| `crates/melodia/src/shutdown.rs`, `crates/melodia/src/boot/ui_setup/hydrate.rs` | Written at exit and hydrated before show, beside the sidebar (Phase 3) |
| `winit/src/platform_impl/{linux/wayland,linux/x11,windows}/…` | The positioned drag events (Phase 7) |

Every production Rust file in the blast radius stays well under the 800-line cap. The largest are `engine/queue.rs` at 566 and `paged_rows/mod.rs` at 704, which this plan does not grow.

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

### Phase 3: Remembered, and the narrow window
Status: implemented 2026-10-21, waiting on the hand pass. Its tests wait for Phase 8.

1. **Persistence.** `QueuePanelLayout` is written in `save_state_on_exit` beside the sidebar and hydrated beside `apply_sidebar_*` (`hydrate.rs:94-114`) before `app.show()`. A seeded open runs the lifecycle's open explicitly, since no `changed` fires on a seeded value. Older files load closed at the default width.
2. **Narrow windows** (decision 10). `Theme.content-min-w` and the three derived steps live in `app-window.slint`, and the sidebar's width binding adds the "yields" term.

**By hand:**
- restart with the panel open, closed and resized; a first launch;
- the launch must not ease the panel in from 0. That is the same concern as the sidebar's, unverified today;
- shrink and widen the window through every step, with the sidebar expanded and folded.

**Amended while implementing:**
- **Under a forced rail the sidebar's fold toggle and Ctrl+B stand down** (Kenan). This settles open question 1. They also stand down over the user's own fold while expanding wouldn't fit beside the shown panel, where a press would only flip the persisted fold with nothing on screen to show for it.
- **While the panel can't fit, the queue button, its overflow row and Q stand down** (Kenan). `AppWindow` publishes `Queue.fits` beside `shown`. It covers the miniplayer too, so Q's own `!MiniPlayer.active` gate went into it.
- **The sidebar comes back once the panel has stepped aside.** The fold is keyed on `shown`, not `open`, which is where this departs from the prior art. With both sets of controls standing down, a panel too wide to show would otherwise hold the sidebar folded with nothing left to press.
- **A seeded open needs no explicit call into the lifecycle.** `queue-shown`'s tracker takes its baseline inside `AppWindow::new()`, before hydration, so the first change-handler pass reads the seeded open as an edge.
- **`Nav.sidebar-yields` and `Queue.fits` are written from `changed` mirrors, not bound.** Bound to the window's width, they would restart the sidebar's width ease and `IconButton`'s colour ease on every resize move (`slint-pitfalls.md`, the structural-dirt entry).
- **The panel's room doesn't depend on `shown`.** So it keeps its width while it eases out from under Now Playing, rather than narrowing as the sidebar comes back.
- **The edge drag stops at the room.** It can't store a width the window won't draw, or fold the sidebar to make one.
- **So does the sidebar's, while the panel is shown.** Dragged past the room, the sidebar would yield to its rail mid-drag, unmounting its handle under the pointer and leaving `Nav.sidebar-dragging` up.
- **`Theme.content-min-w` starts at 452 px**: the narrowest body that still packs two grid columns, plus the page's insets. At the default widths the sidebar folds below a 996 px window, the panel narrows from 862 px to its minimum at 802 px, and it steps aside below that.
- **The settings keys carry the prefix their flattened siblings do**: `queue_panel_open`, `queue_panel_width`.

### Phase 4: Prove the drag primitives (a spike; nothing lands)
On a throwaway build, confirm each fact the next two phases stand on:
1. **A `DragArea` around a track row in a pan-disabled `ListView` keeps every row interaction:** click, Ctrl/Shift pick, double-click, right-click menu, checkbox, hover heart and stars. It starts a drag past 8 px.
2. **A `DropArea` over the queue list gets `can-drop` on every move** with list-local positions, and lets clicks and wheel through.
3. **A window-sized `DropArea` answering `none` still lets the one beneath accept.** If it does, the ghost is a themed Slint chip following `Drag.x/y`. If not, it is `drag-image` set to the source's row-tier cover.
4. **A drop lands and `drag-finished` still fires after the source row has scrolled out of view.** This depends on whether `ListView` recycles or destroys the instance.
5. **A `data-transfer` property on a global, set once from Rust, keeps a `DragArea` armed.**

Record each answer here as an amendment before Phase 5 starts.

### Phase 5: One drag mechanism for the reorders
1. **The shared pieces:** `DragPayload`, the `Drag` global (with the token set at install), `Selection.effective-ids` with both menus moved onto it, and the `DragSource` and `ListDropTarget` components.
2. **The queue panel and the playlist detail reorder through them.** The Rust `on_reorder` handlers keep their `(from, to)`. Deleted:
   - `queue-row.slint`'s drag state, slot maths and ghost timer (was `:58-104, 153-238`);
   - the panel's drop line and nudge guard (was `queue-sheet.slint:62-79, 299-329`);
   - `track-list-row.slint:80-114, 238-299`;
   - `draggable-track-list.slint:44-64, 163-197`;
   - `drag-source`/`drop-slot` on `Queue` and `PlaylistDetail`.
3. **Up Next in Now Playing** (decision 2).
   - It gets `ListDropTarget` over its rows and the row menu with the queue's entries minus the pick entries.
   - Rows map to play indices by the list's own `mod(base-index + idx, queue-length)`, for skip, remove and move. A move that would cross the repeat-all wrap is refused, as the queue's two no-op slots are.
   - The miniplayer host passes `editable: false`.

**Visible:**
- reorders start at 8 px rather than 4;
- edge scrolling keeps going while the pointer rests;
- a reorder may carry its row out of view, which is refused today (`queue-sheet.slint:299-311`);
- Up Next gains its menu and reorder.

Lands as one commit: a row cannot host both mechanisms.

### Phase 6: Dragging into the queue
1. **Engine and library.**
   - `QueueState::insert_at(slot, &[i64])`, with `insert_next` written over it.
   - `queue_insert_at(state, slot, ids)` beside `queue_add_tracks` (`existing_in_order`, then the splice).
   - `queue_play_next_many` becomes one call (decision 8).
2. **Sources.**
   - Every `TrackListRow` on the nine surfaces and every `CardCell` card becomes a `DragSource` of `(kind, effective ids)`, copy-allowed. The playlist's rows also allow move while reorder is on.
   - `TrackList`'s `ListView` pins `mouse-drag-pan-enabled: false`, as the click-to-act grids do.
   - A Browse row outside the library (id 0) doesn't drag, matching `collect_nonzero_track_ids`.
3. **The panel as a target.**
   - It takes outside payloads as copies and its own rows as moves. Below the last row, and the whole empty list, mean slot = length.
   - `Queue.drop-in(kind, ids, slot)` goes through `spawn_with_track_ids` into `queue_insert_at`. A slot that went stale during the resolve is clamped by the splice.
4. **What letting go will do.**
   - While a drag hovers the panel, a banner reads the payload: `Add {n} track(s) to the queue` for tracks, `Add “{}” to the queue` for a card.
   - That means new `@tr` strings: run `scripts/update-translations.py` and fill every catalog.
5. **The ghost**, in whichever shape Phase 4 settled.

**By hand:**
- a drag from each surface;
- a picked row takes the whole pick;
- a large genre onto a long queue, timed against Add to Queue on the same card;
- a drop onto the empty queue;
- a drop between rows under shuffle;
- a drop above the playing row: playback is undisturbed and the highlight follows;
- a drag released outside any target.

### Phase 7: File drops land where they are let go
1. **The fork reports the pointer during a file drag** on Wayland, X11 and Win32 (decision 9). `winit/README.md` lists the fourth change.
2. **The filter tracks that position.**
   - The queue banner shows over the panel, and the playlist banner over an open playlist elsewhere.
   - The coalescer routes each batch by where it was released. Over the panel it appends to the queue as now. Over the content it goes to an open playlist. Anywhere else it is discarded.
3. **Windows is checked on the Windows machine.**

**By hand:**
- a file onto the panel with a playlist open beside it;
- a file onto the playlist with the panel open;
- a file with the panel closed;
- an X11 session.

### Phase 8: Tests and docs (on Kenan's go)
- **New pins:**
  - `insert_at` at slot 0, at the end, and before, at and after current, under shuffle and not;
  - Play Next of many against the old loop's play order;
  - Up Next's row-to-index mapping across the wrap;
  - `effective-ids` as the only statement of the rule, by a walk;
  - no hand-rolled slot maths left outside `ListDropTarget`, by a walk;
  - `min <= default <= max` on the panel tokens;
  - the prewarm test's width table gaining the panel (`grid_prewarm_tests.rs:227-260`);
  - the filter's press arm;
  - `QueuePanelLayout` defaults and an older `settings.json`;
  - drop routing by position.
- **Docs.** Phases 1 to 3's half landed on 2026-10-21:
  - the panel's cross-file facts and the keys in `ui-patterns.md`, the narrow window's order and its two flags among them;
  - the sheet's mentions across the rules;
  - `settings.json`'s contents in the root `CLAUDE.md`;
  - the queue's feature line in `README.md`.

  Still open:
  - `ui-patterns.md` and `slint-pitfalls.md` for the drag (Phase 5), including the stale `DragArea` claim that goes with `queue-row.slint:20-24`;
  - `desktop-shell.md`'s drop flow, routed by position (Phase 7);
  - `README.md`'s footprint rows, relabelled with `scripts/measure-footprint.py` when they are measured again;
  - `winit/README.md` (Phase 7).

## Cross-cutting
- **`settings.json` compatibility.** The new keys are `#[serde(default)]` (closed, default width), so any older file loads, and an older build ignores the keys.
- **Memory.** No new caches. What the panel holds while shown is what the sheet held while up, released on the same delay. The miniplayer now releases it too. A launch with the panel open fetches its window and prewarms its covers during boot.
- **Commits.** One per phase, with Phase 1's rename as its own commit so the move reads as a move.

## Open questions
1. ~~**The sidebar's fold toggle and Ctrl+B while a narrow window forces the rail**~~: settled in Phase 3, both stand down.
2. **The ghost's shape** waits on Phase 4.
3. **Browse's card mode and folder cards** aren't in the issue's list. They can join later through the same `DragSource`, with folders resolved as `folder_actions` does.
4. **The panel widths and the content floor** (300 / 360 / 560 and 452 px) are settled in the Phase 2 and Phase 3 hand passes.

## Verification
- **Every phase:**
  - `cargo fmt --all --check`;
  - `cargo clippy --all-targets --locked --workspace -- -D warnings`;
  - `cargo test --locked --workspace`, with pins the change broke updated in the same phase;
  - `scripts/update-translations.py` whenever a `@tr` string changes.
- **By hand, Kenan's pass:** the checks listed under each phase.
- **RSS gates:** release builds at the end of Phases 2 and 6. Kenan runs `/usr/bin/time -v target/release/Melodia` with the panel open, against today's sheet open.
