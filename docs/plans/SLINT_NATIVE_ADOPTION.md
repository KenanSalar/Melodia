# Slint Native-Feature Adoption Tracker

Melodia carries hand-rolled workarounds for gaps in Slint ≤ 1.16. Slint 1.17 (2026-06-24) and
1.18 (2026-09-16) shipped native replacements for several of them, or the foundations for one.
This doc holds the migration plan from 1.16.1 to 1.18.0 and one section per workaround we can
retire once there.

## Where we stand (checked 2026-09-17)

We are on 1.16.1. We moved to 1.17.0 and reverted on 2026-07-06 because it regressed us twice:

1. **Enter-transitions skipped.** 1.17 instantiates `if`/`for` eagerly at input time; our
   `ViewTransition` 1ms-Timer pattern flips `shown` before the animated bindings' first
   evaluation, so the animation never establishes a from-value. **Our fix:** read the animated
   properties once in `init` to force first evaluation before the timer fires (backward
   compatible with 1.16 semantics).
2. **Per-frame stutter.** 1.17 runs a whole-tree `ensure_instantiated` walk inside
   `draw_contents` every frame; on our monolithic item tree this visibly drops frames during
   animations (confirmed on the queue sheet).

**1.18.0 fixes neither.** Checked in the published crate sources, not the changelog:

- **Blocker 2 is structurally unchanged.** `WindowInner::draw_contents` still opens with
  `ensure_tree_instantiated()` (`i-slint-core-1.18.0/window.rs:1846`, the same call 1.17.1 made),
  and the generated `ensure_instantiated` is the same recursive walk. Every repeater still
  recurses into every instance, and `Repeater::ensure_children_instantiated` collects
  `instances_vec()` into a fresh `Vec` per repeater on each walk (`model/repeater.rs:700`, `:883`).
  There is still **no upstream issue**: slint#11397 is the PR that introduced the pass, and
  nothing tracks its per-frame cost.
- **Blocker 1 is unverified.** 1.17.1's #12303 and 1.18's #12683 both change where an animation
  starts from, and neither targets an `if` mount flipping `shown` from a Timer. The `init`-read
  fix is backward compatible, so it goes in with the bump either way.

1.18 does ship fixes that may shrink blocker 2's cost without removing the walk (#13049,
exponential visibility checks on deeply nested elements; #13043, slow focus moves in very large
trees; #12853, smaller generated code). So the stutter has to be **re-measured, not assumed**, and
that measurement is the go/no-go for everything below.

## Migration plan: 1.16.1 → 1.18.0

A trial on 2026-09-16 (throwaway worktree, since removed) ran
`cargo clippy --all-targets --workspace` on Linux against 1.18.0. With the lockfile fix below it
compiled, and the diagnostics listed in phase 1 were the only ones. Windows-only code
(`window_chrome/parked_loop.rs`, the `tray-icon` path) wasn't compiled there.
`CustomApplicationHandler` is unchanged in the 1.18 source, but the `clippy-windows` job is the
real check.

### Phase 0: go/no-go on blocker 2

- [ ] In a scratch worktree with phase 1 applied, re-measure frame pacing on the queue sheet
      against 1.16.1, same protocol as the July attempt.
- [ ] **Still stutters:** file the upstream issue with the numbers, naming the per-frame walk in
      `draw_contents` and the per-repeater `Vec`, and stay on 1.16.1.
- [ ] **Smooth:** carry on with phase 1 in the tree.

### Phase 1: the bump

- [ ] `slint` and `slint-build` to `1.18.0` in the root `[workspace.dependencies]`.
- [ ] **Lockfile trap: also run `cargo update -p const-field-offset`** (to 0.2.1). 1.18.0's
      generated code calls `sp::compose_field_offsets`, which only exists from
      `const-field-offset` 0.2.1, while `slint` still declares `0.2.0`. A plain
      `cargo update -p slint -p slint-build` keeps 0.2.0, and `melodia-ui` then fails with 7,876
      `E0425`s. A fresh resolve would pick 0.2.1; a targeted update doesn't. Worth reporting
      upstream as a missing version floor.
- [ ] `window_chrome/winit_filter.rs:415`: `try_dispatch_event` → `dispatch_event_with_result`,
      ignoring the new `WindowEventDispatchResult` since only the error is logged. This is the one
      rustc warning in the workspace, and `-D warnings` fails the gate on it.
- [ ] `viewport-x`/`-y`/`-width`/`-height` → `content-*` on `Flickable`, `ScrollView` and
      `ListView`: 123 compiler deprecation warnings. The old names are deprecated aliases and
      build-script warnings don't trip `-D warnings`, but they bury everything else in the build
      output. The prose spelling the old names moves in the same change: CLAUDE.md's scrollbar
      convention (`offset: -sv.viewport-y` / `scroll-to`), the nested-ScrollView and
      `<=>`-on-`viewport-y` entries in `.claude/rules/slint-pitfalls.md`, and the
      `now_playing/lyrics/follow.rs` module doc. `v-viewport-height` / `v-visible-height` are our
      own properties and stay.
- [ ] Re-apply the blocker-1 fix in `crates/melodia-ui/ui/components/view-transition.slint`:
      read the animated properties once in `init`. The file is still the plain 1ms-Timer form.
- [ ] Optional: `slint-build = { version = "1.18.0", default-features = false, features = ["compat-1-18"] }`.
      `renderer-software` became a default feature that only feeds software-renderer resource
      embedding, which `crates/melodia-ui/build.rs` doesn't use.
- [ ] "Slint 1.16" in CLAUDE.md's opening line and README's stack line. The in-tree "Slint 1.16
      has no …" notes go with the code that phase 3 retires, not in this pass.

Nothing else moves. `unstable-winit-030` / `slint::winit_030` are unchanged, and
`i-slint-backend-winit` still wants `winit = "0.30.2"`, so the vendored fork's
`[patch.crates-io]` keeps applying. `WinitWindowAccessor` is unchanged, and MSRV 1.92 is fine
against our 1.97.0 pin. No new binding-loop or pure-context errors surfaced, and our `Tooltip`
shadows the built-in one without a diagnostic.

**What the bump alone buys:**

- **Window sizing:** #13245 (a width set before the window is shown was replaced by the preferred
  width), #12542 (first frame at the wrong size on Wayland, auto-size below preferred at a
  fractional scale), 1.17.1's #12262 (initial size on Wayland). All three sit in the miniplayer
  geometry code's territory.
- **macOS:** the native title bar no longer stays blank (a window is only transparent when its
  background is translucent or it has no decorations). Relevant to the native-titlebar miniplayer
  check.
- **FemtoVG:** drop shadows are no longer re-rendered every frame (#12545). Rasterized SVG icons
  are no longer blurry at fractional device-pixel positions (#6455). `Path` sits in the right
  place at scale factors other than 1 (#13230), e.g. the waveform trace.
- **Text and input:** faster layout for long texts (the lyrics panel). Focus is cleared when the
  focused item goes invisible (#11079). A `Flickable` forwards a press immediately when there's
  nothing to pan (#13117).
- **Build:** smaller, faster-compiling generated code (#12853, upstream's claim, not measured
  here), and deterministic generated code and bundled translations (#12932) for reproducible
  distro packages.
- **Dependency graph**, measured on the trial lockfile: 860 → 829 packages, and crates resolved at
  more than one version 74 → 59. `muda` 0.18 and 0.19 collapse to 0.19.2, `windows` 0.61 drops
  out, and the AVIF/EXR decoders (`rav1e`, `ravif`, `exr`) leave, since slint-build's extra image
  formats are opt-in now.

### Phase 2: manual verification

- [ ] Frame pacing on the queue sheet (phase 0), and peak RSS via `/usr/bin/time -v` against the
      current release build: eager `if`/`for` instantiation creates every mount's contents up
      front.
- [ ] Enter transitions on every nav, tab switch and the return from the miniplayer.
- [ ] Mouse-wheel scrolling now animates (1.17, fixed 180 ms, not configurable). Composite views
      stay stepped; see the `Flickable` section.
- [ ] Word-wrapped `Text` now reports its longest word as its minimum width (1.18). Check the
      miniplayer and Settings rows at the minimum window width.
- [ ] Percentage sizes no longer feed the parent's layout (1.17, #3346).
- [ ] `clip: true` with a border now clips children at the border's inner edge (1.18, #1988). One
      site: `components/dialog/playlist-mosaic-picker.slint:214`. Expected to look better, since
      the scrolled mosaic stops painting over the 1 px edge.
- [ ] Every popup: non-native popup clipping changed (1.17.1, #12324), and a popup's offset is
      recalculated when its size changes while shown (1.18).
- [ ] Locale decimal separator: 1.17 routes float→string through `Platform.decimal-separator`.
      The one fractional readout, `views/settings/playback-section.slint:144` (crossfade
      seconds), renders `2,5 s` in de/fr/es/tr/el/it. Arguably correct; check the `.po` strings
      still read naturally.
- [ ] Default font size is read from system settings on Windows/Linux (1.17). We pin
      `default-font-size` at `app-window.slint:134`, so this should be inert. Visual-check at a
      non-default system font scale anyway, since the patched Vazirmatn metrics assume our size.
- [ ] The Wayland sizing fixes against the miniplayer's restore paths, and the macOS title-bar fix
      against the native-titlebar miniplayer.

### Phase 3: adoptions

One change per section, in this order, each on a tree already on 1.18:

1. 🟢 `FlexboxLayout` → retire the row packer and the `Wrap` global (largest deletion)
2. 🟢 `PopupWindow.is-open` → slim the `PopupHighlight` plumbing
3. 🟢 `animate { enabled }` → replace duration-zero gating
4. 🟢 `cross-axis-alignment` → drop centering wrapper layouts
5. 🟢 `WindowMoveArea` → retire the drag-region press intercept
6. 🟢 Two-way model row bindings → slim the model-patch walkers (prototype first)
7. 🟡 Hangul measuring workaround → retire if 1.18's text stack composes the syllables

Legend: 🟢 adoptable once on 1.18 · 🟡 shipped upstream but wait/verify first · 🔭 upstream
foundation only, watch but not yet usable.

---

## 🟢 `FlexboxLayout` → retire the Rust row packer and the `Wrap` global

- **Today:** Slint 1.16 has no wrapping layout, so four components split their items into rows
  themselves and walk nested arrays.
  - **`MetaChipStrip`**, mounted by Now Playing (`views/now-playing-view.slint:327`) and, as
    `HeroChipStrip`, by both heroes (`components/hero/library-tab-band.slint:426`,
    `components/hero/mosaic-tab-hero.slint:189`). The strip reports its width through `measured`
    (plus a 1 ms Timer, since `changed` misses the first layout). Rust then packs `[[string]]`
    rows (`Player.chip-rows`, `HeroChips.rows`) from an *estimated* chip width: `ui/chips.rs`'s
    `CHAR_W` times the character count, plus a hand-copied `Theme.pad-sm` (`SPACING`). The split
    shape is cached in three places (`chips::split_shape`, `NowPlayingState.chip_last_width` /
    `chip_last_shape`, `hero_chips::PublishedChips.width` / `shape`), because every re-publish is
    a model reset that rebuilds each chip once per pointer motion of a resize drag.
  - **`ChipGroup`** (`components/settings/chip-group.slint`, 13 mounts across Settings,
    onboarding and the tag editor). A hidden ruler measures the widest chip, `Wrap.per-row`
    counts how many fit, `Wrap.chunk-indices` returns `[[int]]` from a Rust cache
    (`chips::IndexRows`), and `Wrap.rows-height` pins the height. Chips still draw at their
    natural width; the ruler only makes the wrap point conservative.
  - **`ColorDotGrid`** (`components/settings/color-dot-grid.slint`, 3 mounts): the same index
    split over fixed-size dots.
  - **Recent searches** (`views/search/recent-searches.slint:148`): `Wrap.pack-labels`, packed
    in Rust by estimated width with its own cache (`chips::PackedLabels`).
- **Upstream in 1.18:** `FlexboxLayout`, which wraps by default and is laid out by taffy.
  `spacing-horizontal` / `spacing-vertical` carry the chips' `pad-sm`-across / `pad-xs`-down
  split, and `alignment` covers Now Playing's centred rows and the heroes' left-aligned ones.
  Every item is measured for real, so no estimate and no ruler.
- **Migration:** each surface becomes a `FlexboxLayout` over a flat model.
  - `ui/chips.rs` and `ui/tests/chips_tests.rs` are deleted, along with the
    `ui::chips::install` call in `crates/melodia/src/boot/ui_setup/views.rs`.
  - `globals/wrap.slint` is deleted. `ColorDotGrid` still needs to know whether a dot is on the
    first row, for its tooltip side (`tip-side: r == 0 ? above : below`). The dots are fixed
    width, so that's `i < per-row` over the same arithmetic, moved into the component as a
    private function since it would be the only caller left.
  - `Player.chip-rows: [[string]]` becomes a flat `[string]`. `Player.recompute-chip-rows`,
    `NowPlayingState.chip_last_width` / `chip_last_shape` and the re-chunk in
    `now_playing/source_change.rs` all go.
  - `HeroChips.rows` becomes a flat list. `HeroChips.recompute`, `PublishedChips.width` /
    `shape` and `write_rows`'s shape check all go.
  - `MetaChipStrip` loses `measured`, its Timer and `watched-w`. Its `min-height` /
    `min-width: 0px` notes argue against the nested `VerticalLayout`, so re-derive them against
    the new layout rather than carrying them over.
- **Decide first:** a hero caps its chips at `HERO_MAX_ROWS = 2` and *drops* what doesn't fit.
  Each builder leads with the most important fact, and a band growing during a resize drag reads
  as thrashing. `FlexboxLayout` has no row cap. The likely answer is a two-row height plus
  `clip: true` on the hero strip: chips are uniform height, so clipping at exactly two rows
  hides the third row whole. Settle it before touching the heroes.
- **Risk: height-for-width.** Every host gets its height from Rust's row count or from
  `Wrap.rows-height` today. A wrapping container has to report the height it needs at the width
  its parent layout hands it. 1.18 synthesizes that constraint pass, and #12776 fixed the same
  case for word-wrapped cells. Prototype on one `ChipGroup` in a Settings card first, where a
  wrong answer is obvious and nothing else depends on it. Then do recent searches and the heroes,
  and Now Playing last, since its reserved row is what keeps the artwork from reflowing.
- **Not for the card grids.** `FlexboxLayout` doesn't virtualize, and
  `ui::grid_prewarm::cover_size` sizes the cover tier off the card width `GridGeometry` packs.
  Those stay `ListView` rows.

## 🟢 `PopupWindow` geometry reactivity + `is-open` → simplify popup plumbing

- **Today:** two workarounds: (a) fixed-reserve geometry in `overflow-menu.slint` (popup always
  sized for menu + flyout because geometry was frozen at `show()`), (b) `FocusLossWatcher` +
  `PopupHighlight.id` discriminators (`globals/shell.slint:176`) because Slint has no
  popup-closed signal.
- **Upstream in 1.17:** popups react to geometry-property changes after being shown; new
  `is-open` out property, confirmed in the 1.18 source as `true` while shown and `false` after
  any close (a dismissing click, a selection, or `close()`). 1.17.0 also had a popup-clipping bug
  fixed in 1.17.1 (#12324), so visual-check all popups regardless (phase 2).
- **Migration:** (a) optionally size the popup to the actual open flyout instead of the fixed
  reserve, verifying it doesn't *drift* now that geometry is live; (b) a trigger in the same
  component as its popup binds `force-bg: pop.is-open` instead of the `PopupHighlight.id`
  discriminator. Ids aren't readable across components, so a trigger whose popup lives elsewhere
  keeps the global. The winit Release-clear for row context menus probably stays, since that's
  about outside-click semantics, not open state.
- **Risk:** the fixed-reserve pattern also solves bottom-anchoring; don't unwind it without
  screenshots on both anchor directions.

## 🟢 `animate { enabled: … }` → replace duration-zero gating

- **Today:** the "don't animate drag micro-updates" pitfall is handled with
  `duration: is-dragging ? 0ms : …`, at `layout/sidebar.slint:71`, `:132` and `:186` (rail width
  and row padding).
- **Upstream in 1.17:** `animate` blocks take an `enabled` boolean (default true).
- **Migration:** swap to `enabled: !is-dragging` where the intent is "no animation", keeping the
  duration token intact. Cleaner semantics, same behavior.
- **Risk:** trivial; verify the disabled path snaps to target (upstream implements it as
  jump-to-target).

## 🟢 `cross-axis-alignment` on box layouts → drop centering wrapper layouts

- **Today:** the "fixed-width children don't center in a wider VerticalLayout" pitfall is worked
  around by wrapping children in `HorizontalLayout { alignment: center; … }`.
- **Upstream:** `cross-axis-alignment` on `VerticalLayout`/`HorizontalLayout` (1.17). 1.18 adds
  the per-child `cross-axis-self-alignment` and fixes min/max size constraints being ignored when
  the alignment isn't `stretch` (#8988), which is what makes the property safe on children
  carrying a `min-width` or `max-width`.
- **Migration:** replace the wrapper layouts that exist purely for cross-axis centering.
  Cosmetic cleanup; do opportunistically.

## 🟢 `WindowMoveArea` → retire the drag-region press intercept

- **Today:** dragging the custom titlebar and the miniplayer goes through the winit layer,
  because `drag_window()` called from a Slint `pointer-event(down)` leaks the input grab. The
  titlebar and miniplayer `TouchArea`s report hover as a `DragRegion` via
  `WindowChrome.drag-region-changed` into a cell. `winit_filter.rs` intercepts
  `MouseInput { Pressed, Left }` over a region, calls `drag_window()` and returns
  `PreventDefault`. Slint never sees those presses, so the titlebar's double press to maximize is
  read in Rust too (`window_chrome/drag_region.rs`'s `DoublePress`).
- **Upstream in 1.18:** `WindowMoveArea`. Verified in `i-slint-core-1.18.0`
  (`items/input_items.rs`, `items/drag_n_drop.rs`):
  - A left press is forwarded to the children first (`ForwardAndInterceptGrab`), so buttons
    inside stay clickable.
  - The element takes over only once the pointer passes `Flickable`'s drag threshold, then calls
    `drag_window()` through the backend's `start_window_move`. A plain click never moves the
    window, and the move starts on the drag, not on the press.
  - It has an `enabled` property, and no double-click or resize handling.
- **Migration:** wrap the titlebar and miniplayer drag regions in `WindowMoveArea`, and give the
  titlebar a `double-clicked` on its `TouchArea`, which fires once Slint sees the presses. The
  miniplayer gets none, since a double press there only moves the window. Delete:
  - `window_chrome/drag_region.rs`
  - the `DragRegion` enum and `WindowChrome.drag-region-changed`
  - the drag-region `MouseInput` arm in `winit_filter.rs`

  The **resize** arm, `resize_grab.rs`, `resize_release.rs` and `ResizeZone` stay: nothing in
  1.18 starts an OS resize. The DnD arms and the `MouseWheel` / `CompositeScroll` arm are
  unrelated and stay. Set the new `window-title-bar` accessible role on the titlebar in the same
  change.
- **Risks, all manual checks:**
  - The OS owns the pointer for the move, so Slint never sees that press's release. Check that
    the next click on a titlebar button lands on the button.
  - The miniplayer's region runs to the window edge. The resize arm keeps running in the winit
    filter ahead of Slint's dispatch, so a press on a grab still resizes, but check the handoff
    at the edge.
  - On Windows the move now starts from inside Slint's item dispatch rather than the winit
    filter. Check `parked_loop` still ticks during a drag.
  - A move that starts after a few pixels, not on press, is a feel change on the custom titlebar.

## 🟢 Two-way model row bindings → slim the model-patch walkers

- **Today:** optimistic favorite/rating flips walk the `VecModel` from Rust via
  `crates/melodia-views/src/ui/model_patch.rs::patch_track_row_by_id` (+ per-view `apply_*` one-liners, `wire_row_flag!`
  macro); queue-sheet selection mirrors through `ShadowEntry` snapshots.
- **Upstream in 1.17:** two-way bindings to model row data — a row's control can write back
  into the model directly.
- **Migration:** evaluate whether star-rating / favorite toggles can bind row-fields two-way and
  let Slint propagate, keeping Rust as persistence-only. The `ShadowEntry` selection mirror is a
  UI-thread/`Send` issue, not a binding issue — likely stays.
- **Risk:** our flow is optimistic-UI + async persist + cross-surface sync
  (`sync_current_track_if_in`); two-way bindings must not bypass the persistence path. Prototype
  on one view (ratings) before committing.

## 🟡 Hangul measuring workaround → retire if the new text stack composes syllables

- **Today:** `now_playing/lyrics/measure.rs`'s `OPEN_HANGUL_EMS = 2.0` charges a Hangul syllable
  with no final consonant two ems, because Slint 1.16 draws it as two loose jamo.
  `.claude/rules/slint-pitfalls.md` traces that to the generic families appended to every font
  stack, and reproduces it against parley 0.8 with no Slint involved.
- **Upstream in 1.18:** the stack shape is unchanged: a named family, then `SansSerif`, then
  `SystemUi` (`i-slint-common-1.18.0/sharedfontique.rs:188`, consumed at
  `i-slint-core-1.18.0/textlayout/sharedparley/shaping.rs:86`). But parley moved 0.8 → 0.11 and
  harfrust 0.5 → 0.12 underneath it, so the composition bug may be gone.
- **Trigger:** a Korean lyrics sheet shows composed syllables on 1.18.
- **Migration:** delete `OPEN_HANGUL_EMS`, `is_open_hangul_syllable` and the measuring branch
  that reads them, plus the pitfall entry.

## 🟡 Flickable animated wheel scrolling → consistency with composite-scroll routing

- **Today:** composite views (Favorites, Recently Played, Artist Detail, Browse) route
  vertical wheel through the winit layer (`CompositeScroll` + `composite-scrollbars.slint`)
  because a nested ListView swallows wheel at its scroll edge. Wheel there writes the scroll
  offset directly (unsmoothed).
- **Upstream in 1.17:** discrete-wheel scrolling is animated (fixed 180 ms physics decel, not
  disableable). Verified in source: the edge-swallow behavior (`EventAccepted` during the
  scroll-capture window) is **unchanged**, so the composite routing remains required.
- **Still true in 1.18** (`i-slint-core-1.18.0/items/flickable.rs:465`): `TouchPhase::Started`
  captures without checking the delta's direction, so `route_wheel`'s `Unphased` re-dispatch
  stays too. 1.18 also adds `Flickable.mouse-drag-pan-enabled` and renames the scroll offsets
  to `content-*` (phase 1).
- **On adoption:** native-wheeled views become smooth while composite views stay stepped. Decide
  whether to (a) accept the inconsistency, (b) add matching smoothing to the composite path, or
  (c) leave everything as-is. Also note: an external offset write during the 180 ms wheel
  animation doesn't clear the leftover-delta state, still the case in 1.18. A follow-up wheel
  tick can re-add stale distance (relevant to overlay-scrollbar thumb drags immediately after
  wheeling).
- **Retirement watch:** if upstream ever makes Flickable return `EventIgnored` at the edge
  (bubbling to outer scrollers), the entire composite wheel plumbing can shrink dramatically.

---

*The sections below need an upstream change before they're adoptable.*

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
  2026-07-20.
- **Still blocked through 1.18 (checked 2026-09-17).** winit's newest release is
  `0.31.0-beta.3` (2026-09-04), with no stable 0.31. slint#11243 "Upgrade to winit 0.31" is open,
  and `feature/winit-0.31`'s last commit is 2026-07-20. 1.18's `DataTransfer` gained a list of
  file paths (#1967), but its winit backend still handles no `DroppedFile` / `HoveredFile`, so
  nothing produces them for us.
- **Trigger:** a winit **0.31 release** carrying #4571 + a Slint release cut from
  `feature/winit-0.31` that surfaces external drops as file paths on `DropArea`. This also means
  `unstable-winit-030` → `unstable-winit-031` and a `slint::winit_030` → `winit_031` rename
  across the nine files importing it today (`main.rs`, `shell/tray_bridge.rs` and seven under
  `window_chrome/`). The fork retirement and the winit major bump land together.
- **Migration:** delete `winit/` + the `[patch.crates-io]` block; replace the `winit_filter`
  DnD arms + `drop_coalescer` with a `DropArea` over the content panel feeding
  `queue_import_files`; re-check the queue-sheet drop gating (`is_open` atomic filter).
- **Risk:** #4571 is a **rewrite** around a new `DataTransfer` abstraction, not a continuation
  of #4009 — so the fork's `WindowId` fix and URI percent-decoding aren't "did they take our
  commits" questions, they're entirely different code. Re-test percent-decoded paths (spaces,
  non-ASCII) empirically on the new API before deleting the fork.
- **Also update `CLAUDE.md` on retirement** — the fork's provenance, its retirement condition and
  the `unstable-winit-030` → `-031` rename now sit in one Known Gaps bullet (search `Vendored winit
  fork`), which is what has to go with the directory. winit#1881 is still open but #4571 supersedes
  it in practice.

## 🟡 `SystemTrayIcon` element → retire the dual tray stack

- **Today:** `crates/melodia-platform/src/services/platform/tray/` cfg-split (Linux `ksni` with the
  zbus-feature footgun; Win/mac `tray-icon` with deferred init + pre-exit drop),
  `crates/melodia-views/src/ui/shell/tray_bridge.rs`, embedded `tray.png`,
  restart-gated enable toggle, close-to-tray geometry-restore timer dance.
- **Upstream:** declarative `SystemTrayIcon` element (1.17). As of 1.18 it carries `icon`,
  `tooltip`, `visible`, `title`, a `clicked` callback and a `Menu` child. 1.18 also fixed
  `show()`/`hide()` and reports an error when no tray backend is available. On Linux it's built
  on `ksni` 0.3 with `blocking` + `async-io` (`i-slint-core`'s `system-tray` feature), the same
  features we pin, so adopting it wouldn't reopen the zbus footgun.
- **Trigger:** verified on all three platforms: menu labels that follow bindings (play/pause),
  tooltip updates, click actions, and an SNI-less Linux session degrading rather than aborting.
  That's everything `TraySnapshot`/`TrayAction` does today.
- **Migration:** replace `services/tray/` + `tray_bridge` with the element + callbacks; keep the
  close-to-tray window logic (that part is ours, not the tray lib's). Removes the ksni zbus
  pin worry entirely.
- **Risk:** feature parity on all three platforms; check idle-RSS impact per Memory Discipline.

## 🟡 Built-in `Tooltip` element → retire our tooltip component

- **Today:** `crates/melodia-ui/ui/components/tooltip.slint`, whose name shadows the built-in. It
  compiles silently on 1.18 (trial) but is confusing long-term. Seventeen mounts:
  - **Nine anchored in-tree:** `action-pill.slint:146`, `icon-button.slint:137`,
    `macos-traffic-light.slint:72`, `caption-buttons.slint:132`,
    `now-playing/play-button.slint:153`, `now-playing/lyrics-controls.slint:71`,
    `settings/color-dot-grid.slint:45`, `now-playing/volume-popup.slint:280`,
    `now-playing/volume-popup-horizontal.slint:159`.
  - **Eight top-layer:** seven `TooltipFrame` mounts (`radio-view.slint:280`,
    `browse-view.slint:340`, `recently-played-view.slint:197`, `settings-view.slint:193`,
    `favorites-view.slint:239`, `my-library-view.slint:463` and `:472`) plus the collapsed
    rail's tip (`app-window.slint:607`).

  The top-layer ones are the awkward half of any migration: they exist precisely because the pill
  has to be drawn somewhere other than on its anchor, which a built-in anchored element can't
  express.
- **1.18's runtime `z` doesn't retire the top-layer frames.** `z` only re-sorts siblings. Where a
  band's tooltip is covered by the body declared after it, band and body are already siblings
  (checked in My Library and Settings), and a constant `z` was available in 1.16 too. The rail tip
  is clipped by `nav-scroll`, which no `z` escapes.
- **Upstream in 1.17:** native `Tooltip` element.
- **Blocked (re-checked 2026-09-17):** slint#12260 *"Tooltip is clipped when the anchor widget is
  near the edge of the window"* is **still open**, with no activity since 2026-06-29. The 1.17.1
  popup-clipping fix (#12324) is a different bug. Several of our call sites are edge-adjacent
  (`caption-buttons.slint:132`, `macos-traffic-light.slint:72`), so adopting today would regress
  them.
- **Trigger:** #12260 closed and released.
- **Migration:** swap call sites (IconButton `tooltip-text`, etc.), delete our component, drop
  the name shadowing.
- **Risk:** styling parity with our popup chrome (`PopupSurface` look); reveal-delay behavior.

## 🟡 `Window.minimized`/`maximized` + `close()`/`hide()` → drop winit accessors?

- **Today:** all window-control APIs go through `WinitWindowAccessor::with_winit_window`
  because Slint's `set_minimized`/`set_maximized` property cache stalled on Wayland; tray
  show/hide uses Slint `Window::hide/show` + `WINDOW_VISIBLE` atomic + geometry-restore timer.
- **Upstream in 1.17:** `minimized`/`maximized` in-out properties and `close()`/`hide()`
  functions on `Window`. 1.18 makes `close()` return whether the close was accepted and deprecates
  setting `Window.x`/`y`; the root sets neither. Nothing in 1.18 addresses the Wayland cache.
- **Trigger:** verified-on-Wayland behavior (KDE + GNOME) of the new properties — test in a
  scratch app first, not in Melodia.
- **Risk:** high regression surface (custom titlebar, maximize seed, resize ring gating,
  `RESPAWN_AFTER_EXIT`). The winit path works; migrate only if it meaningfully simplifies
  `window_chrome/`. Low priority.

---

## Not yet upstream — watching

- **Public Rust-callable translations** (`tr("…")` equivalent): would retire the
  `Settings` pure-callback `@tr` bridges for toast strings (`playlist-{import,export}-*` etc.).
  Still `i_slint_core`-internal in 1.18, which only adds `slint::update_all_translations()`
  (re-renders everything; it translates nothing from Rust).
- **`direction: rtl` / bidi-aware layouts**: blocks fa/ar/he locales; still absent in 1.18.
  `layout-order` and `FlexboxLayout`'s `row-reverse` are building blocks, not bidi.
- **`string.contains()`**: 1.18 adds `starts-with`, `ends-with` and `replace-all` only, so the
  Settings search's row-visibility test (`globals/settings-page.slint:96`,
  `ui/settings/settings_page.rs:62`) stays in Rust.
- **Entry/mount animation semantics**: if Slint grows a first-class "animate on mount"
  mechanism, the `ViewTransition` 1ms-Timer pattern (and its `init`-read adaptation) can go.
- **Runtime `z` (1.18): no consumer.** Nothing in the tree sets `z`, and no stacking flips at
  runtime. It becomes useful with new work that needs it, such as a dragged queue row lifted above
  its neighbours.

### Backdrop blur / frosted glass — checked 2026-07-25, status re-checked 2026-09-17

- **Status 2026-09-17:** 1.18 adds no backdrop blur. slint#2066 has no maintainer comment since
  the 2026-06-29 one quoted below; its newest activity is a user use-case comment (2026-09-13).
  1.18's experimental Vello renderer doesn't change the FemtoVG caveat.
- **Wanted for:** frosted-glass fills on the Now-Playing metadata chips (`MetaChip`,
  `crates/melodia-ui/ui/views/now-playing-view.slint`) and the Up Next row hover slab
  (`crates/melodia-ui/ui/components/now-playing/up-next-list.slint`). Both currently fake depth with a flat
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
  used for the NP backdrop (`crates/melodia-views/src/ui/now_playing_artwork.rs`), and it degrades badly for these
  two surfaces — chips reflow across rows on resize, the hover slab moves per row and scrolls,
  so each would need its own correctly-offset crop recomputed on every layout change. The flat
  tint is the right stand-in until upstream lands.

*Promoted out of this list on 2026-07-23: external drag-and-drop (winit #4571 merged) and
Slint-native window drag (`WindowMoveArea` on master) — both now have their own sections.
2026-09-17: the 1.18.0 migration plan replaced the re-migration checklist; `WindowMoveArea` moved
to 🟢 on its release; `FlexboxLayout` and the Hangul workaround got sections of their own.*
