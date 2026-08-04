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

**Upstream status — checked 2026-07-23.** 1.17.1 (2026-07-07) is still the newest release; no
1.17.2, and master's `CHANGELOG.md` has no Unreleased section. Neither regression has a
released fix, so there is nothing new to re-attempt yet. Blocker 2 has **no upstream issue at
all** — slint#11397 is the closed work that *introduced* the eager `ensure_instantiated` pass,
and nothing tracks its per-frame cost. Filing that issue with our queue-sheet numbers is the
highest-value action available: every 🟢 item below queues behind it.

## Re-migration checklist

Run this when a release lands that clears both blockers. (The 1.17.0-attempt plan file this
used to point at is gone — the checklist lives here now.)

- [ ] Bump `slint` (`Cargo.toml:73`) + `slint-build` (`Cargo.toml:84`) + `Cargo.lock`. Both
      live in `[workspace.dependencies]` since the `melodia-ui` split, not `[dependencies]`. No
      feature renames touch our set — `unstable-winit-030` and `slint::winit_030` are
      unchanged in 1.17.x, and `i-slint-backend-winit` still wants `winit = "0.30.2"`, so the
      vendored fork's `[patch.crates-io]` keeps applying. MSRV 1.92 vs our 1.97.0 pin: fine.
- [ ] Re-apply the blocker-1 fix in `melodia-ui/ui/components/view-transition.slint` — read the animated
      properties once in `init` so 1.17's eager instantiation still establishes a from-value.
      It was reverted with the rollback; the file is currently back to the plain 1ms-Timer form.
- [ ] Re-measure frame pacing on the queue sheet (blocker 2) before touching anything else.
- [ ] Peak RSS via `/usr/bin/time -v` against the current release build — eager `if`/`for`
      instantiation touches ~134 `if`-gated mounts.
- [ ] **Locale decimal separator** — 1.17 routes float→string through the locale's separator
      (exposed as `Platform.decimal-separator`). Exactly one site yields a fractional value:
      `melodia-ui/ui/views/settings/playback-section.slint:138`, which renders `2,5 s` instead of `2.5 s`
      in de/fr/es/tr/el/it. Arguably the correct localization; check the `.po` strings still
      read naturally. Everything else is integer-valued (the `round(…)` dB/volume readouts) or
      a string literal (`current-speed-label()`), so unaffected.
- [ ] Default font size is now read from system settings on Windows/Linux. We pin
      `default-font-size` at `melodia-ui/ui/app-window.slint:129` and have a single `Window` root, so this
      should be inert — visual-check on a non-default font scale anyway, since the patched
      Vazirmatn metrics assume our size.
- [ ] Visual-check every popup: 1.17.1 changed non-native popup clipping (#12324).
- [ ] `Tooltip` name shadowing is benign (all 7 call sites import ours explicitly), but confirm
      the compiler stays quiet about it.

Legend: 🟢 adoptable once on 1.17.x · 🟡 shipped upstream but wait/verify first · 🔭 upstream
foundation only — watch, not yet usable.

---

## 🟡 OS drag-and-drop → retire the vendored winit fork (the big one)

- **Today:** `winit/` vendored fork (0.30.13 + 3 Wayland-DnD commits from abandoned winit
  PR #4009), wired via `[patch.crates-io]`. Flow: `winit_filter.rs::DroppedFile` →
  `drop_coalescer.rs` → `queue_import_files`; `HoveredFile{,Cancelled}` → `Queue.is-drop-hovered`.
- **Upstream in 1.17:** `DragArea`/`DropArea` elements + `data-transfer` type — **in-process
  only** in the released 1.17.x.
- **Moved 2026-07-16 (was 🔭):** winit **PR #4571 "New drag and drop API" merged**. Receive
  *and* initiate on Wayland/Windows/macOS; X11 receive-only (initiating explicitly out of
  scope). Written expressly to support Slint's DnD work (slint#1967, closed 2026-07-19). Slint
  is already plumbing it on the **`feature/winit-0.31` branch** — `Add native drag-and-drop to
  the winit backend (#12294)` on 2026-07-17, `Support dragging and dropping images (#12549)` on
  2026-07-20; tracking issue slint#11243 still open. Both halves of the trigger are now in
  motion rather than speculative — this is the nearest-term large retirement available.
- **Trigger:** a winit **0.31 release** carrying #4571 (newest tag is 0.31.0-beta.2) + a Slint
  release cut from `feature/winit-0.31` that surfaces external drops as file paths on
  `DropArea`. Note this also means `unstable-winit-030` → `unstable-winit-031` and a
  `slint::winit_030` → `winit_031` rename across `winit_filter.rs`, `main.rs`,
  `src/ui/albums/grid.rs` — the fork retirement and the winit major bump land together.
- **Migration:** delete `winit/` + the `[patch.crates-io]` block; replace the `winit_filter`
  DnD arms + `drop_coalescer` with a `DropArea` over the content panel feeding
  `queue_import_files`; re-check the queue-sheet drop gating (`is_open` atomic filter).
- **Risk:** #4571 is a **rewrite** around a new `DataTransfer` abstraction, not a continuation
  of #4009 — so the fork's `WindowId` fix and URI percent-decoding aren't "did they take our
  commits" questions, they're entirely different code. Re-test percent-decoded paths (spaces,
  non-ASCII) empirically on the new API before deleting the fork.
- **Also update `CLAUDE.md` on retirement** — three sites still cite the superseded lineage:
  `CLAUDE.md:130` (fork provenance, PR #4009), `:192` ("winit#1881 open since 2021"), `:254`
  ("Retires … when upstream lands native Wayland DnD (winit#1881)"). winit#1881 is still open
  but #4571 supersedes it in practice.

## 🟡 `SystemTrayIcon` element → retire the dual tray stack

- **Today:** `src/services/tray/` cfg-split (Linux `ksni` with the zbus-feature footgun; Win/mac
  `tray-icon` with deferred init + pre-exit drop), `src/ui/tray_bridge.rs`, embedded `tray.png`,
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

## 🟡 Built-in `Tooltip` element → retire our tooltip component

- **Today:** `melodia-ui/ui/components/tooltip.slint` (hand-rolled, `reveal-timer` at line 55) — our
  component name shadows the new built-in, which compiles fine but is confusing long-term.
  Seven call sites, all importing ours explicitly: `icon-button.slint:119`,
  `macos-traffic-light.slint:112`, `action-pill.slint:182`, `settings/color-dot-grid.slint:43`,
  `playlists-view.slint:321`, `custom-titlebar.slint:83`, `now-playing/play-button.slint:160`.
- **Upstream in 1.17:** native `Tooltip` element.
- **Blocked (was 🟢; re-checked 2026-07-23):** slint#12260 *"Tooltip is clipped when the anchor
  widget is near the edge of the window"* is **still open** (filed 2026-06-26). The 1.17.1
  popup-clipping fix (#12324) is a *different* bug — non-native popups — and does not resolve
  it. Several of our call sites are edge-adjacent (`custom-titlebar.slint:83`,
  `macos-traffic-light.slint:112`), so adopting today would regress them.
- **Trigger:** #12260 closed and released.
- **Migration:** swap call sites (IconButton `tooltip-text`, etc.), delete our component, drop
  the name shadowing.
- **Risk:** styling parity with our popup chrome (`PopupSurface` look); reveal-delay behavior.

## 🟢 Two-way model row bindings → slim the model-patch walkers

- **Today:** optimistic favorite/rating flips walk the `VecModel` from Rust via
  `src/ui/model_patch.rs::patch_track_row_by_id` (+ per-view `apply_*` one-liners, `wire_row_flag!`
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

## 🔭 `WindowMoveArea` → retire the winit drag-window intercept

- **Today:** dragging the custom titlebar goes through the winit layer because `drag_window()`
  called from a Slint `pointer-event(down)` leaks the input grab. A `TouchArea` reports
  `has-hover` via `WindowChrome.drag-region-hover-changed` into an atomic, and
  `winit_filter.rs` intercepts `MouseInput { Pressed, Left }` when that atomic is true →
  `drag_window()` → `PreventDefault`.
- **Upstream:** `WindowMoveArea` element — landed on master **2026-07-07**, **not in 1.17.1**
  (verified against the `v1.17.1` tag), so it ships in 1.18. Its own docs target our exact
  case: *"such as a custom title bar in a window without native decorations (`no-frame: true`)
  … A plain click doesn't move the window, so child elements like TouchArea stay interactive.
  The windowing system performs the move."* That last clause is the grab-leak problem we
  worked around. Carries an `enabled` property, so the maximized/native-titlebar gating stays
  expressible declaratively.
- **Trigger:** the 1.18 release (plus the two blockers above cleared).
- **Migration:** wrap the titlebar drag region in `WindowMoveArea`; delete the
  `drag-region-hover-changed` callback, its atomic, and the `MouseInput` arm in
  `winit_filter.rs`. The DnD arms and the `MouseWheel`/`CompositeScroll` arm in that file are
  unrelated and stay.
- **Risk:** verify the drag threshold doesn't swallow clicks on titlebar buttons (traffic
  lights, window controls) — that's the whole reason we route through `has-hover` today.

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

- **Public Rust-callable translations** (`tr("…")` equivalent): would retire the
  `Settings` pure-callback `@tr` bridges for toast strings (`playlist-{import,export}-*` etc.).
  Still `i_slint_core`-internal as of 1.17.
- **`direction: rtl` / bidi-aware layouts**: blocks fa/ar/he locales; still absent in 1.17.
- **Entry/mount animation semantics**: if Slint grows a first-class "animate on mount"
  mechanism, the `ViewTransition` 1ms-Timer pattern (and its 1.17 init-read adaptation) can go.

### Backdrop blur / frosted glass — checked 2026-07-25

- **Wanted for:** frosted-glass fills on the Now-Playing metadata chips (`MetaChip`,
  `melodia-ui/ui/views/now-playing-view.slint`) and the Up Next row hover slab
  (`melodia-ui/ui/components/now-playing/up-next-list.slint`). Both currently fake depth with a flat
  `Player.np-accent-bright.with-alpha(0.16)` tint over the blurred-artwork backdrop.
- **Status: absent upstream, and not on the roadmap.** Verified in source, not docs:
  `i-slint-compiler`'s `typeregister.rs` registers exactly two blur-typed properties in the
  whole language — `drop-shadow-blur` (:221) and `inner-shadow-blur` (:229) — plus
  `BoxShadow`'s own `blur` (`builtins.slint:1589`). All three describe a shadow the element
  *casts*; nothing reads back the pixels underneath, which is the prerequisite. `opacity` is
  not a substitute — fixed-function blending, not a filter over the backdrop.
- **Tracking:** slint#2066 *"Add first-class support for blurring what's underneath a
  Rectangle"* (2023-01, 17 👍, no milestone/assignee/PR) and slint#612 *"Compositing /
  effects"* (2021-10, 18 👍). General form: slint#10887 *"Custom Shader support"* (2026-02,
  labelled `a:renderer-femtovg`, quiet since March).
- **Newest maintainer word — slint#2066, 2026-06-29 (eira-fransham):** "We've been talking
  about implementing arbitrary shaders for a while, with blur just being a special case, but
  it's a big topic. I made the suggestion the other day that blur is common enough that it
  would be worth special-casing … but whether that translates to it coming up soon on the
  roadmap is a different question." Discussed internally, some agreement to special-case blur
  ahead of general shaders, explicitly uncommitted.
- **⚠ Renderer caveat.** The most concrete path a maintainer has named (tronical, 2024-12) is
  a read-back into an offscreen surface via **Skia**'s `SaveLayerRec`, which "would probably
  work with Skia out of the box". We render with **FemtoVG**. A release announcing backdrop
  blur is therefore not automatically a release where *we* get it — check the renderer before
  planning any work.
- **Not the same thing: OS window blur.** slint#2339 *"Blurred window"* has recent movement
  (winit gained blur support 2026-04; KDE 6.7 swapped `org_kde_kwin_blur_manager` for
  `ext_background_effect_manager_v1`; winit PR #4580 backports it to the 0.30.x branch our
  vendored fork sits on). That blurs the **desktop behind the window** — our chips sit over
  the app's own opaque gradient + artwork blur, so it can never reach them. Relevant to the
  window shell, irrelevant here.
- **Trigger:** a Slint release whose changelog names a backdrop/background-blur property on
  `Rectangle` **and** covers FemtoVG (or we've moved renderers by then).
- **Migration:** swap the two `.with-alpha(0.16)` accent fills for the blur property plus a
  much lighter tint; re-check legibility of the solid `np-accent-bright` chip label and "Up
  Next" heading against a blurred rather than tinted ground. Measure — a per-element backdrop
  read-back every frame is exactly what Memory Discipline exists for.
- **Meanwhile:** the only in-engine approximation is the pre-blurred-`Image` trick already
  used for the NP backdrop (`src/ui/now_playing_artwork.rs`), and it degrades badly for these
  two surfaces — chips reflow across rows on resize, the hover slab moves per row and scrolls,
  so each would need its own correctly-offset crop recomputed on every layout change. The flat
  tint is the right stand-in until upstream lands.

*Promoted out of this list on 2026-07-23: external drag-and-drop (winit #4571 merged) and
Slint-native window drag (`WindowMoveArea` on master) — both now have their own sections.*
