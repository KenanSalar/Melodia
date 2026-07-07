# Slint Native-Feature Adoption Tracker

Melodia carries a number of hand-rolled workarounds for gaps in Slint ≤ 1.16. Slint 1.17
(released 2026-06-24) shipped native replacements — or the foundations — for several of them.
This doc tracks each workaround → native-feature migration so we can retire our custom code
as upstream matures.

**Prerequisite for everything below: being on Slint ≥ 1.17.x at all.** As of 2026-07-06 we
reverted to 1.16.1 because 1.17.0 regressed us twice:

1. **Enter-transitions skipped** — 1.17 instantiates `if`/`for` eagerly at input time; our
   `ViewTransition` 1ms-Timer pattern flips `shown` before the animated bindings' first
   evaluation, so the animation never establishes a from-value. Not fixed by 1.17.1
   (upstream #12303 only covers the direct-assignment path). **Our fix:** read the animated
   properties once in `init` to force first evaluation before the timer fires (backward
   compatible with 1.16 semantics).
2. **Per-frame stutter** — 1.17 runs a whole-tree `ensure_instantiated` walk inside
   `draw_contents` every frame; on our monolithic item tree this visibly drops frames during
   animations (confirmed on the queue sheet). No in-app mitigation; needs an upstream issue
   with our numbers if it persists on ≥ 1.17.1.

Re-migration checklist lives in the plan file from the 1.17.0 attempt; once we're stable on
1.17.x, items below unblock individually.

Legend: 🟢 adoptable once on 1.17.x · 🟡 shipped upstream but wait/verify first · 🔭 upstream
foundation only — watch, not yet usable.

---

## 🔭 OS drag-and-drop → retire the vendored winit fork (the big one)

- **Today:** `winit/` vendored fork (0.30.13 + 3 Wayland-DnD commits from abandoned winit
  PR #4009), wired via `[patch.crates-io]`. Flow: `winit_filter.rs::DroppedFile` →
  `drop_coalescer.rs` → `queue_import_files`; `HoveredFile{,Cancelled}` → `Queue.is-drop-hovered`.
- **Upstream in 1.17:** `DragArea`/`DropArea` elements + `data-transfer` type — **in-process
  only** for now. Cross-application drag/drop (external file drops included) is in development
  in **winit PR #4571** (this supersedes the #1881/#4009 lineage our fork is based on; update
  the CLAUDE.md watch reference).
- **Trigger:** winit release containing #4571 + a Slint release that plumbs external drops
  into `DropArea` with file paths.
- **Migration:** delete `winit/` + the `[patch.crates-io]` block; replace the `winit_filter`
  DnD arms + `drop_coalescer` with a `DropArea` over the content panel feeding
  `queue_import_files`; re-check the queue-sheet drop gating (`is_open` atomic filter).
- **Risk:** the fork also carries the `WindowId` fix and URI percent-decoding — verify upstream
  covers both before deleting.

## 🟡 `SystemTrayIcon` element → retire the dual tray stack

- **Today:** `src/services/tray/` cfg-split (Linux `ksni` with the zbus-feature footgun; Win/mac
  `tray-icon` with deferred init + pre-exit drop), `ui/tray_bridge.rs`, embedded `tray.png`,
  restart-gated enable toggle, close-to-tray geometry-restore timer dance.
- **Upstream in 1.17:** declarative `SystemTrayIcon` element. Brand new — already accumulating
  feature requests upstream (icon-by-name, macOS template images), so let it bake a release or
  two.
- **Trigger:** a Slint release where `SystemTrayIcon` supports: dynamic menu labels
  (play/pause), tooltip updates, click actions, and graceful absence of an SNI host on Linux —
  everything `TraySnapshot`/`TrayAction` does today.
- **Migration:** replace `services/tray/` + `tray_bridge` with the element + callbacks; keep the
  close-to-tray window logic (that part is ours, not the tray lib's). Removes the ksni zbus
  pin worry entirely.
- **Risk:** feature parity on all three platforms; check idle-RSS impact per Memory Discipline.

## 🟢 Built-in `Tooltip` element → retire our tooltip component

- **Today:** `ui/components/tooltip.slint` (hand-rolled, reveal-timer at line ~43) — our
  component name shadows the new built-in, which compiles fine but is confusing long-term.
- **Upstream in 1.17:** native `Tooltip` element; 1.17.0 has a known clipping issue near window
  edges (upstream #12260) — check its status first.
- **Migration:** swap call sites (IconButton `tooltip-text`, etc.), delete our component, drop
  the name shadowing.
- **Risk:** styling parity with our popup chrome (`PopupSurface` look); reveal-delay behavior.

## 🟢 Two-way model row bindings → slim the model-patch walkers

- **Today:** optimistic favorite/rating flips walk the `VecModel` from Rust via
  `ui/model_patch.rs::patch_track_row_by_id` (+ per-view `apply_*` one-liners, `wire_row_flag!`
  macro); queue-sheet selection mirrors through `ShadowEntry` snapshots.
- **Upstream in 1.17:** two-way bindings to model row data — a row's control can write back
  into the model directly.
- **Migration:** evaluate whether star-rating / favorite toggles can bind row-fields two-way and
  let Slint propagate, keeping Rust as persistence-only. The `ShadowEntry` selection mirror is a
  UI-thread/`Send` issue, not a binding issue — likely stays.
- **Risk:** our flow is optimistic-UI + async persist + cross-surface sync
  (`sync_current_track_if_in`); two-way bindings must not bypass the persistence path. Prototype
  on one view (ratings) before committing.

## 🟡 `Window.minimized`/`maximized` + `close()`/`hide()` → drop winit accessors?

- **Today:** all window-control APIs go through `WinitWindowAccessor::with_winit_window`
  because Slint's `set_minimized`/`set_maximized` property cache stalled on Wayland; tray
  show/hide uses Slint `Window::hide/show` + `WINDOW_VISIBLE` atomic + geometry-restore timer.
- **Upstream in 1.17:** `minimized`/`maximized` in-out properties and `close()`/`hide()`
  functions on `Window`.
- **Trigger:** verified-on-Wayland behavior (KDE + GNOME) of the new properties — test in a
  scratch app first, not in Melodia.
- **Risk:** high regression surface (custom titlebar, maximize seed, resize ring gating,
  `RESPAWN_AFTER_EXIT`). The winit path works; migrate only if it meaningfully simplifies
  `window_chrome/`. Low priority.

## 🟢 `PopupWindow` geometry reactivity + `is-open` → simplify popup plumbing

- **Today:** two workarounds: (a) fixed-reserve geometry in `overflow-menu.slint` (popup always
  sized for menu + flyout because geometry was frozen at `show()`), (b) `FocusLossWatcher` +
  `PopupHighlight.id` discriminators because Slint has no popup-closed signal.
- **Upstream in 1.17:** popups react to geometry-property changes after being shown; new
  `is-open` out property. 1.17.0 also had a popup-clipping bug — fixed in 1.17.1 (#12324), so
  visual-check all popups on re-migration regardless.
- **Migration:** (a) optionally size the popup to the actual open flyout instead of the fixed
  reserve — verify it doesn't *drift* now that geometry is live; (b) `is-open` can likely
  replace parts of the `PopupHighlight` open-tracking (the winit Release-clear for row context
  menus probably stays — that's about outside-click semantics, not open-state).
- **Risk:** the fixed-reserve pattern also solves bottom-anchoring; don't unwind it without
  screenshots on both anchor directions.

## 🟢 `animate { enabled: … }` → replace duration-zero gating

- **Today:** the "don't animate drag micro-updates" pitfall is handled with
  `duration: is-dragging ? 0ms : 250ms` (sidebar width/padding, slider-adjacent animates).
- **Upstream in 1.17:** `animate` blocks take an `enabled` boolean (default true).
- **Migration:** mechanical swap to `enabled: !is-dragging` where intent is "no animation",
  keeping the duration token intact. Cleaner semantics, same behavior.
- **Risk:** trivial; verify the disabled path snaps to target (upstream implements it as
  jump-to-target).

## 🟢 `cross-axis-alignment` on box layouts → drop centering wrapper layouts

- **Today:** the "fixed-width children don't center in a wider VerticalLayout" pitfall is
  worked around by wrapping children in `HorizontalLayout { alignment: center; … }`.
- **Upstream in 1.17:** `cross-axis-alignment` property on `VerticalLayout`/`HorizontalLayout`.
- **Migration:** replace wrapper layouts with the property at the sites that exist purely for
  cross-axis centering. Cosmetic cleanup; do opportunistically.

## 🟡 Flickable animated wheel scrolling → consistency with composite-scroll routing

- **Today:** composite views (Favorites, Recently Played, Artist Detail, Browse) route
  vertical wheel through the winit layer (`CompositeScroll` + `composite-scrollbars.slint`)
  because a nested ListView swallows wheel at its scroll edge. Wheel there writes `viewport-y`
  directly (unsmoothed).
- **Upstream in 1.17:** discrete-wheel scrolling is animated (fixed ~180 ms physics decel,
  not disableable). Verified in source: the edge-swallow behavior (`EventAccepted` during the
  scroll-capture window) is **unchanged**, so the composite routing remains required.
- **On adoption:** native-wheeled views become smooth while composite views stay stepped —
  decide whether to (a) accept the inconsistency, (b) add matching smoothing to the composite
  path, or (c) leave everything as-is. Also note: an external `viewport-y` write during the
  180 ms wheel animation doesn't clear the leftover-delta state — a follow-up wheel tick can
  re-add stale distance (relevant to overlay-scrollbar thumb drags immediately after wheeling).
- **Retirement watch:** if upstream ever makes Flickable return `EventIgnored` at the edge
  (bubbling to outer scrollers), the entire composite wheel plumbing can shrink dramatically.

---

## Not yet upstream — watching

- **External drag-and-drop**: winit PR #4571 (see top item). The single biggest vendor-code
  retirement available to us.
- **Public Rust-callable translations** (`tr("…")` equivalent): would retire the
  `Settings` pure-callback `@tr` bridges for toast strings (`playlist-{import,export}-*` etc.).
  Still `i_slint_core`-internal as of 1.17.
- **`direction: rtl` / bidi-aware layouts**: blocks fa/ar/he locales; still absent in 1.17.
- **Entry/mount animation semantics**: if Slint grows a first-class "animate on mount"
  mechanism, the `ViewTransition` 1ms-Timer pattern (and its 1.17 init-read adaptation) can go.
- **Slint-native window drag without input-grab leak**: `drag_window()` from Slint
  `pointer-event(down)` still leaks the grab; our winit-layer intercept stays until upstream
  changes.
