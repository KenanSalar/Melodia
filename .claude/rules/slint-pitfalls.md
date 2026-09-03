---
paths:
  - melodia-ui/ui/**/*.slint
  - src/ui/**/*.rs
  - src/boot/**/*.rs
  - crates/melodia-core/src/themes/**/*.rs
  - melodia-ui/build.rs
---

# Slint Pitfalls (battle-tested)

Melodia-specific, each paid for once already. General Slint patterns are in `slint.md`;
this file is what builds, looks right, and is wrong.

- **`visible: false` doesn't remove from layout.** Hidden child still claims stretch.
  Fix: `if !collapsed: VerticalLayout { … }`. Ref: slint#7377.

- **Don't `animate` a property driven by both toggle and continuous input** (drag micro-updates
  get full easing → spongy). Gate duration on bool:
  `animate width { duration: is-dragging ? 0ms : 250ms; }`. Boolean ternaries safe; #7999 only
  fires on array/list calcs.

- **An animated property with a *binding* restarts whenever a dependency is marked dirty — not
  when its value changes.** `AnimatedBindingCallable::mark_dirty`
  (`i-slint-core/properties/properties_animations.rs`) checks `original_binding.dirty` and
  nothing else, resets the start time, and the next `evaluate` re-bases `from_value` on the
  current frame. Dirty propagation is structural, so a bool that *stays* `true` through a resize
  drag restarts the ease every time the width feeding it moves. Symptom: convergence tracks the
  rate of input events rather than wall-clock — crawls under a slow drag, perfect under a fast
  one, reading as an easing-curve problem.
  Fix: **write** the property rather than bind it — an imperative assignment compiles to
  `Property::set_animated_value`, replacing the animated binding, so only a real write restarts
  it. Keep the original expression as the declared *seed* so the first evaluation lands in
  `NotAnimating` and mount doesn't animate (`tab-bar.slint`'s `compact-t`).
  **`states`/`transitions` only half-fix it**: `StateInfoBinding::evaluate` *is* value-compared,
  so `change_time` survives the dirt and the timeline stays anchored, but `from_value` is still
  re-based every frame and the curve collapses to the target early. **That half is the whole cure
  when the dirt is *discrete***: a late one-shot target (an async result mid-animation) restarts a
  plain `animate` and finishes a full duration after it, where a transition ends at
  `change_time + duration` and adopts it in one frame. Hence `library-tab-band.slint`, whose
  tab-bar brushes cross to `HeroBackdrop` tiers solved from the artwork decode — it eases the four
  mirrors it crosses *to* on a short curve of their own and lets the transition follow.

- **A shared component may not `animate` a brush its host hands it — it cannot tell an eased
  input from a stepped one, so it eases a *float* and lets the brush track its source.** A host
  crossing its palette over 400 ms re-dirties the leaf's binding every frame, so the leaf's
  `animate` restarts every frame, sits still until the source settles, then catches up in one late
  rush. Symptom: a colour that "changes at the end of the animation" in **both** directions.
  `touch.has-hover ? hover-fill : transparent` is **not** an escape — the tab you point at while
  clicking *is* the hovered arm. Cure: ease `hover-t: touch.has-hover ? 1.0 : 0.0`, a float no
  host can dirty, and paint `root.hover-fill.transparentize(1.0 - root.hover-t)`;
  `tab-bar.slint`'s `TabBarCell` does this for hover and for `sel-t`. Two primitives matter.
  **`transparentize` multiplies alpha where `with-alpha` sets it** and takes a `brush`, so a
  translucent tier keeps its weight at full hover and two fades compose. And **`mix()` has no
  brush overload** (`ColorMix: (Color, Color, Float32) -> Color`), so blending two host brushes
  means *two stacked layers* — **bottom at full alpha**, only the top riding the float, since
  layers at `t` and `1 - t` composite to three quarters coverage at the midpoint and the text
  thins; keep them identical in everything but colour or the crossing ghosts. The exemption is the
  opposite case — a host that never eases what it hands over, hence `icon-button.slint`'s two
  `animate`s. Pinned by `ui::tab_bar::tests::`
  `{the_cell_eases_floats_and_never_a_brush, the_selected_colour_crosses_over_two_matched_layers}`.
  **The mirror image bites the host**: a layer may not ease *out of* a value nothing was painting.
  `HeroBackdrop` is held across a My Library tab leave, so a mirror bound to it unconditionally
  settles on a hero the band stopped painting and crosses out of it on the next open — a genre's
  pink under a playlist. Cure at the source, not the animation: make the idle value honest
  (`root.detail-open ? <tier> : <idle token>`), suppressing the ease on the right frame being
  impossible when the colour and the id land in the same tick. See `ui-patterns.md`.

- **Drag handles inside resized element need absolute coords.** Snapshot
  `start_abs = self.absolute-position.x + self.pressed-x` on `down`, then
  `parent.width = clamp(start_w + (self.absolute-position.x + self.mouse-x - start_abs), ...)`.

- **Material Symbols glyphs need a collapsed line-box.** `Text` defaults to ~1.2× `font-size`;
  pin inside fixed `icon-size × icon-size` Rectangle. `MaterialIcon` does this.

- **Fixed-width children don't center in a wider `VerticalLayout`.** Let column track child's
  natural width, or wrap in `HorizontalLayout { alignment: center; … }`.

- **`parent` not accessible from component's root binding.** Take host metric as explicit
  `in property <length> host-width;`.

- **An element passed into a `@children` slot is *drawn* where the slot is and *resolves names*
  where it was written, so `parent` there is the mount, not the slot's container.** A host writing
  `Card { btn := IconButton { y: parent.height - self.height - 8px; } }` reads as positioning
  against whatever `Card` wraps its `@children` in; it compiles to the **`Card` element's** own
  height. The offsets are still applied inside the container, so the control lands at the
  container's origin plus a distance measured off something else — `EntityCard`'s overlay is its
  artwork square and its mount is a taller card, so four station controls compiled to
  `card-height - …` and drew `card-height - tile-size` low, over the text block, with no term in the
  source that looks wrong. Both dimensions come off `GridGeometry`, so the miss is a relation rather
  than a number and there is no window width at which it reads as a rounding error.
  Nothing warns, and both readings are plausible enough that review settles on the wrong one.
  Cure: **the container publishes its own metric and the host frames against that**
  (`out property <length> tile-size` on `EntityCard`, `card-body.tile-size - self.width - …`),
  never `parent`. Verify in the generated tree — the binding names the property it actually read.
  Pinned by `ui::entity_card_tests::no_overlay_host_positions_against_parent`, which finds the
  hosts by the flag that opens the slot and reads **the mount's own braces**: a host is free to
  spell `parent` elsewhere in its file, `x: parent.width - self.width` being the canonical
  `OverlayScrollbar` mount, so a file-wide ban would fail the first overlay host carrying a bar.

- **`height: 100%` on child + `height: Npx` on parent → unbounded layout.** Row swallows whole
  body; sibling `ListView` renders 0 rows. Pin fixed-size rows with `min-height` + `max-height` +
  `vertical-stretch: 0`. Never `height/width: 100%` on layout child.

- **Nested ScrollView + ListView need `viewport-height: self.height` on outer.** Reverse: lock
  `viewport-width: self.width`.

- **A nested `ListView` swallows the mouse wheel at its scroll edge — it never bubbles to an outer
  `ScrollView`** (`flickable.rs::process_wheel_event` returns `EventAccepted` at the edge during
  the scroll-capture window), so an outer scroller wrapping a *height-capped* (still virtualizing)
  `TrackList` can't be wheeled once the inner list has travel, and no pure-Slint interceptor
  works. The **composite views** route vertical-dominant wheel through winit instead:
  `winit_filter.rs`'s `MouseWheel` arm reads `CompositeScroll.hovered` (a per-view ancestor
  `hover-catch` TouchArea), converts the delta like the Slint backend (`LineDelta*60` /
  `PixelDelta.to_logical`), and drives the split via `CompositeScroll.wheel-{dy,tick}`. The Slint
  half lives once in `components/composite-scrollbars.slint`, mounted as the last root child
  (`x/y: 0`, `100%×100%`; contract in its header).
  **That arm also owes `ui.window().request_redraw()`** — `run_change_handlers` is reached only
  from `new_events`, and what schedules that frame is `WindowRedrawTracker`, over the properties
  the **render** pass read. Nothing paints `wheel-{dy,tick}`, so the loop slept on each delta
  until the next notch woke it: every notch one late (#64). **A Rust write watched only by a
  `changed` handler owes the frame; one that also moves something rendered gets it free.** Pinned
  by `winit_filter::tests::the_composite_wheel_arm_asks_for_a_frame`, a source walk.
  **Every content-view switch owes a `CompositeScroll.reset()`** — a `public function` (a
  callback's single handler slot must not be clobberable, as with `Dialog.closed-teardown()`)
  clearing `hovered` *and* any un-applied `wheel-dy`. Called from five `changed` handlers on the
  always-mounted `AppWindow` root — `watched-nav-idx`, `watched-my-library-tab`,
  `watched-now-playing-open`, `watched-artist-detail-id`, `watched-mini-render` — since Slint
  destroys the outgoing view instantly and gives no unmount hook. Miss one and the filter keeps
  eating vertical wheel on the *new* view until the next mouse move; a new always-mounted mirror
  unmounting a composite view needs a sixth. (Only composite views owe it; the focus-regrab mirror
  set in `ui-patterns.md` is wider.) Horizontal-dominant wheel stays native.
  **Reach for tabs before the composite** when a page wants a list under a band of cards — one
  scroller per tab with plain `OverlayScrollbar`s is the way out. The two mounts left are where
  the upper section is a header the list belongs under, not a peer view: `browse-view.slint` and
  `ArtistDetailBody`.

- **A `Flickable` claims a whole *gesture* on `TouchPhase::Started`, whatever direction it is
  going — so a touchpad scrolls where a wheel does not.** `flickable.rs`'s filter returns
  `Intercept` for that phase unconditionally and `process_wheel_event` sets `capture_events` with
  no `is_allowed_scroll_direction` call, so the *outermost* `Flickable` under the pointer owns
  every event until `Ended` — its `Moved`/`Cancelled` arms do check direction, which is the whole
  of why a wheel behaves. **Only a precision device sends the phase**: Wayland folds a discrete
  axis to `Moved`, X11 and Win32 send nothing else — Wayland and macOS only, invisible to a mouse.
  Bit every page whose body is a plain `TrackList`, which wraps its vertical `ListView` in a
  horizontal-only `ScrollView` for the column pan. Cure in `winit_filter.rs`'s `route_wheel`:
  swallow the native `Started` and re-send its delta through
  `Window::try_dispatch_event(PointerScrolled)`, which lands as a one-shot `Cancelled` and leaves
  the capture flag unset for the rest of the gesture. **Ungated on purpose** — it reads no view
  state, so a new nested scroller is covered without owing a sentinel or a `reset()`. It costs
  Slint's kinetic fling, gated on that same flag. `MouseWheel` carries no position, hence the
  `CursorMoved` mirror beside it.

- **The same `Flickable` steals a *mouse* drag wherever it is interactive, and there the cure is
  per-list: `mouse-drag-pan-enabled`.** A row drag and a drag-pan are one gesture and only one
  element can own it. **Which one wins is the *style's* answer rather than Slint's**, and the
  property is the styled widget's, not the item's: `ScrollView` publishes
  `mouse-drag-pan-enabled <=> flickable.interactive`, and while the bare `Flickable.interactive`
  does default **`true`**, fluent, cupertino, cosmic and qt each pin `interactive: false` inside
  their own `ScrollView`. Material is the one that doesn't, which is what the Slint docs mean by
  "defaults to `true` for the Material style and `false` for all other styles".
  `ListView inherits ScrollView`, so it takes the same answer, and the tree declares no bare
  `Flickable` at all. Melodia sets no style and `i-slint-compiler`'s `typeloader.rs` resolves it to
  **fluent**, so the live default here is *off* and every scroller binding nothing already
  compiles to `false`. What the opt-outs buy is therefore a pin rather than an override: no
  click-to-act list depends on which style compiled. **Verify in the generated file, never in the
  `.slint`** — the value is inlined per instance, so a source binding and an inherited default are
  the same line.
  Where the pan *is* on, a press inside one returns `DelayForwarding`, and `handle_mouse_grab`
  (`input.rs`) re-consults exactly those ancestors *during* the inner grab; past
  `DISTANCE_THRESHOLD` (8 px) inside `DURATION_THRESHOLD` (500 ms) on a scrollable axis it returns
  `Intercept`, and every item below is sent `MouseEvent::Exit`, delivered by `TouchArea` as
  `PointerEventKind::Cancel`. The row's drag state is wiped and the committing pointer-up never
  arrives. **The row arms at 4 px, i.e. inside that window**, so the failure is total rather than
  occasional, and at 4–8 px of travel the computed slot is still the source's own, so no drop
  indicator paints either. It still reads as intermittent, both escapes being real: a list shorter
  than its viewport can't flick, and a press held past 500 ms before moving is never intercepted.
  Both draggable lists opt out outright, `draggable-track-list.slint` on both axes (a diagonal
  drag steals sideways once the columns overflow) and `queue-sheet.slint`, pinned by
  `ui::playlists::tests::every_draggable_list_opts_out_of_drag_panning`, since it only misbehaves
  under a pointer; the click-to-act grids and lists are pinned by `ui::scrollbar_tests`.
  **`!reorder-enabled` is what that binding used to say, and is the trap worth keeping**: it reads
  as leaving a `true` default alone and instead *enables* the pan on every sort that retires the
  drag. `!interactive` still forwards wheel events, so only drag-to-pan goes.

- **Animating a binding derived from another animating property phase-lags.** Animate source only.

- **Concurrent `animate` blocks aren't free at vsync** — re-evaluated per frame. For *periodic*
  visuals prefer one shared `Timer` + counter + math-derived bindings.

- **An animated `width` on a *component root* eases the window's own minimum width.** Slint
  reports a root's bound `width` as both `min` *and* `max` in its `layout_info` (and rejects a
  `min-width` beside it), so the layout floor tracks the animation and propagates up to the
  window: dragging an edge inward against a still-easing floor **stutters**, and the element
  becomes the constraint that stops the window reaching widths its own responsive threshold waits
  for, so a measured breakpoint fires late or never. Fix: drop the bound `width` and spell out
  `min-width` (constant floor) / `preferred-width` (the animated value — a centring or
  non-stretching host draws it identically) / `max-width`. Tell: `layout_info`'s `min` reading
  back the root's own `width`.

- **The same is true of an animated `height`, and there the split obliges a `clip`.**
  `library-tab-band.slint` morphs a ~132 px idle band to a 232 px hero via `min-height` /
  `preferred-height: compact-h + (hero-h - compact-h) * hero-t` / `max-height`. Consequence of
  that freedom: the element is drawn shorter than it asked and the hero's contents paint down out
  of the band into the page body on every frame of the shrink leg (on the width axis they spill
  sideways under a neighbour instead). `clip: true` on the root contains it — rectangular and
  borderless, so it lowers to a scissor rather than the offscreen layer a rounded clip over text
  would cost. Pinned by `ui::library_tab_band_tests`.

- **Components writing own `in-out property` orphan one-way `name: source` binding on first
  click.** `clicked => { root.selected-index = i; }` detaches `selected-index: SomeGlobal.field`.
  Fix: two-way `<=>`. `ToggleSwitch`: `manual: true` emits `toggled(new-value)` *without* mutating
  own `checked`. `Dropdown`: `manual: true` fires `selected(i)` *without* self-writing
  `selected-index`, so a one-way `selected-index: model.field` binding survives — the
  smart-playlist rule rows need that, a field change resetting the operator index the op dropdown
  must re-read.

- **Rectangle-inheriting components don't size from `if`-conditional children — wrap in a
  layout.** `Rectangle { if has-matches: SectionCard { … } }` reports 0×0; fix
  `VerticalLayout { … }`. An `if` lowers to a repeater and
  `default_geometry::gen_layout_info_prop` folds a child *layout* into the root's layout info but
  skips a repeated one (tell: the missing `+ …layoutinfo_v` term in `app-window.rs`).
  **It also skips any child that binds `x` or `y`, and that half is the inverse hazard.** A
  hand-positioned child contributes nothing, where the same content in a centring layout folds its
  whole constraint set — including whatever is *animated* inside it — into the root's
  `layout_info` and so into the host layout's dependency graph. `IconButton` centres its glyph by
  arithmetic rather than a `HorizontalLayout` precisely so a press-shrink doesn't re-solve the
  now-playing bar per frame; nothing visible moves when it does, so the only tell is that
  `+ …layoutinfo_h` term reaching an animated property.
  **A `min-height` beside it makes this survivable and therefore invisible**: `MetaChipStrip`
  shipped as `Rectangle { min-height: 26px; if show-rows: VerticalLayout { … } }`, reported
  `preferred: 0`, took its 26 px floor, and drew every wrapped row *outside* the box — only wrong
  once content wraps, i.e. a narrow window or a long-plural locale. **And the wrapper owes a
  `min-width: 0px`** — a layout child folds in *both* orientations, and `layout_items` never
  shrinks a cell past its `min`, so a strip that chunks itself against the width it is handed
  reports the old width back, never learns there is less room, and clips instead of wrapping. An
  explicit `min-width` **replaces** the merged constraint rather than maxing with it, so one line
  is the whole cure (`settings/chip-group.slint` carries it against a hidden ruler). Nothing
  higher up saves you: the same replacement happens on the root `Window`. Pinned by
  `ui::hero_chips::tests::{the_strip_rows_hang_off_a_layout_not_off_the_root,`
  `the_strip_leaks_no_width_floor}`.

- **`VerticalLayout` divides surplus equally when every child has `vertical-stretch: 0`.** Append
  trailing `Rectangle { vertical-stretch: 1; }`, or set `alignment: start`, which hands out no
  surplus at all. Inside a `GridLayout` one of the two is mandatory: a row is as tall as its
  taller cell and the shorter is stretched to match, so surplus lands inside whatever that cell
  contains — a settings body column left on `stretch` grows its cards instead of ending above the
  taller column's floor.

- **A `GridLayout` cannot do masonry, and the cure is to make each cell a column rather than an
  item.** Cells are top-aligned to a row as tall as its tallest, so a short item is followed by
  dead space down to the next row. No per-column flow, no `Flow` element to fall back on.
  Restructure: one cell per column, each a `VerticalLayout` of the items
  (`views/settings/pages/*.slint` place *columns*, not cards). Two consequences: stacked, it is
  column A's items then column B's, so columns want to be **contiguous halves** of the list rather
  than alternating; and a page with fewer items than columns leaves one empty, so an item that
  must not fill the surplus width needs its own `max-width` (`SectionCard` pins one beside its
  `preferred-width`).

- **`GridLayout` honours a runtime expression for `row`/`col` on a plain child, and forces `Auto`
  on a repeated one.** `convert_row_col_expr` lowers `RowColExpr::Named` to a `PropertyReference`
  read per layout pass, so `row: SomeGlobal.grid-row(1)` re-flows the grid reactively — how the
  Settings body swings its columns from side-by-side to stacked with every card still **mounted
  exactly once**. Two constraints. (1) `passes/lower_layout.rs`'s `add_element` forces
  `col_expr`/`row_expr` to `Auto` when `repeated.is_some()`, so a `for`-loop's items can't place
  themselves; only `colspan`/`rowspan` survive. (2) `check_numbering_consistency` rejects
  **mixing** auto-numbering with runtime expressions for the same property — once one child spells
  `row`, every child must (a literal is fine, an omission is not). Mount-once matters: a component
  reads its children's `out` properties back (a settings page ORs its sections' `has-matches`),
  and an element inside an `if` can't be read from outside it — so one branch per column count is
  unusable.

- **A `GridLayout` column starts at its cells' *preferred* width, so cells of different natural
  width give columns of different width.** `layout_items` sets `it.size = it.pref` for every
  column and only then distributes surplus by stretch, so equal `horizontal-stretch` does **not**
  equalise columns — it adds an equal share to unequal starting points, and two cards side by side
  read as a misalignment. Fix: pin every cell's `preferred-width` to the same number
  (`SectionCard` takes `SettingsPage.card-w`), leaving `min-width` alone so the cell still
  compresses. **Not `width`** — that is reported as both `min` and `max` and climbs out to the
  window's own floor.

- **A view root's bottom padding sits *outside* its scroller's viewport, so it reads as a dead
  strip rather than as breathing room.** `padding: Theme.pad-lg` on a root `VerticalLayout` whose
  stretchy child is a list/grid/ScrollView pads all four sides, and the bottom one shortens the
  viewport instead of the content: the last row is clipped mid-glyph and a band of bare
  `Theme.base` sits between it and the panel border at every scroll position. Easy to mistake for
  the horizontal `OverlayScrollbar` at the same `y` — the tell is colour: that track is `surface0`
  at half alpha, rounded and inset, where the strip is flat full-bleed `base`. Inset
  **left/right/top only**; for clearance at the end, put `padding-bottom` on the column *inside*
  the viewport — which is exactly what `reserve-scrollbar-lane` does for the horizontal bar's own
  slot, so the two read alike on screen and are opposite in the tree. Artist and Playlist can't inset on the root at all — Artist's `below-hero` must
  run full-bleed for `CompositeScrollbars` and the hover sentinel, and Playlist's empty state and
  drop banner deliberately fill `body`.

- **`changed` doesn't accept path expressions on globals — mirror via local property.**
  `changed Nav.selected-index => {}` fails to parse. Use
  `property <int> watched-nav-idx: Nav.selected-index; changed watched-nav-idx => {}`.

- **A faked placeholder `Text` in a non-layout parent paints out of its input, and
  `overflow: elide` alone doesn't stop it.** Slint's raw `TextInput` has no `placeholder-text`, so
  all four placeholders in the tree are a sibling `Text` gated on the field being empty. Under a
  plain `Rectangle`, `make_default_implicit` sizes each to `max(preferred, min)` of its **own**
  `layout_info` — the untruncated string — so it paints through the pill's right edge, and the
  default `clip` is against that same full-string width. Elide is half the cure: `text_layout_info`
  lowers only `min` to one `…`, leaving `preferred` at the full string, so a non-layout parent
  hands over the larger and elide never fires. **Bound the width *and* elide.** The `TextInput`
  beside each one never had the bug: it fills its parent (an item with **no** implicit size is
  sized to 100 %) and clips itself besides. `multiline-input.slint` is exempt — bounded to its
  scroller (`width: sv.width`) and wrapping rather than eliding. A **locale** bug in practice,
  English fitting every slot where the catalogues run ~1.3× longer; pinned by
  `ui::placeholder_tests`.

- **A `SearchBar`'s slot is sized off its own placeholder, so its root spells
  `min-width`/`preferred-width`/`max-width` and never binds `width`** — a bound `width` is
  reported as both `min` and `max` and would drag the window's resize floor along with a
  locale-sized default. The natural width is `placeholder-text.preferred-width + chrome-overhead`,
  read off the Text the bar draws (the `label-w` idiom: a Text's *horizontal* layout info is
  intrinsic to the string and never reads back the width it was handed, so it's a derivation and
  not a cycle). Only the **default** is measured — the four hosts that set `input-width` outright
  never evaluate it. What every mount keeps is the `min-width` floor: below it the bar compresses
  and the placeholder elides. That floor is **published as `out property min-w`**, the
  `TabBar.compact-w` contract — a host budgeting its row reserves against it rather than restating
  the number. `ui::placeholder_tests` pins both ends.

- **`has-hover` is not continuous, so a *translucent* fill may not read it raw.** Slint clears the
  flag from two places the pointer never left: `input_items.rs` drops it on a wheel event the
  `TouchArea` doesn't handle (restored on the next mouse move), so any hover surface **over a
  scroller** blinks on every wheel tick; and `send_exit_events` compares an item's **stack index**
  rather than its membership, so an item still under the pointer is handed a spurious `Exit`
  whenever the hit path's depth shifts around it. Both last one frame, which is why an *opaque*
  fill gets away with it — the blink lands mid-`animate` between two solid colours. A
  translucent one shows what is behind the element straight through, and hides until the backdrop
  changes: `search-bar.slint`'s `surface0` at .75 over flat `Theme.base` is the same pixel as 1.0,
  until the bar floats over its own results. Fix: **arm on the raw flag and release on a latch** —
  `engaged: pointer-inside || hover-held || has-focus`, a `changed` handler setting `hover-held`
  on arrival and a short `Timer` clearing it once the pointer has stayed gone. That is the sidebar
  rail tooltip's `held` shape, including why `hover-held` sits *beside* the raw flag rather than
  replacing it: `changed` doesn't fire on a first evaluation, so a component mounted under the
  pointer would never engage. Opaque fills are fine — don't retrofit the latch where there is
  nothing to see through.

- **Reusable filter SearchBar pattern.** (1) `text <=> SomeGlobal.filter`,
  `blur-trigger: SomeGlobal.blur-search-tick`; (2) backdrop
  `TouchArea { clicked => { SomeGlobal.blur-search-tick += 1; } }` at view root before content;
  (3) clear filter + bump blur tick on nav-away. Match in Rust through `src/ui/row_match.rs`,
  never a hand-rolled `to_lowercase().contains(…)`; Settings routes there via
  `pure callback SettingsPage.matches(...)`.
  **A page whose box describes more than one surface takes the same three steps against one global
  and dispatches in Rust.** My Library's band is the tree's only one: the bar binds
  `MyLibrary.filter` / `.blur-search-tick`, and `ui::my_library::filter::dispatch` routes a
  settled keystroke to whichever of nine surfaces is mounted — a *write*, since a `.slint` binding
  belongs to the scope it is written in, making `Tracks.filter: MyLibrary.filter` unspellable.
  Two things follow. A tab pick clears **both sides**, but the entering tab's
  own filter only when there *is* one: `filter::clear_mounted` guards on that needle being
  non-empty, since the pick runs ahead of the section gate and the surface's Rust cache is already
  wiped by its own leave — an unconditional rebuild writes `total-count = 0` plus an empty model,
  the exact pair `GridEmptyState` mounts on. And **anything that changes what the box *means* with
  nobody typing** — a detail id crossing zero, a tab move that isn't a pick — *reseats* it via
  `filter::sync_box`, taking the newly-mounted surface's own filter; a reseat, not a clear, since
  clearing on the way out would drop the grid filter the user is returning to. That is five
  `changed` mirrors, not four: `dispatch` clears only the *entering* tab's needle, so a cross-tab
  drill or a Mouse-4/5 walk otherwise lands on a tab filtered by an untouched one.

- **Filter debounce via `FilterThrottle`** (`components/filter-throttle.slint`). Non-visual (wraps
  a `Timer`); the host keeps one `property <int> filter-tick-pending`, bumps it in the SearchBar's
  `edited`, and mounts
  `FilterThrottle { pending: root.filter-tick-pending; fire() => { SomeGlobal.apply-filter(SomeGlobal.filter); } }`.
  Default `interval` 130 ms. The component owns the `applied` counter + `running` gate — don't
  hand-roll a `filter-tick-applied` property or inline `Timer`; every filterable view uses this.
  **`fire()` has a second shape, and Settings takes it**:
  `fire() => { SettingsPage.search-query = SettingsPage.search-input; }`, with the `SearchBar`
  bound to the *input* and the section cards reading the *query*. The shape follows where the cost
  lands — a list view debounces a model rebuild in Rust, where a Settings keystroke fans out to
  three `SettingsPage.matches` round-trips per row across all five tab pages, so the expensive
  work is Slint's own re-evaluation and the settled value must be a property it can read. Clear
  both on nav-away and on a tab pick; clearing only the settled one leaves the box holding text
  the page is no longer filtered by.

- **`PopupWindow.y: -self.height - …` needs explicit `width`/`height`.** Else `self.height` is 0
  before first layout — popup lands above trigger top, expands downward. Canonical:
  `components/now-playing/overflow-menu.slint`.

- **A flyout opens *inside* the overflow menu's single `PopupWindow` — no nesting.** The
  playback-speed row (`speed-flyout.slint`, presets in shared `flyout-presets.slint` globals) is
  the worked example. Fixed-reserve geometry, as with the volume popup: size the popup for
  menu-column + flyout up front and bottom-anchor both, so the menu stays pinned under its
  trigger; a transparent full-popup `TouchArea` **declared first** closes on stray clicks.

- **Sleep timer = a session-only cancel-and-replace tokio countdown + a `PlayerState`
  end-of-track flag.** In `src/ui/sleep_timer.rs` — a **UI-layer** module, because `tasks/` may
  not import `ui::*` and the countdown writes a Slint property. **Duration is
  playback-linked**: each 1 s tick decrements only while `status_atomic == Playing` (lock-free),
  so pausing holds the timer and it never expires on a paused player. Never persisted; bounds
  `[30 s, 2 h]`; `Player.set-sleep-timer(minutes)` takes 0 off / `>0` duration / `-1`
  End-of-track, the last arming `PlayerState::pause_after_current_track` (monitor half in
  `src/player/CLAUDE.md`).

- **Flash-free image cross-fade = two slots, never cleared.** Two stacked `Image`s + `use-a` bool;
  Rust writes the new image into the *inactive* slot then flips the bool so both `opacity`
  animate. Slot `source` is never reset — the outgoing layer stays painted for the fade. Clearing
  a pair is only ever right where nothing fades *into* the new source and no mounted element reads
  it; `ui::now_playing::source_change` is the one such site, and argues it there.

- **A rounded `clip: true` (or a `border`) on an element that *contains text/children* blurs +
  upscales that subtree on HiDPI.** FemtoVG renders it into an offscreen texture at logical size,
  then upscales by the display scale factor on blit. Don't wrap a scrolling text list in a
  bordered/rounded-clipped card: let the `ScrollView` clip its viewport rectangularly (cheap
  scissor, no layer) and paint the rounded border with a **childless overlay `Rectangle`**
  sibling. Canonical: `components/dialog/selectable-picker.slint::PickerListCard`.

- **Nothing that draws text may be cut to its layout box — the box is a line box and the ink is
  not. Two mechanisms cut, and both are invisible in Latin.** The shipped Vazirmatn faces are
  patched to a ~1.05 em line box (`hhea`/`sTypo` at `1650/-500` on 2048 UPM, `USE_TYPO_METRICS`
  set) while their outlines reach `yMax 2163` / `yMin -1160`. Latin ink fits; Arabic marks do not,
  so **a crop no reviewer on a Latin locale can see removes the hamza above an alif and the dots
  under a final ya.**
  - **`opacity` under 1 rasterizes the subtree into a texture sized to child *geometry*.**
    `Opacity::need_layer` (`i-slint-core/items.rs`) has **two** bails: exactly 1.0, and a lone
    child that is itself childless — so a hairline divider's `opacity: 0.5` is free where the same
    line over a layout is not. Past both, the texture is sized by `item_children_bounding_rect` —
    the union of subtree geometry, and a text item's `bounding_rect` is its geometry verbatim. A
    *fade* therefore crops the marks for its whole duration and hands them back on the settling
    frame. **The union decides who bleeds** — the block's first and last children, not every
    `Text` under the fade.
    Three cures, in order of preference. **Fold the alpha into the brush**
    (`Theme.text.with-alpha(t)`): pixel-identical where elements don't overlap, no texture.
    **Where there is no brush, pass a `fade` float into the component** (`ArtworkImage` spends it
    on the fill via `transparentize` and on a childless `Image`): an image has no brush, and a
    layer clipped against an *animating* height re-allocates rather than reuses its texture every
    frame; costs a few percent of extra weight wherever the faded elements overlap. **Where the
    component's
    brushes feed its own `animate` blocks, do neither** — a fade multiplied in re-dirties them
    every frame and stalls each crossfade (the shared-brush entry above; "no pointer is on it
    while it animates" is not an escape, since you *click* the back disc to close a detail).
    Satisfy `need_layer`'s second bail instead: `IconButton`'s glyph moved out of the disc to be
    its sibling, both centred on the root so no geometry moved, leaving the disc *childless*; the
    glyph fades through `MaterialIcon`'s own `fade` and carries its own `x`/`y` rather than a
    centring layout, for the `gen_layout_info_prop` reason above. Verify in the generated tree,
    not the `.slint` — but a sub-component has no `ITEM_TREE` of its own, so anchor on names in
    the enclosing struct's item field list. **None of the three were measured**, only argued off
    `need_layer`. Unfixable in `components/view-transition.slint`, which fades whole pages with no
    brush to reach. Pinned by `library_tab_band_tests::the_hero_fades_on_the_morph_at_both_ends`.
  - **A `Text`'s default `overflow` is `clip`, and that pushes a scissor at its line box
    before a single glyph is emitted** (`textlayout/sharedparley.rs` → femtovg
    `intersect_scissor`, a per-fragment mask that cuts *through* glyphs). **`overflow: elide`
    pushes none at all** — load-bearing: it is why the track rows, Now Playing bar and hero title
    render Arabic correctly. Any `Text` that can hold user
    data wants `elide` even where nothing can overflow — **but it also lowers that element's
    layout `min` to the width of one `…`**, so an over-packed row that used to overflow one chip
    now compresses and truncates all of them (`components/meta-chip.slint`;
    `ui::chips::estimated_chip_width` keeps the wrap estimate out of reach). Sharper for a
    *fixed*-height `Text` — it is asking to be cut, so give it slack or leave the height alone.

- **A function's arguments are unreachable inside a gradient's stops, so a shared
  `tile-gradient(a, b)` helper is unspellable.** `from_at_gradient` swaps `ctx.property_type` to
  `Type::Color` per stop, and `ArgumentsLookup` yields nothing unless that field is a
  `Callback`/`Function`. Tell: `Unknown unqualified identifier 'a'` on the stop, which reads like a
  typo. **Unqualified** lookup is the half that breaks — `root.a` is fine — so the escape is a
  component with an `out property <brush>`, which has to be mounted to be read, an element per
  caller. Rust can't either: `slint` re-exports `Brush` but not
  `LinearGradientBrush`/`GradientStop`. Hence the genre tile gradient spelled out at
  `grid/genre-grid.slint`, `my-library-view.slint` and `search/top-result-card.slint`.

- **A `border-radius` past half an element's *height* becomes an ellipse, not a clamped stadium.**
  FemtoVG's `rounded_rect_varying` clamps a corner's x- and y-radii **independently**
  (`rad.min(halfw)` / `rad.min(halfh)`) and Slint passes the radius straight through, so a short
  wide rect with a large radius has arcs spanning its half-width and pinches to a lens. Any radius
  on a **size-varying** element must be clamped on both axes —
  `min(self.width / 2, self.height / 2, <cap>)`. Bit the spectrum bars at their resting floor and
  the EQ band fill at 0 dB; a fixed-size pill is fine with the usual `self.height / 2`.

- **An explicit `background: transparent` costs a discarded path per element per frame.**
  `resolve_native_classes` picks an element's native class from *which properties have bindings*,
  regardless of value, so binding `background` at all promotes it out of `Empty` (never visited by
  the renderer) into `Rectangle` — and the FemtoVG item renderer builds the path *before* it looks
  at the paint. `Rectangle` already defaults to transparent, so the binding is pure cost; delete
  it rather than spelling out the default. Only matters at scale — 65 wasted paths a frame in
  `spectrum-bars.slint` (root plus 64 bands); noise on a one-off container.

- **Generated `Path` geometry has to arrive as an SVG `commands` string, and needs an explicit
  viewbox with `fit: fill`.** `for`-`in` inside a `Path` is rejected outright (slint-ui/slint#754),
  so a computed vertex count means building the string in Rust. Then **declare
  the viewbox**: without one Slint fits the path's *own* bounding box to the element,
  renormalising every frame — a whisper draws as loud as a chorus. And `fit` must be `fill`, not
  the default `contain`, which preserves aspect ratio and letterboxes a tall narrow box into a
  sliver. Finally, emit a closed figure **lower-edge-first** so its signed area is positive:
  femtovg reads the winding to decide solid vs `Solidity::Hole`. All four bit
  `waveform-trace.slint`; `player::waveform::write_path_commands` is the writer.

- **A `<=>` on a `Flickable`'s `viewport-y` silently disables Slint's own out-of-bounds correction
  — a one-way binding doesn't.** `Flickable::init` installs a change handler that pulls a
  scrolled-past-the-end viewport back in range, guarded on
  `if *y_out_of_bounds && !y.has_binding()` (`items/flickable.rs:112`), so a `viewport-y` carrying
  a binding opts out. **The distinction is which kind survives a write.** Most scrollers here
  reach `viewport-y` imperatively (`sv.viewport-y = -o`); the constant `0px` binding the generated
  tree seeds them with is *orphaned by the first such write* (the one-way-binding pitfall above),
  and until then `0px` can't be out of bounds either way — so they keep the guard. `<=>` compiles
  to `Property::link_two_way`, which **does** survive a self-write, making the two composite views
  (`browse-view.slint`, `ArtistDetailBody`) the only permanently unguarded scrollers in the tree.
  Left uncorrected, content that shrinks under the current offset paints a **blank region** —
  Browse leaving a deep folder for the much shorter library root, or either view filtering a long
  list to a few rows. Nothing looks wrong in the source; the symptom is an empty view that fixes
  itself when you switch tabs, the content-area `if` remounting it at 0. `CompositeScrollbars`
  re-clamps on `changed v-outer-max`; a new scroller that two-way-binds `viewport-y` needs the
  same. To audit, grep the generated file for `r#Flickable :: FIELD_OFFSETS . r#viewport_y ())`
  (empty parens included): 2 `link_two_way` hits, 23 benign `set_property_binding` seeds.

- **Stock Slint 1.16 + winit 0.30 have zero OS file-drop on Wayland.** `DragArea`/`DropArea` is
  in-process only — no `PathBuf`, still true of released 1.17.x. Vendored winit PR #4009. Fix
  merged upstream but unreleased: winit **#4571**, a new `DataTransfer` DnD API superseding
  #1881/#4009.

- **`changed <local-prop>` doesn't fire when first layout settles directly on final value.**
  Native-titlebar reaches final grid width in one pass — derived counts never *transition*. Fix:
  pair the handler with a single-shot 1 ms `Timer` firing the body once at mount. **The
  `changed width => { self.<mirror> = self.width; }` idiom is the recurring victim and always owes
  that seed**: a window opened at its final size never *resizes*, so the mirror keeps its declared
  seed forever and every width derived from it is wrong until something moves the window. Bit
  `settings-view.slint`'s `page-w` and `now-playing-bar.slint`'s `bar-w` (the latter hidden by
  consumers that `clamp` well below its 1100 px seed); both now carry the timer.

- **A `changed` handler inside an `if` outlives its branch, and whether that is fatal comes down
  to what it watches.** The generated `ChangeTracker` lives *in* the repeated component and holds
  a `VWeakMapped` back to it, and `ChangeTracker::evaluate` opens with
  `self_weak.upgrade().unwrap()`. Dropping the branch drops the strong refs but `VRc` keeps the
  allocation alive for the weaks, so the tracker stays registered in `CHANGED_NODES` with nothing
  to upgrade to. It only fires if something re-dirties the watched property *after* the drop — the
  whole distinction: a tracker reading a property driven from **inside** the branch dies quietly
  (`tooltip.slint`'s `changed hovered`, fed by its own `TouchArea`), where one reading a **layout**
  property is re-dirtied by the *surviving parent* the moment it re-flows without the child
  (`meta-chip-strip.slint`'s `changed watched-w`). So the rule is about the **condition**, not the
  handler: a branch carrying a layout-watching tracker may only be dropped when nothing has
  re-dirtied that property in the same frame — never by an animated property, never by anything a
  `changed` handler writes. **Don't read "a discrete write from Rust" as the cure — the quiet
  frame is what makes it safe, not the writer.** A `Timer`'s `triggered` body and a Rust callback
  both sit outside the change-handler pass and **both still panic** if they drop a branch whose
  width the frame just moved; an animated predicate is fatal because a morph re-flows the branch
  on *every* frame including its last.
  Symptom: `called Option::unwrap() on a None value` at `app-window.rs` in
  `InnerFoo::user_init::{closure#N}`, on a frame with no obvious cause, naming a component nowhere
  near what you touched; the tell in the generated source is a
  `change_tracker<N>.init` whose *eval* closure reads a `_width` / `_height` / `_watched_w`.
  The shape to copy is `library-tab-band.slint`: everything tracker-free rides one
  `hero-shown: root.detail-open || root.hero-t > 0` (which also stops the two halves mounting a
  frame apart), and **the chip strip is mounted for the life of the band**, fading by brush alpha
  through `HeroBackdrop.chip-fill-at` — brush alpha rather than `opacity` because a permanent
  mount satisfies neither `need_layer` bail. Pinned by
  `ui::library_tab_band_tests::the_chip_strip_outlives_every_morph_it_is_painted_in` (a brace
  walk, not an indent walk — the band has `if`-gated *siblings* at shallower depths).

- **`init` runs *after* bindings resolve to final values — useless for entry animations.** Setting
  `shown: true` in `init` makes `true` the *initial* value, so `animate opacity` never runs. Fix:
  single-shot 1 ms `Timer` flips `shown` once at mount.

- **Glyphs outside Vazirmatn inflate `Text` line-box and break patched-metrics centring.** Unicode
  arrows or any glyph the bundled font lacks pulls a fallback font; its taller `typoAsc/Desc`
  defines the line-box, so patched glyphs drift down. Fix: render the foreign glyph as a sibling
  `MaterialIcon` (collapsed em-box).
