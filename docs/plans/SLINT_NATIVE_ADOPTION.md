# Slint Native-Feature Adoption Tracker

Melodia carries hand-rolled workarounds for gaps in Slint ≤ 1.16. Slint 1.17 (2026-06-24) and
1.18 (2026-09-16) shipped native replacements for several of them, or the foundations for one.
This doc holds the migration plan from 1.16.1 to 1.18.1 and one section per workaround we can
retire once there.

## Where we stand (checked 2026-09-17, re-checked against 1.18.1 on 2026-10-11)

We are on 1.16.1. We moved to 1.17.0 and reverted on 2026-07-06 because it regressed us twice:

1. **Enter-transitions skipped.** 1.17 instantiates `if`/`for` eagerly at input time; our
   `ViewTransition` 1ms-Timer pattern flips `shown` before the animated bindings' first
   evaluation, so the animation never establishes a from-value. **Our fix:** read the animated
   properties once in `init` to force first evaluation before the timer fires (backward
   compatible with 1.16 semantics).
2. **Per-frame stutter.** 1.17 runs a whole-tree `ensure_instantiated` walk inside
   `draw_contents` every frame; on our monolithic item tree this visibly drops frames during
   animations (confirmed on the queue sheet).

**1.18.1 fixes neither.** Checked in the published crate sources, not the changelog:

- **Blocker 2 is structurally unchanged.** `WindowInner::draw_contents` still opens with
  `ensure_tree_instantiated()` (`i-slint-core-1.18.1/window.rs:1846`, now inside
  `evaluate_no_tracking`, the same call 1.17.1 made), and the generated `ensure_instantiated` is
  the same recursive walk. Every repeater still recurses into every instance, and
  `Repeater::ensure_children_instantiated` collects `instances_vec()` into a fresh `Vec` per
  repeater on each walk (`model/repeater.rs:702`, `:883`). Master only adds a loop over overlays.
  There is still **no upstream issue**: slint#11397 is the PR that introduced the pass, #13621
  (merged after 1.18.1) only moves `ComponentContainer` building into it, and nothing tracks its
  per-frame cost.
- **Blocker 1 is unverified.** 1.17.1's #12303 and 1.18's #12683 both change where an animation
  starts from, and neither targets an `if` mount flipping `shown` from a Timer. The `init`-read
  fix is backward compatible, so it goes in with the bump either way.

1.18 does ship fixes that may shrink blocker 2's cost without removing the walk (#13049,
exponential visibility checks on deeply nested elements; #13043, slow focus moves in very large
trees; #12853, smaller generated code). So the stutter has to be **re-measured, not assumed**, and
that measurement is the go/no-go for everything below.

**Outcome (2026-10-11):** neither blocker shows on 1.18.1. Enter transitions play with the `init`
read in place, and the queue sheet and nav transitions run as smooth as on 1.16.1, though the
per-frame walk is still in the source.

## Migration plan: 1.16.1 → 1.18.1

**Status (2026-10-12):** phases 0 and 1 landed together as `24453338` on
`refactor/upgrade-dependencies`, since neither half validates alone. fmt, clippy and `cargo test`
pass on Linux. Phase 0 is a go: on the manual run the queue sheet and the nav transitions were as
smooth on 1.18.1 as on 1.16.1, so the bump stays. Phase 2 passed on the manual pass, on Linux,
once the miniplayer backdrop's fade-in was fixed on top of the bump. Phase 4 landed as `75493151`
and passed its manual pass on a Wayland touchpad, and the composite views keep their own scrolling
(the decision after it). Phase 3 is under way: items 1–3 landed and passed by hand
(`FlexboxLayout` as `5b3aa4ac`, `PopupWindow.is-open` as `22c4a775` with `2c0ea226` and
`a49c7309` on top, then `animate { enabled }`), and items 4–7 remain.

A trial on 2026-09-16 (throwaway worktree, since removed) ran
`cargo clippy --all-targets --workspace` on Linux against 1.18.0, and the diagnostics listed in
phase 1 were the only ones. On 1.18.1 against the real tree the gate came out the same, and with
the renames below it prints no Slint build-script warning at all. Windows-only code
(`window_chrome/parked_loop.rs`, the `tray-icon` path) isn't compiled on Linux.
`CustomApplicationHandler` is unchanged in the 1.18 source, but the `clippy-windows` job is the
real check.

### Phase 0: go/no-go on blocker 2

- [x] Measurement binaries, fresh release builds of HEAD (`47c49567`) and of the bumped tree:
      `~/Development/melodia-measure/target/slint-1.16.1/Melodia` and `…/slint-1.18.1/Melodia`.
- [x] Re-measure frame pacing on the queue sheet and the nav transitions on both, with
      `SLINT_DEBUG_PERFORMANCE=refresh_full_speed,console`: Slint prints
      `average frames per second: n` to stderr once a second in both versions, so the per-frame
      walk shows as a lower steady rate with the sheet mounted. Smooth by eye on both
      (2026-10-11), and the debug build under `cargo run` didn't stutter either. No rate was
      printed: the command broke across lines, so the variable never reached the binary.
- [ ] ~~Peak RSS via `/usr/bin/time -v` on both, since eager instantiation builds every mount's
      contents up front.~~ Not part of the go/no-go: measured separately, outside this plan.
- [ ] ~~**Still stutters:** file the upstream issue with the numbers, naming the per-frame walk in
      `draw_contents` and the per-repeater `Vec`, `git restore` the bump, and stay on 1.16.1.~~
      Not taken.
- [x] **Smooth:** keep the bump and carry on with phase 2.

### Phase 1: the bump

- [x] `slint` and `slint-build` to `1.18.1` in the root `[workspace.dependencies]`.
- [x] **The 1.18.0 lockfile trap is gone.** 1.18.0's generated code called
      `sp::compose_field_offsets`, which only exists from `const-field-offset` 0.2.1, while
      `slint` declared `0.2.0`, so a targeted update kept 0.2.0 and `melodia-ui` failed with
      7,876 `E0425`s. `slint` and `i-slint-core` 1.18.1 both declare `0.2.1`, so a plain
      `cargo update -p slint -p slint-build` takes it.
- [x] `window_chrome/winit_filter.rs:473`: `try_dispatch_event` → `dispatch_event_with_result`,
      ignoring the new `WindowEventDispatchResult` since only the error is logged. This was the one
      rustc warning in the workspace, and `-D warnings` fails the gate on it.
- [x] `viewport-x`/`-y`/`-width`/`-height` → `content-*` on `Flickable`, `ScrollView` and
      `ListView`: 123 sites plus 19 `.slint` comments, and the `now_playing/lyrics/follow.rs`
      module doc. The old names are deprecated aliases and build-script warnings don't trip
      `-D warnings`, but they buried everything else in the build output. `v-viewport-height`,
      `r-viewport-*`, `t-viewport-y` and the row's `list-viewport-y` are our own and stay.
      CLAUDE.md's scrollbar convention and the `slint-pitfalls.md` entries moved with the docs
      pass.
- [x] The blocker-1 fix in `crates/melodia-ui/ui/components/view-transition.slint`: `init` reads
      `opacity`, `x` and `y` once, so each starts from its `shown: false` value before the Timer
      flips it. The generated `app-window.rs` keeps the three `.get()`s in every instance's
      `user_init`.
- [x] `slint-build = { version = "1.18.1", default-features = false, features = ["compat-1-18"] }`.
      `renderer-software` became a default feature that only feeds software-renderer resource
      embedding, which `crates/melodia-ui/build.rs` doesn't use.
- [x] `crates/melodia-ui/build.rs` lost its `cargo:rerun-if-changed=translations`: slint-build 1.18
      prints the same directory itself (#13329).
- [x] `slint-tr-extractor` 1.18.1 and `scripts/update-translations.sh`: no catalog moved.
- [x] `.cargo/audit.toml`: `rustybuzz` left the lockfile with usvg 0.48, so its ignore went, it
      being one that would hide a future advisory on that crate. The bincode, ttf-parser and paste
      reasons now name the 1.18.1 chains, and an ignore-free audit fires exactly those three.
- [x] The in-tree "Slint 1.16 has no …" notes whose claim 1.18.1 still makes true now say 1.18
      (`closed`, `.contains()`, the Rust-callable `tr`, `Text`'s font axes, `DragArea.dragging`,
      cross-app drops). The ones phase 3 retires keep their tag until then. Two are left as they
      were: `theme.slint`'s easing note, unverified on 1.18, and `color-dot-grid.slint`'s "no
      transform on a Rectangle", which 1.16.1's own compiler doesn't bear out either.
- [x] The docs pass: "Slint 1.18" in CLAUDE.md (opening line, RTL note) and README's stack line,
      `content-y` in CLAUDE.md's scrollbar convention, and `slint-pitfalls.md` / `ui-patterns.md`
      retagged and re-checked against the 1.18.1 source. The touchpad entry and `route_wheel`'s doc
      comment now say 1.18.1 fixed the capture (#13453); the arm itself is phase 4.

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
- **1.18.1 over 1.18.0**, the reason for the target: a panic when a repeated element's handler
  removes its own row or closes its popup and then touches something outside it (#13528, our row
  context menus' remove entries); a min-height over a nested layout lost, making windows too short
  (#13523); nested `Flickable`s unscrollable by touchpad (#13453, the composite views); and a
  spurious "hide failed" warning on close (#13507, close-to-tray).
- **Dependency graph**, measured on the bumped lockfile against HEAD's: 867 → 836 packages, and
  names resolved at more than one version 77 → 62. `windows` 0.61 drops out, and the AVIF/EXR
  decoders (`rav1e`, `ravif`, `exr`) leave, since slint-build's extra image formats are opt-in
  now. Slint's `muda` moves 0.18 → 0.19.3 and stays a second copy beside tray-icon's 0.21.2, so
  `keyboard-types` stays at two versions with it. wgpu 30 and Skia appear in the lockfile only as
  Slint's optional renderers; `cargo tree` builds neither on Linux, Windows or macOS.

### Phase 2: manual verification

- [x] Frame pacing on the queue sheet (phase 0).
- [x] Enter transitions on every nav, tab switch and the return from the miniplayer.
- [x] Mouse-wheel scrolling now animates (1.17, fixed 180 ms, not configurable). Composite views
      stay stepped; see #127.
- [x] Word-wrapped `Text` now reports its longest word as its minimum width (1.18). Check the
      miniplayer and Settings rows at the minimum window width.
- [x] Percentage sizes no longer feed the parent's layout (1.17, #3346).
- [x] `clip: true` with a border now clips children at the border's inner edge (1.18, #1988). One
      site: `components/dialog/playlist-mosaic-picker.slint:214`. Expected to look better, since
      the scrolled mosaic stops painting over the 1 px edge.
- [x] Every popup: non-native popup clipping changed (1.17.1, #12324), and a popup's offset is
      recalculated when its size changes while shown (1.18).
- [x] Locale decimal separator: 1.17 routes float→string through `Platform.decimal-separator`.
      The one fractional readout, `views/settings/playback-section.slint:144` (crossfade
      seconds), renders `2,5 s` in de/fr/es/tr/el/it. Arguably correct; check the `.po` strings
      still read naturally.
- [x] Default font size is read from system settings on Windows/Linux (1.17). We pin
      `default-font-size` at `app-window.slint:134`, so this should be inert. Visual-check at a
      non-default system font scale anyway, since the patched Vazirmatn metrics assume our size.
- [x] The Wayland sizing fixes against the miniplayer's restore paths, and the macOS title-bar fix
      against the native-titlebar miniplayer.
- [x] Keyboard shortcuts after a field that stays mounted but goes hidden held focus: 1.18 clears
      focus off an invisible item (#11079), and only a view change runs the mirrors that hand
      it back to `ShortcutScope`.
- [x] Remove a row through its own context menu in the queue, a playlist and Favorites (#13528).
- [x] Touchpad scrolling in the composite views (#13453).
- [x] Close to tray and back, with the log open for a hide warning (#13507).
- [x] The `ListView`s that set an explicit `content-width`/`-height`: 1.18 stops overwriting it
      (#5485), e.g. `horizontal-card-strip.slint` and Artist Detail's album strip.
- [x] The miniplayer backdrop's fade-in, the other fade a 1 ms Timer flips right after its mount.
      Missed by the first pass and found in review: it snapped on some presses, blocker 1 one
      `if` deeper, its layers first evaluated at paint while the Timer could fire before it.
      `HeroBlurBackdrop` and `AuroraBackdrop` now read their layers in `init`, which is enough
      even inside `NpBackdropStack`'s arm: `Conditional::ensure_updated` runs a new branch's `init`
      and builds its nested `if`s in one call, before the event loop can fire the Timer. A Timer
      waiting on the arm's mount went in with the first fix and came out for that reason. Pinned
      with `ViewTransition` by `ui::mount_fade_tests`.
- [x] Re-check the miniplayer backdrop over a few presses on the reads alone, the first re-check
      having run with the arm wait still in place. Fades on every press.

### Phase 3: adoptions

One change per section, in this order, each on a tree already on 1.18:

1. 🟢 `FlexboxLayout` → retire the row packer and the `Wrap` global (largest deletion). **Done
   2026-10-12.**
2. 🟢 `PopupWindow.is-open` → slim the `PopupHighlight` plumbing. **Done 2026-10-12.**
3. 🟢 `animate { enabled }` → replace duration-zero gating. **Done 2026-10-12.**
4. 🟢 `cross-axis-alignment` → drop centering wrapper layouts
5. 🟢 `WindowMoveArea` → retire the drag-region press intercept
6. 🟢 Two-way model row bindings → slim the model-patch walkers (prototype first)
7. 🟡 Hangul measuring workaround → retire if 1.18's text stack composes the syllables

Legend: 🟢 adoptable once on 1.18 · 🟡 shipped upstream but wait/verify first.

### Phase 4: retire the touchpad re-dispatch

Small and separate from phase 3's adoptions: it deletes a workaround for a bug 1.18.1 fixed, and on
1.18 the workaround is what makes touchpad scrolling worse.

- **Why it can go.** Through 1.18.0 a `Flickable` intercepted `TouchPhase::Started` whatever the
  delta's axis (`i-slint-core-1.18.0/items/flickable.rs:762`), so the outermost one owned the whole
  gesture and a card strip under Search's vertical scroller never saw a sideways swipe. 1.18.1
  intercepts only a gesture it already holds or one on an axis it can scroll (#13453,
  `items/flickable.rs:765-782`), and `process_wheel_event` hands a crossing-axis gesture back
  (`:469-477`).
- **Why it should go.** `route_wheel` swallows the native `Started` and re-sends its delta as a
  `PointerScrolled`, which Slint delivers as a one-shot `Cancelled` (`api.rs:702-706`). Slint never
  sees the gesture start, so nothing captures it, and every touchpad `Moved` takes the mouse-wheel
  arm: animated over 180 ms since 1.17 (`WHEEL_SCROLL_DURATION`), so the content trails the
  fingers, with no kinetic fling on release.
- **The fix is the deletion.** Not a phase-aware re-dispatch and not a tuned animation: upstream's
  own path now does what the arm was for, with direct tracking and the fling.

- [x] `window_chrome/winit_filter.rs`: delete `WheelRoute::Unphased`, the `Started` branch in
      `route_wheel` and the `PointerScrolled` arm in `on_mouse_wheel`, then the parameters nothing
      reads any more (`route_wheel`'s `phase`, `on_mouse_wheel`'s `cursor_pos` and `phase`). The
      `CursorMoved` mirror stays, since the titlebar's double press reads it. Trim `route_wheel`'s
      doc and the module doc's item 6 to the one defect left, the composite one.
- [x] `window_chrome/tests/winit_filter_tests.rs`: drop
      `a_touchpad_gesture_start_is_unphased_on_both_axes`, its sideways case folding into
      `a_wheel_is_left_to_slint`. The rest lose their `TouchPhase` argument with `route_wheel`'s
      `phase`, and the overlay test expects `Native` for both its cases.
- [x] `.claude/rules/slint-pitfalls.md`: delete the touchpad entry, its bug fixed and its cure gone.
- [x] Gate: `cargo fmt --all --check`, clippy, `cargo test --locked --workspace` (2026-10-12).
- [x] Manual, on a touchpad under Wayland (2026-10-12). X11 and Win32 send no phase, so a mouse
      and those two see no change; macOS does send one, and is the other platform this reaches.
      - A sideways two-finger swipe moves a Search card strip.
      - A vertical swipe over that strip still scrolls the page.
      - A plain list follows the fingers and flings on release.
      - Browse and Artist Detail still scroll vertically, the composite route being untouched.
      - Now Playing or the queue sheet opened over Browse scrolls its own list, the overlay arm
        now going native.

### Decision: how the composite views scroll

Browse and Artist Detail scroll through `CompositeScroll`, which writes each delta straight to the
offset. Everywhere else Slint scrolls, and since 1.17 it eases a wheel notch over 180 ms. Phase 4
carries the gap over to the touchpad:

|                       | mouse wheel             | touchpad                                   |
|-----------------------|-------------------------|--------------------------------------------|
| plain lists           | glides over 180 ms      | follows the fingers, flings on release     |
| Browse, Artist Detail | jumps a notch at a time | follows the fingers, stops dead on release |

- [x] Decided on phase 4's manual pass (2026-10-12): **accept the difference.** The composite path
      is a workaround for the edge-swallow defect, which #127 watches to shrink, and matching
      Slint grows it. A port of `Flickable`'s fling into that path was tried the same day, made no
      visible difference on the manual pass, and was dropped. Easing a notch over the same 180 ms
      in `composite-scrollbars.slint` was never tried.

---

## 🟢 `FlexboxLayout` → retire the Rust row packer and the `Wrap` global

Done 2026-10-12, every step passed by hand.

- **The four wrapping surfaces** (the Now Playing and hero chip strips, every `ChipGroup` and
  `ColorDotGrid`, Recent Searches) are `FlexboxLayout`s over flat models, breaking lines on the
  chips' real widths. `ui/chips.rs`, `globals/wrap.slint`, the strip's width channel and the
  hidden ruler are gone. A hero's two-row cap is a clip on `MetaChipStrip` (`max-rows`), argued at
  `HeroChipStrip`'s binding.
- **Two more wraps went the same way**: Radio Browse's suggestion pills, which lost their
  four-pill cap, and the playlist mosaic picker's tiles, which lost their hand-placed grid.
- **The Settings body was tried as a flex of two card columns and reverted.** A flex measures its
  items at a trial width, which only a layout-rooted component can answer, and the settings cards
  are full of `Rectangle`-rooted ones whose height follows their width: it built clean and
  panicked `Recursion detected` on opening Interface. `.claude/rules/slint-pitfalls.md`'s flex
  measuring entry carries the mechanism.
- **Not a fit, checked**: the virtualized card grids (a flex doesn't virtualize), the station
  cards' tag line (a fixed card height inside a `ListView` row), the lyrics rows (word wrap plus a
  Rust-owned offset table), the track columns (per-column widths), the miniplayer (a continuous
  morph) and the band pill rows (a fixed band row height).
- **Possible now and not done**: the band's chip strip joining `hero-shown`, a `MetaChipStrip` in
  the welcome card, Search's Top Result and Songs stacking on a narrow panel, the Browse
  breadcrumb wrapping, and `SettingRow` dropping its control under the label.

## 🟢 `PopupWindow` geometry reactivity + `is-open` → simplify popup plumbing

Done 2026-10-12, every step passed by hand.

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
- **Outcome:** (b) went whole: every trigger sat beside its popup, so `PopupHighlight` and the
  winit Release arm are gone. (a) was tried everywhere and kept only where the origin holds still:
  an open popup re-lays its content at once and moves a loop pass later, so a box growing left or
  up draws one frame out of place. The overflow menu keeps its reserve; a right-click menu grows
  rightward and holds its room up front leftward. `slint-pitfalls.md` carries the mechanism.
- **Follow-up, the right-click menus:** live geometry also made a menu anchored to the row's live
  `mouse-x` follow a held right button, and an outside press was always swallowed, so a second
  right-click only closed the menu. Every menu now opens on the release through
  `ContextClick.raises-menu` and takes its anchor through `show-at`; `PopupDismissCatcher` closes
  on a right press, so the release reaches what lies beneath. Pinned by
  `crates/melodia/tests/context_click.rs`.
- [x] Manual: the bar overflow's Speed, Sleep and the switch between them; Copy on a right-click
      menu opening each way; each trigger lit while open and dark after Escape, an outside click and
      a focus loss.
- [x] Manual: right-click a song, then another near and far from the menu, moving it in one click;
      a held right button opens nothing until released and the menu stays put; a press dragged off
      its row raises nothing; the same on a card, a text field and the queue.
- [x] Manual: a right-click on a flyout menu's padding or a separator moves the menu to the row
      beneath; the radio facet chip's search field raises an edit menu that stays open.

## 🟢 `animate { enabled: … }` → replace duration-zero gating

Done 2026-10-12, every step passed by hand.

- **Today:** the "don't animate drag micro-updates" pitfall is handled with
  `duration: is-dragging ? 0ms : …`, at `layout/sidebar.slint:71`, `:132` and `:187` (rail width
  and row padding).
- **Upstream in 1.17:** `animate` blocks take an `enabled` boolean (default true).
- **Migration:** swap to `enabled: !is-dragging` where the intent is "no animation", keeping the
  duration token intact. Cleaner semantics, same behavior.
- **Risk:** trivial; verify the disabled path snaps to target (upstream implements it as
  jump-to-target).
- **Outcome:** the three sidebar sites, plus one the list missed: `tab-bar.slint`'s `compact-t`,
  gated on `eased`. A disabled animation returns its target at once
  (`i-slint-core-1.18.1/properties/properties_animations.rs:171`), imperative writes included.
  `Sidebar`'s local `is-dragging` went too, a copy of `Nav.sidebar-dragging` the handle wrote
  beside it, and so did the slider thumb's `animate x { duration: 0ms; }`, which never eased. No
  `animate` in the tree spells a zero duration now.

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

---

*2026-09-17: the 1.18.0 migration plan replaced the re-migration checklist. 2026-10-11: retargeted
to 1.18.1, phase 1 applied in the tree, phase 4 added. 2026-10-12: phase 4 landed and passed by
hand, and every section waiting on an upstream Slint change moved to #127. Later that day phase
3's items 1 (`FlexboxLayout`, its Settings-body step reverted), 2 (`PopupWindow.is-open`, the
right-click menus reworked on top) and 3 (`animate { enabled }`) landed and passed by hand.*
