---
paths:
  - crates/melodia-ui/ui/**/*.slint
  - crates/melodia-views/src/ui/**/*.rs
  - crates/melodia/src/boot/**/*.rs
  - crates/melodia-core/src/themes/**/*.rs
  - crates/melodia-ui/build.rs
---

# Slint Pitfalls (battle-tested)

Melodia-specific, each paid for once already. General Slint patterns are in `slint.md`;
this file is what builds, looks right, and is wrong.

- **`visible: false` doesn't remove from layout.** Hidden child still claims stretch.
  Fix: `if !collapsed: VerticalLayout { … }`. Ref: slint#7377.

- **A box collapsed with `clip: true` cuts everything its contents draw past it, collapsed or not.**
  A zero-width clip is a tempting way to hide something while keeping its ids readable, but the
  clip applies while the box is open too, and a box sized to its content leaves no room for what a
  control paints outside its own geometry: a focus swell, a drop shadow, a tooltip. It bit the
  shared tab header's search box, whose swollen ends were shaved flat. Collapse through `visible`
  instead: `passes/visible.rs` lowers it to a `Clip`
  whose `clip` is `!visible`, so the element is clipped only while hidden, and hidden it draws
  nothing and takes no events. Layouts are lowered before that pass, so the element's own width
  constraints still collapse it (`tab-search-header.slint`).

- **Don't `animate` a property driven by both toggle and continuous input** (drag micro-updates
  get full easing → spongy). Switch the ease off for the drag and keep the token:
  `animate width { duration: Theme.dur-med; enabled: !Nav.sidebar-dragging; }`. A disabled
  animation lands on its target at once, imperative writes included (`layout/sidebar.slint`,
  `tab-bar.slint`'s `compact-t`).

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
  **And an animated property cannot land on the frame that dirtied it, so it may not be paired
  with a snapped sibling off the same flag.** `mark_dirty` only sets `ShouldStart`; the first
  `evaluate` runs on the next render and takes `from_value` from whatever is on screen then, so
  the curve opens at zero on the frame its partner has already finished. No duration closes that
  gap, and a short one turns the wait into a jump rather than into simultaneity, which reads as
  an easing problem and is not one. `entity-card.slint` carries both halves: its selection ring
  is a plain `border-width` and its tint is an `if root.selected` fill with no `animate` at all,
  `selection-pill.slint`'s shape, which is why a track row's pick never had it. Pinned by
  `crates/melodia/tests/card_selection.rs`'s `the_card_selection_ring_and_tint_never_animate`,
  which reads the tint's own braces: an `animate` elsewhere in that file is ordinary.

- **A `for` over a `pure callback`'s model rebuilds every instance whenever one of the call's
  inputs is marked dirty, not when its answer changes.** The same structural dirt as the entry
  above re-runs the binding, and `Repeater::model` (`i-slint-core/model/repeater.rs`) compares the
  old and new `ModelRc` **by pointer**, so a callback building a fresh model per call drops and
  rebuilds the whole repeater, trackers and tooltip layers included. Symptom: a page that stutters
  through a resize drag while nothing on it visibly changes, because every width feeding an argument
  re-ran the call. Cure: hand back the *same* `ModelRc` for the same answer, or better, keep the
  model flat and let a layout do what the callback was computing, which is why the chip strips are
  `FlexboxLayout`s over one list rather than rows split for them.

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
  `animate`s, which stand down for the one writer that does step their defaults, the palette fade
  below. Pinned by `ui::tab_bar::tests::`
  `{the_cell_eases_floats_and_never_a_brush, the_selected_colour_crosses_over_two_matched_layers}`.
  **The mirror image bites the host**: a layer may not ease *out of* a value nothing was painting.
  `HeroBackdrop` is held across a My Library tab leave, so a mirror bound to it unconditionally
  settles on a hero the band stopped painting and crosses out of it on the next open — a genre's
  pink under a playlist. Cure at the source, not the animation: make the idle value honest
  (`root.detail-open ? <tier> : <idle token>`), suppressing the ease on the right frame being
  impossible when the colour and the id land in the same tick. See `ui-patterns.md`.

- **`Theme`'s palette brushes are stepped every frame while one Material You palette fades into the
  next, so an `animate` whose binding can read one carries `enabled: !Theme.recoloring`.** Slint
  can't animate a global, so `ui::appearance::theme_paint` writes a blend each frame and holds
  `Theme.recoloring` up for the fade. An ungated `animate` on a palette brush restarts on every
  write and lands after the window: the entry above, at the scale of the whole window. Disabled, it
  takes each written value as it comes and so rides the fade. **"Can read" includes a defaulted
  input**: `IconButton`, `OverlayScrollbar`, `ToggleSwitch` and `StarRating` ease brushes their
  hosts hand over, and every one of those defaults to a `Theme` token. A float, an `np-*` or
  `HeroBackdrop` tier Rust writes once a solve, and a `states` transition, whose timeline survives
  the dirt, need no gate. Where one binding switches between the two kinds, gate on the switch:
  `library-tab-band.slint`'s mirrors take `root.detail-open || !Theme.recoloring`, and the
  miniplayer's mirrors and those four leaves `root.on-backdrop || !Theme.recoloring`, so a hero or
  backdrop tier keeps easing through a fade. A leaf can't see which kind it was handed, so a host
  passing it a tier sets its `on-backdrop`. Missing, either gate shows only during a fade: an
  ungated control settles after the rest, and a tier-fed leaf left unset jumps ahead of it.

- **Drag handles inside resized element need absolute coords.** Snapshot
  `start_abs = self.absolute-position.x + self.pressed-x` on `down`, then
  `parent.width = clamp(start_w + (self.absolute-position.x + self.mouse-x - start_abs), ...)`.
  `components/edge-resize-handle.slint` is the one that does it, mounted by the sidebar and the
  queue panel; a new resizable edge mounts it too.

- **Material Symbols glyphs need a collapsed line-box.** `Text` defaults to ~1.2× `font-size`;
  pin inside fixed `icon-size × icon-size` Rectangle. `MaterialIcon` does this.

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
  Pinned by `crates/melodia/tests/entity_card_overlay.rs`'s
  `no_overlay_host_positions_against_parent`, which finds the
  hosts by the flag that opens the slot and reads **the mount's own braces**: a host is free to
  spell `parent` elsewhere in its file, `x: parent.width - self.width` being the canonical
  `OverlayScrollbar` mount, so a file-wide ban would fail the first overlay host carrying a bar.

- **`height: 100%` on child + `height: Npx` on parent → unbounded layout.** Row swallows whole
  body; sibling `ListView` renders 0 rows. Pin fixed-size rows with `min-height` + `max-height` +
  `vertical-stretch: 0`. Never `height/width: 100%` on layout child.

- **Nested ScrollView + ListView need `content-height: self.height` on outer.** Reverse: lock
  `content-width: self.width`.

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

- **The same `Flickable` steals a *mouse* drag wherever its drag-pan is on, and there the cure is
  per-list: `mouse-drag-pan-enabled`.** A row drag and a drag-pan are one gesture and only one
  element can own it. **Which one wins is the *style's* answer rather than Slint's**: the bare
  `Flickable.mouse-drag-pan-enabled` defaults **`true`**, and every style's `ScrollView` binds its
  own `mouse-drag-pan-enabled` to its flickable's and pins it `false`, Material included.
  `interactive` is a separate property left at `true`, and it alone decides a touch, so a
  touchscreen pans every list whatever `mouse-drag-pan-enabled` says.
  `ListView inherits ScrollView`, so it takes the same answer, and the tree declares no bare
  `Flickable` at all. Melodia sets no style and `i-slint-compiler`'s `typeloader.rs` resolves it to
  **fluent**, so the live default here is *off* and every scroller binding nothing already
  compiles to `false`. What the opt-outs buy is therefore a pin rather than an override: no
  click-to-act list depends on a style's default. **Verify in the generated file, never in the
  `.slint`** — the value is inlined per instance, so a source binding and an inherited default are
  the same line.
  Where the pan *is* on, a press inside one returns `DelayForwarding`, and `handle_mouse_grab`
  (`input.rs`) re-consults exactly those ancestors *during* the inner grab; past
  `DISTANCE_THRESHOLD` (8 px) inside `DURATION_THRESHOLD` (500 ms) on a scrollable axis it returns
  `Intercept`, and every item below is sent `MouseEvent::Exit`, delivered by `TouchArea` as
  `PointerEventKind::Cancel`. A list's `DragSource` takes the press at that same 8 px, so on a list
  that pans, which under a touch is every list, the two compete for one gesture. It reads as
  intermittent, both escapes being real: a list shorter than its viewport can't flick, and a press
  held past 500 ms before moving is never intercepted. The draggable lists opt out outright,
  `draggable-track-list.slint`'s `inner-list` and
  `queue-panel.slint`'s, pinned by
  `ui::playlists::tests::every_draggable_list_opts_out_of_drag_panning`, since it only misbehaves
  under a pointer; the click-to-act grids and lists are pinned by `crates/melodia/tests/scrollbars.rs`.
  **`!reorder-enabled` is what that binding used to say, and is the trap worth keeping**: it reads
  as leaving a `true` default alone and instead *enables* the pan on every sort that retires the
  drag. The wheel scrolls either way, so only the mouse drag-pan goes.

- **A grabbing item that unmounts mid-gesture hands its release to the ancestor, as a fresh
  event.** `handle_mouse_grab` walks the `item_stack` upgrading each weak ref, and a grabber whose
  branch has gone sets `invalid`, which returns `Some(event)` and drops the whole gesture back into
  `process_mouse_input` for a normal hit test. So an ancestor `TouchArea` that reads
  `PointerEventKind.up` out of `pointer-event` sees a release for a press it never got, on a click
  the user aimed at something else entirely. Anything hover-mounted whose action rewrites what it
  sits in is the shape to watch: a track row's favourite toggle and star strip hang off
  `if touch.has-hover` and patch the row, and the row body picked the row the user was rating.
  **Cure: act on `clicked`, which `items/input_items.rs`'s `Released` arm gates on that item's own
  `pressed`** — never set for an area that missed the press, re-dispatch or not. It costs the
  modifiers, `clicked` carrying none, so a range-pick snapshots `ev.modifiers` on the `Down` arm
  instead. `clicked` also runs *before* `pointer-event(Up)` in the same arm, which is what lets one
  handler read a predicate the pick is about to invalidate (`entity-card.slint` opens a detail only
  when the click was not a pick).

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
  once content wraps, i.e. a narrow window or a long-plural locale. The wrapper is also the one
  thing outside the `if` the root can read its height off, an id inside an `if` being unreadable
  from out there. **Not for a flex**, which inside an `if` answers at the wrong width (the entry
  on a flex inside an `if`, below). **The fold runs in *both* orientations**, and `layout_items`
  never shrinks a cell past its `min`, so a wrapper's `min-width` becomes a floor under whatever
  holds the root. An explicit `min-width` **replaces** the merged constraint rather than maxing
  with it, so one line is the whole cure (`settings/chip-group.slint`, whose flex would otherwise
  floor the card at its widest chip). Nothing higher up saves you: the same replacement happens on
  the root `Window`.

- **A box layout holds a stretched child to that child's own `min`/`max` on the cross axis, and the
  std `ScrollView` floors itself at 50 px.** `clamp_cross_stretch_size` (`passes/lower_layout.rs`)
  wraps the column's width in the child's explicit constraints, where 1.16 handed the column over
  as is. So a std scroller (`ListView` included, which inherits the floor) in a narrower column
  overflows it, and everything sized off its `content-width` goes too: the collapsed sidebar's rows
  drew their fill past the rail. A wide column never shows it and nothing warns. Cure:
  `min-width: 0px` at the mount, which replaces the widget's own (`layout/sidebar.slint`'s
  `nav-scroll`). Tell: `.max (50f64` on the binding in `app-window.rs`.

- **A sized cell of a box layout sits at the start of the cross axis, and centring it moves every
  cell of that layout onto another solve.** On the plain path a cell is stretched and clamped to
  its explicit constraints (the entry above), which pins a fixed one to the start edge.
  `cross-axis-alignment: center` on the layout, or `cross-axis-self-alignment: center` on one cell,
  sends the whole layout through `solve_box_layout_ortho` (`i-slint-core/layout.rs`), and three
  things change with it:
  - **A stretched sibling is clamped to its implicit min/max too**, so a cell that has to fill the
    row stops at whatever max its contents report. Self-align only where every stretched sibling's
    max is unbounded: a `Text` or `TextInput`, a `Rectangle` whose children are all `if`s, a layout
    of such cells. Otherwise the row stays on the plain path and keeps its wrappers
    (`queue-row.slint`, whose duration cell reports the remove button's 26 px while it shows).
  - **An explicit `x`/`y` on a cell is a build error**, the solve owning every position.
  - **A removed wrapper can cap the row.** A row's cross-axis info ignores `cross-axis-alignment`
    and takes the least of its cells' maxes, and a wrapper whose own `alignment` wasn't `stretch`
    reported an unbounded one where its fixed child reports its own size. A row with an explicit
    cross size never notices; a host with surplus to hand out finds the row has stopped growing.
    That is what the `alignment: start` wrappers round fixed children are for, no-ops in position
    that keep the max open.
  A lone `Text` centres its own lines with `vertical-alignment` and stays on the plain path, and a
  sized child of a plain `Rectangle` is already centred (the default-geometry entry below), so
  neither wants a layout round it. Tell: a converted layout calls `solve_box_layout_ortho` in
  `app-window.rs`.

- **`VerticalLayout` divides surplus equally when every child has `vertical-stretch: 0`.** Append
  trailing `Rectangle { vertical-stretch: 1; }`, or set `alignment: start`, which hands out no
  surplus at all, a stretching child included: under it the smart editor's value field sat at its
  preferred width beside a row of free space. Inside a `GridLayout` one of the two is mandatory: a row is as tall as its
  taller cell and the shorter is stretched to match, so surplus lands inside whatever that cell
  contains — a settings body column left on `stretch` grows its cards instead of ending above the
  taller column's floor.

- **A `GridLayout` cannot do masonry, and the cure is to make each cell a column rather than an
  item.** Cells are top-aligned to a row as tall as its tallest, so a short item is followed by
  dead space down to the next row. No per-column flow, and a `FlexboxLayout` of columns can't stand
  in for it over cards like these (the flex measuring entry below).
  Restructure: one cell per column, each a `VerticalLayout` of the items. Two consequences:
  stacked, it is column A's items then column B's, so columns want to be **contiguous halves** of
  the list rather than alternating; and a page with fewer items than columns leaves one empty, so
  an item that must not fill the surplus width needs its own `max-width` (`SectionCard` pins one
  beside its `preferred-width`).

- **A `FlexboxLayout` measures its items at a width it hasn't committed to, and only a
  layout-rooted component can answer.** taffy solves both axes in one pass, so placing an item
  whose height follows its width means asking that height at a trial width, through the
  synthesized `layoutinfo-v-with-constraint`. The compiler synthesizes it on *layout* elements only
  (`passes/lower_layout.rs`), and a host treats a child as height-for-width only when the child's
  component root has one (`llr/lower_layout_expression.rs`, `is_height_for_width_cell`). A
  `Rectangle`-rooted component whose height follows its width (`ChipGroup`, `SettingRow` with its
  wrapped label, …) is read at its *drawn* height instead, which
  depends on its drawn width, which comes from the solve still in progress. Symptom: `Recursion
  detected` at `i-slint-core/properties.rs`, at run time only and on the first layout, through
  `layout_item_info_at_cross_width` and `*_layoutinfo_v_with_constraint` frames; the build and
  every test stay green. A flex Settings body of card columns hit it on opening a page of rows,
  which is why that body is box layouts, solving widths before heights. **What a flex may hold
  is fixed-height, layout-rooted or `Text`-rooted**, wrapped or not: a `Text` is a builtin
  height-for-width cell, so the flex hands it the trial width as its `cross_axis_constraint`, which
  `text_layout_info` (`items/text.rs`) reads ahead of the drawn `width`. Every flex in the tree
  keeps to that: chips, pills, swatches, tiles, and the smart editor's rule rows, whose `Dropdown`s,
  value field and remove button each bind a constant height beside a `Text`-rooted `RowLabel`. A
  root `height` bound to its own flex's `preferred-height` (`ChipGroup`, `MetaChipStrip`) is read
  at the drawn width too, so neither may sit in a flex itself.

- **A `FlexboxLayout` reports no maximum height and stretches its lines into whatever it is
  given**, `cross-axis-line-alignment` defaulting to `stretch` (taffy's `align-content`). A host
  with height to spare spreads the rows apart, and nothing says so until one has some. Three
  answers in the tree: bind the root's `height` to the flex's `preferred-height` (`ChipGroup`), set
  `cross-axis-line-alignment: start` (the mosaic picker, whose viewport outgrows one row of tiles),
  or hand the flex no surplus (`alignment: start` on `RecentSearches`' root).
  **The width half is the opposite trap**: a wrapping flex's `preferred-width` is a near-square
  block (`flexbox_layout_info_main_axis`), not its items on one line, so a host that hands a strip
  only its preferred width (`alignment: start`, a zero-stretch cell) wraps it where a line would
  fit. Every strip in the tree fills a column; one moved into a row beside a control won't.

- **A `FlexboxLayout` inside an `if` reports its height at its near-square preferred width, not
  at the width it is drawn.** The repeated component's `layout_info(Vertical)` hands its
  `fn_layoutinfo_v_with_constraint` its own `layoutinfo_h.preferred`. A box layout's solve asks
  `layout_item_info_at_cross_width` instead and places the flex right, which is why Radio's
  suggestion row draws correctly; what goes wrong is anything reading the layout's *reported*
  height, so a root binding its height to a wrapper round the `if` reserves the lines a block a
  few chips across needs. It gave Now Playing a band of blank lines under its chips. Cure: the
  flex as a direct child, measured at its root's width, hidden through `visible` where its lines
  should stay reserved (`MetaChipStrip`'s `show-rows`). Tell, in `app-window.rs`: a repeated
  flex's `layout_info` hands `fn_layoutinfo_v_with_constraint` its own `…_layoutinfo_h ()` …
  `.preferred`, where a direct child's `flexbox_layout_info_cross_axis` ends on its root's
  `…_width ()`; the first is a bug only where something reads that layout's reported height.
  Pinned by `ui::hero_chips::tests::the_strip_measures_its_rows_at_the_width_it_draws`.

- **The same holds for any component behind an `if` or `for`, and a pin on its root can't
  steer it.** The vertical arm measures at the width the root's *child layout* computes; an
  explicit `preferred-width`/`max-width` on the root reaches only the horizontal arm (the compiler
  calls it "a single-line-height approximation", `get_layout_info_v_constrained_for_repeated`). So
  a row of wrapped text reports one line, and the card sized from it lets the rest spill out below.
  Shrinking the texts' preferred width to let a root pin win overshoots the other way: the row is
  measured as a narrow column and its card runs on behind the next. Cure: pin a **cell**, where an
  explicit `preferred-width` replaces what the cell asks for (on a component root it only raises
  it, `LayoutInfo::merge` taking the max), so the row is one cell of a `VerticalLayout` carrying
  the pin (`setting-row.slint`, `SectionCard`'s body). Tell, in `app-window.rs`: the repeated
  `Vertical` arm reads `(… + …_layoutinfo_h ()).preferred` where the `Horizontal` arm shows the pin.

- **`GridLayout` honours a runtime expression for `row`/`col` on a plain child, and forces `Auto`
  on a repeated one.** `convert_row_col_expr` lowers `RowColExpr::Named` to a `PropertyReference`
  read per layout pass, so `row: SomeGlobal.grid-row(1)` re-flows the grid reactively, which is how
  a grid swings its columns from side-by-side to stacked with every item still **mounted exactly
  once**. Two constraints. (1) `passes/lower_layout.rs`'s `add_element` forces
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
  `Theme.base` sits between it and the panel border at every scroll position. Inset
  **left/right/top only**; for clearance at the end, put `padding-bottom` on the column *inside*
  the viewport. A track list page pads no sides at all, the list owning its own insets
  (`ui-patterns.md`'s track-list inset entry).

- **`changed` doesn't accept path expressions on globals — mirror via local property.**
  `changed Nav.selected-index => {}` fails to parse. Use
  `property <int> watched-nav-idx: Nav.selected-index; changed watched-nav-idx => {}`.

- **A faked placeholder `Text` in a non-layout parent paints out of its input, and
  `overflow: elide` alone doesn't stop it.** Slint's raw `TextInput` has no `placeholder-text`, so
  both placeholders in the tree are a sibling `Text` gated on the field being empty:
  `single-line-input.slint`'s, which every single-line field mounts, and
  `multiline-input.slint`'s. Under a
  plain `Rectangle`, `make_default_implicit` sizes each to `max(preferred, min)` of its **own**
  `layout_info` — the untruncated string — so it paints through the pill's right edge, and the
  default `clip` is against that same full-string width. Elide is half the cure: `text_layout_info`
  lowers only `min` to one `…`, leaving `preferred` at the full string, so a non-layout parent
  hands over the larger and elide never fires. **Bound the width *and* elide.** The single-line
  `TextInput` beside it is the opposite case on purpose: it grows wider than the field once its
  text does, sliding to keep the cursor in view, and the component's own scissor `clip` holds it
  in. `multiline-input.slint` is exempt — bounded to its
  scroller (`width: sv.width`) and wrapping rather than eliding. A **locale** bug in practice,
  English fitting every slot where the catalogues run ~1.3× longer; pinned by
  `ui::placeholder_tests`.

- **A `SearchBar`'s slot is sized off its own placeholder, so its root spells
  `min-width`/`preferred-width`/`max-width` and never binds `width`** — a bound `width` is
  reported as both `min` and `max` and would drag the window's resize floor along with a
  locale-sized default. The natural width is `input.placeholder-width + chrome-overhead`, which
  `SingleLineInput` publishes off the hint it draws (the `label-w` idiom: a Text's *horizontal* layout info is
  intrinsic to the string and never reads back the width it was handed, so it's a derivation and
  not a cycle). Only the **default** is measured — the four hosts that set `input-width` outright
  never evaluate it. What every mount keeps is the `min-width` floor: below it the bar compresses
  and the placeholder elides. That floor is **published as `out property min-w`**, the
  `TabBar.compact-w` contract — a host budgeting its row reserves against it rather than restating
  the number. **A host with no room for the floor folds the bar instead**, easing `fold-t` while
  the slot narrows onto a `bar-height` square around the glyph and the floor gives way to it; the
  input stays mounted at zero width, so it keeps its focus target and its `blur-trigger` tracker.
  `ui::placeholder_tests` pins both ends.

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
  **Where nothing underneath takes the wheel, the flag stays down until the pointer moves**, opaque
  fill or not: Slint re-sends the pointer's position only after a wheel event something accepted. A
  control that takes the wheel itself answers `accept` on every event, an empty one included, and
  keeps its hover (`Player.wheel-volume`).

- **Reusable filter SearchBar pattern.** (1) `text <=> SomeGlobal.filter`,
  `blur-trigger: SomeGlobal.blur-search-tick`; (2) backdrop
  `TouchArea { clicked => { SomeGlobal.blur-search-tick += 1; } }` at view root before content;
  (3) clear filter + bump blur tick on nav-away. Match through `Needle` (`ui-patterns.md`'s
  Filtering: `crates/melodia-views/src/ui/row_match.rs` for a list held in memory, a folded column
  in SQL for a library-sized one), never a hand-rolled `to_lowercase().contains(…)`; Settings
  routes there via `pure callback SettingsPage.matches(...)`.
  **A page whose box describes more than one surface takes the same three steps against one global
  and dispatches in Rust.** My Library's band is the tree's only one: the bar binds
  `MyLibrary.filter` / `.blur-search-tick`, and `ui::my_library::filter::dispatch` routes a
  settled keystroke to whichever of nine surfaces is mounted — a *write*, since a `.slint` binding
  belongs to the scope it is written in, making `Tracks.filter: MyLibrary.filter` unspellable.
  Two things follow. A tab pick clears **both sides**, but the entering tab's
  own filter only when there *is* one: `filter::clear_mounted` guards on that needle being
  non-empty, a dispatch otherwise costing Songs a requery for nothing, and a grid's own load waits
  for its enter's reload, the pick running ahead of the section gate. And **anything that changes
  what the box *means* with
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
  three `SettingsPage.matches` round-trips per row across every category page, so the expensive
  work is Slint's own re-evaluation and the settled value must be a property it can read. Clear
  both on nav-away and on a category pick; clearing only the settled one leaves the box holding
  text the page is no longer filtered by.

- **`PopupWindow.y: -self.height - …` needs explicit `width`/`height`.** Else `self.height` is 0
  before first layout — popup lands above trigger top, expands downward. Canonical:
  `components/now-playing/overflow-menu.slint`.

- **An open `PopupWindow` re-lays its content at once and moves a loop pass later.** Everything
  inside reads the popup's new size on the frame that changed it, while its origin and box are
  re-read by `WindowInner::update_popup_properties` on the zero-delay timer the popup's
  `PropertyTracker` schedules, which runs after that frame is drawn. So a popup whose origin moves
  while open (end-aligned, opening upward, growing leftward) draws one frame of its new layout at
  the old origin. Two answers in the tree. Keep the origin fixed: `OverflowMenu` reserves its
  widest flyout, and a right-click menu opening left holds its flyout's room from the start
  (`ContextMenuMetrics`, applied by `ContextMenuPopup`). Or keep the content still while open:
  `Dropdown`'s `about-to-open` returns the revision its refresh will publish and shows the popup
  once that lands, the output device picker being the host that needed it
  (`components/settings/dropdown.slint`).

- **A `PopupWindow` taller than the window is drawn past its edge.** `place_popup` does shrink an
  oversized rect to the window, but its two callers in `i-slint-core/window.rs`,
  `WindowInner::show_popup` and `update_popup_properties`, keep only the origin it hands back. The
  popup is parked at the window's top at its full height and the rows past the bottom are cut off,
  with nothing reporting it, and only on a window short enough to show it. Cure: place it yourself
  through `components/popup-fit.slint`'s `PopupFit`, which slides a popup raised at a point
  (`fit-y`) and opens one hanging off a control on the side with room, capped at it
  (`opens-above`, `side-h`), and list its rows in a `PopupColumn`, which scrolls what that box
  can't hold. Every popup whose rows can outgrow the full UI's shortest window places itself that
  way and lists its rows in a `PopupColumn`, but for `FacetChip`, whose virtualized list scrolls
  itself; the rest are short enough that Slint's own slide never cuts them.

- **A press outside an open `PopupWindow` closes it and reaches nothing else.**
  `WindowInner::process_mouse_input` closes the popup on that press and, the pointer being outside
  a popup that isn't a `Menu`, dispatches it nowhere, so whatever it landed on never sees it. The
  release does arrive, the popup being gone by then. Hence a right-click menu raised on the press
  needed a second right-click to move to another row, and every menu now opens on the release
  (`ContextClick`). An element that closes its own popup on a press hands its release onward the
  same way, the grab going with the popup (the grab-invalidation entry): `PopupDismissCatcher`
  leans on it.

- **An open `PopupWindow` takes the hover, and the keyboard, from everything outside it.** The
  same dispatch hands a pointer event to the popup or to nothing and sends every item outside an
  exit, so the `has-hover` of whatever raised it reads false for exactly as long as it is up, and
  the popup takes focus on `show()` and hands it back on close. A row or a field goes dark under
  its own menu, and nothing marks what the menu is for. Cure: read the popup's `is-open` beside
  the flag, which the runtime sets on show and clears on every close. `IconButton.force-bg` is the
  trigger's spelling and `SingleLineInput.has-focus` the field's; `ui-patterns.md` has the item
  menus.

- **A `TouchArea` grabs on a press of any button, and only `pressed` is left-only.** So its
  `mouse-x`/`mouse-y` keep tracking the pointer until that button comes up, wherever it goes, and
  an open popup follows its `x`/`y` (the entry above). A menu anchored to a host's live
  `touch.mouse-x` dragged after a held right button. A menu takes its anchor once, through
  `show-at`. A slider's `pointer-event` arms on that same press of any button, so it filters the
  button itself: `SliderPointer` is the one place that does, and a right click moves no value.

- **A child with no `x`/`y` in a non-layout parent is *centred*, not at the origin** — Slint
  generates `(parent.width - self.width) / 2`, which reads as 0 only for a child as wide as its
  parent. **A zero-size wrapper therefore lands at half the parent**, and anything positioned
  against it goes with it: `TrackContextMenu` holds its `ContextMenuPopup` in such a wrapper, and
  the row passed a pointer position measured in the row's own frame, so the menu opened half a row
  right of the cursor on both its hosts. A wrapper meant to be a coordinate frame pins `x: 0px; y: 0px`
  **inside the component**, where a host cannot forget it.

- **`absolute-position` inside a `PopupWindow` is measured from the popup's corner, not the
  window's.** It lowers to `map_to_window`, which walks parents with `StopAtPopups` and never adds
  where the popup was placed (`map_to_native_window` does, and a binding can't reach it). An
  element comparing it against window geometry therefore sees every edge nearer than it is, off by
  the popup's own offset, so it looks right wherever that offset happens to be small. `Tooltip`
  takes `in-popup` for exactly this, set at both volume readouts.

- **A flyout opens *inside* the overflow menu's single `PopupWindow` — no nesting.** The
  playback-speed row (`speed-flyout.slint`, presets in shared `flyout-presets.slint` globals) is
  the worked example. Fixed-reserve geometry, as with the volume popup: size the popup for
  menu-column + flyout up front and bottom-anchor both, so opening a flyout never moves the
  popup's origin (the entry above) and the menu stays pinned under its trigger; a transparent
  full-popup `TouchArea` **declared first** closes on stray clicks.

- **Sleep timer = a session-only cancel-and-replace tokio countdown + a `PlayerState`
  end-of-track flag.** In `crates/melodia-views/src/ui/sleep_timer.rs` — a **UI-layer** module, because `tasks/` may
  not import `ui::*` and the countdown writes a Slint property. **Duration is
  playback-linked**: each 1 s tick decrements only while `status_atomic == Playing` (lock-free),
  so pausing holds the timer and it never expires on a paused player. Never persisted; bounds
  `[30 s, 2 h]`; `Player.set-sleep-timer(minutes)` takes 0 off / `>0` duration / `-1`
  End-of-track, the last arming `PlayerState::pause_after_current_track` (monitor half in
  `.claude/rules/audio-stack.md`).

- **Flash-free image cross-fade = two slots, never cleared under a fade.** Two stacked `Image`s +
  `use-a` bool; Rust writes the new image into the *inactive* slot then flips the bool so both
  `opacity` animate. A write never resets the outgoing slot, which stays painted for the fade. A
  slot is cleared only where no fade can still be reading it: a station, which nothing fades
  *into* (`ui::now_playing::source_change`), a hero handed back with its page
  (`detail_view::release_hero_slots`), and Now Playing once no surface draws it, emptying only the
  slots not on show and only after `dur-med` (`NowPlayingState::release_artwork`). Each argues
  itself there.

- **A rounded `clip: true` renders everything under it into a texture, and a clip on a rounded
  element is always a rounded one.** `passes/clip.rs` copies the element's radius onto the `Clip`
  it creates, and `visit_clip` sends any non-zero radius through `render_layer`: the children draw
  into a texture at physical size, drawn again whenever anything they read changes (the texture's
  tracker registers with the one around it, so a nested one dirties its parent too), then filled
  back through an antialiased path. A radius-less clip is a scissor with no texture, and **a
  `border` never makes one** (`draw_border_rectangle` draws directly). Two costs follow. The
  texture is sampled linearly on the way back, so one landing on a fractional pixel softens the
  text in it; and a clip around a whole view renders that view into its texture again on every
  scroll and visualizer frame. Where the clip only guards overflow, keep `clip` on an element with
  no radius and move the rounded fill into a child pinned at `x: 0; y: 0` (the track rows, the
  header cells); where the content has to read rounded over an opaque ground, `RoundedFrame` masks
  the corners instead (the content panel). A scrolling list in a card clips its viewport with the
  `ScrollView` and keeps its fill and border on the card, inset from the border
  (`components/dialog/selectable-picker.slint::PickerListCard`).

- **FemtoVG antialiases every rounded fill, stroke and clip on its own, and the coverages of one
  curve stack.** A pixel the arc half covers is painted past half by each further shape on the
  same curve, so the corner reads harder than its radius, as if the rounding started later.
  Nothing warns, and a single shape never shows it. The stacks this tree had: a `background` under
  a rounded clip whose content fills to the edge (every cover tile), a border element laid over a
  separately rounded fill, a stroke ending on a clip's edge, and a fill drawn over a whole track
  of the same shape. Cures: the fill as a **square** child inside the clip (`ArtworkImage`, the
  window shell); fill and border on **one** element, where the renderer tucks the fill under the
  stroke; a stroke inside a rounded clip as an `EdgeOutline`, which argues its straddle; and a
  track drawn only beside its fill (`ProgressBar`, `EqBandSlider`).
  **The one-element cure holds only for an opaque border.** `draw_border_rectangle` tucks the fill
  under an opaque stroke and runs it out to the edge under a translucent one, so a translucent
  border (the lyrics pill, a focus ring, a colour fading through `transparent`) stacks again. Tuck
  the fill yourself with a `TuckedFill`, or keep the border opaque (the dropdown rests on its fill
  colour). **Except over a drop shadow**: the shadow sits under the whole element, and a tucked fill
  lets it show through the rim, which reads worse than the second pass.

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
    **A dimmed state is the long case**: a disabled button or a row outside the library holds its
    texture for as long as it stays dimmed, not for the length of a fade. The dimmed row, card,
    button and menu row carry a `fade` float into every brush instead, the track row folding it
    into one `ink` its cells share.
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
    now compresses and truncates all of them (`components/meta-chip.slint`, safe only because its
    strip breaks lines on the chips' real widths). Sharper for a
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
  `min(self.width / 2, self.height / 2, <cap>)`. Bit the EQ band fill at 0 dB, and the spectrum
  bars at their resting floor before they gave their radius up entirely; a fixed-size pill is fine
  with the usual `self.height / 2`.

- **A rounded `clip: true` with a square corner beside rounded ones paints its children over the
  border at the square corner.** A clip with any radius renders its children into a layer and
  fills it through `clip_path_for_rect_alike_item` (`i-slint-renderer-femtovg`'s
  `itemrenderer.rs`), which subtracts `border_width * KAPPA90` from **every** radius, and
  `BorderRadius`'s `SubAssign` doesn't clamp. The zero corner goes negative, `rounded_rect_varying`
  skips its plain-rect path unless all four radii are under 0.1, and the negative corner makes the
  path overshoot and double back, landing the page on the border's last pixel. Symptom: a border
  that stops one pixel short of the square corner, with the content showing through, which reads as
  a faint rounding and only against a contrasting child (a hero, the Now Playing backdrop). Bit the
  content panel under the native titlebar, rounded on the left and oversized past a square wrapper
  to meet the OS frame flush on the right. **Safe shapes are one radius on every corner, or zero on
  all four** (a scissor with no layer). Where an edge has to look square, give the element a gutter
  rather than clipping a square corner.

- **An explicit `background: transparent` costs a discarded path per element per frame.**
  `resolve_native_classes` picks an element's native class from *which properties have bindings*,
  regardless of value, so binding `background` at all promotes it out of `Empty` (never visited by
  the renderer) into `Rectangle` — and the FemtoVG item renderer builds the path *before* it looks
  at the paint. `Rectangle` already defaults to transparent, so the binding is pure cost; delete
  it rather than spelling out the default. Only matters at scale, a repeater's worth of them per
  frame; noise on a one-off container.
  **A fill that is only sometimes transparent pays too, and a rounded one pays twice**:
  `draw_border_rectangle` builds its fill path and its border path before it checks either paint,
  so a per-row fill painted only on hover or selection belongs behind an `if`, not a ternary ending
  in `transparent` (the track row's pill). Where something eases it, gate on the eased float rather
  than its target (`if ring-t > 0`, the mosaic picker's selection ring), so the branch outlives the
  fade-out. That predicate is animated, so the branch may carry no layout-watching `changed`
  tracker (the entry on a `changed` handler inside an `if`).

- **A gradient stop inset from the extents drops FemtoVG's two-stop path and costs a slab of the
  GPU driver's buffer pool.** `femtovg`'s `GradientColors::from_stops` answers the cheap `TwoStop`
  variant, filled from shader uniforms with no texture at all, only when there are exactly two
  stops **and** the near one sits at or before `0` with the far one at or past `1`. Everything
  else falls through to `MultiStop`, which synthesizes a 256×1 gradient texture per distinct stop
  set through `GradientStore` and allocates it out of the driver's pool. A *two*-stop gradient
  whose far stop is inset is the case to watch, reading as the cheap shape at a glance.
  **Source-level reasoning predicts the trigger and not the magnitude, which is the whole trap.**
  The texture is a kilobyte and `GradientStore` migrates it across frames rather than
  reallocating, so the cost reads as nothing until it is measured on the process, where the driver
  turns out to back it with a whole slab that is never handed back. It surfaces only while
  something animates over it: a static frame re-submits nothing, so a hero that merely *sits*
  there costs the same either way and the bill arrives when a visualizer or a morph starts
  repainting the window.
  The aurora's three washes ended at `70.71%` and were the tree's only brushes on that path;
  moving the stop to `100%` retired the slab. `aurora-backdrop.slint` argues why the stops sit at
  the extents and `ui::aurora_backdrop_tests` pins them, an inset stop looking like nothing on
  screen and only a measurement or that test catching one coming back.

- **A drop shadow that changes over an animation leaves a texture per frame resident.** Since
  1.18 the renderer keeps shadows across frames in a `BoxShadowCache`
  (`i-slint-core/graphics/boxshadowcache.rs`) keyed by size, colour, blur, radius and offset, up to
  sixteen, each a texture plus the stencil buffer it was drawn with; 1.16 rebuilt it every frame.
  So easing any `drop-shadow-*`, or easing the size of the element casting it, fills all sixteen
  slots, and they stay after the animation ends. A colour-only ease is no escape, the colour being
  baked into the texture. A shadow has to be switched rather than eased and cast from an element
  whose size holds still, which is why the search bar and the labeled input, the two that eased
  theirs on focus, dropped them instead. Nothing looks wrong on screen; only the process's GPU
  memory shows it.

- **Generated `Path` geometry has to arrive as an SVG `commands` string, and needs an explicit
  viewbox with `fit: fill`.** `for`-`in` inside a `Path` is rejected outright (slint-ui/slint#754),
  so a computed vertex count means building the string in Rust. Then **declare
  the viewbox**: without one Slint fits the path's *own* bounding box to the element,
  renormalising every frame — a whisper draws as loud as a chorus. And `fit` must be `fill`, not
  the default `contain`, which preserves aspect ratio and letterboxes a tall narrow box into a
  sliver. Finally, **wind every closed figure counter-clockwise on screen** (a waveform:
  lower-edge-first) so its signed area is positive. `draw_path` marks a negative one
  `Solidity::Hole`, which femtovg enforces by reversing the winding, and a concave fill draws its
  antialiased fringe only outside the fill, so a hole's edges come out stepped with nothing else
  wrong. The sum skips the closing segment, so where the figure starts matters too: close along a
  vertical edge or one at `y = 0`. All four bit `waveform-trace.slint`, whose writer is
  `player::waveform::write_path_commands`; the winding bit `rounded-frame.slint` too.

- **A number turned into a string inside `.slint` writes the running translation's decimal
  separator, so a `commands` string built with `+` breaks in every bundled locale but English.**
  `shared_string_from_number` (`i-slint-core/string.rs`) swaps the `.` for the separator
  `select_bundled_translation` installs, and every non-English bundle sets `,`. `"A " + 5.5`
  becomes `A 5,5`, the parser rejects it with a `Flag` error on stderr, and the path draws nothing:
  the content panel's corner masks, the toast's accent bar and the play button's hold ring all
  went missing that way. An integer has no `.` to swap, so a path off whole-pixel metrics passes
  until a half-stroke inset or a `sin` lands a fraction, and English, the locale every review runs
  in, never shows it. Cure: every number in a string something parses goes through
  `.to-string-unlocalized()` (`rounded-frame.slint` names its three once as string twins,
  `notification-stack.slint` and `play-button.slint` convert inline). Text a person reads keeps the
  bare `+`, a localized decimal being right there, and a string built in Rust is unaffected,
  `format!` knowing nothing of the bundle. Tell, in `app-window.rs`: a
  `shared_string_from_number (` inside a `PathData :: Commands` binding.

- **A round cap on a 1 px stroke draws nothing, and the stroke's two ends come out different.**
  femtovg's `expand_stroke` (`path/cache.rs`) tessellates a cap with `curve_divisions` over the
  half-width, which for a line one physical pixel wide lands on its floor of two, so
  `round_cap_start`/`round_cap_end` emit a semicircle of two rim points and no area. The stroke
  stops on a hard edge at its endpoint, and where that edge runs through pixel centres, as it does
  at the tip of a 45° line on integer coordinates, the rasterizer's tie-break keeps those pixels on
  one side and drops them on the other: every left-hand tip fades out and every right-hand one is
  cut off, whichever end the path starts from. **Only at a scale factor of 1.** Slint hands the
  canvas a `ceil`ed scale as its dpi, which narrows the tessellation tolerance and doubles the
  physical width together, so the same logical stroke gets a real cap on any HiDPI display and the
  review there shows nothing. Cure: `LineCap.square`, whose `butt_cap_*` lays a one-pixel fringe
  past the endpoint, so geometry lies on both sides of the edge and both tips fade alike. Bit
  `caption-buttons.slint`'s KDE glyphs, copied from a decoration that draws round caps.

- **A `<=>` on a `Flickable`'s `content-y` silently disables Slint's own out-of-bounds correction
  — a one-way binding doesn't.** `Flickable::init` installs a change handler that pulls a
  scrolled-past-the-end viewport back in range, guarded on
  `if *y_out_of_bounds && !y.has_binding()` (`items/flickable.rs:118`), so a `content-y` carrying
  a binding opts out. **The distinction is which kind survives a write.** Most scrollers here
  reach `content-y` imperatively (`sv.content-y = -o`), and 1.18's generated tree seeds them with
  no binding at all, so they keep the guard. `<=>` compiles to `Property::link_two_way`, which
  **does** survive a self-write, making the two composite views (`browse-view.slint`,
  `ArtistDetailBody`) the only permanently unguarded scrollers in the tree.
  Left uncorrected, content that shrinks under the current offset paints a **blank region** —
  Browse leaving a deep folder for the much shorter library root, or either view filtering a long
  list to a few rows. Nothing looks wrong in the source; the symptom is an empty view that fixes
  itself when you switch tabs, the content-area `if` remounting it at 0. `CompositeScrollbars`
  re-clamps on `changed v-outer-max`; a new scroller that two-way-binds `content-y` needs the
  same. To audit, grep the generated file for `r#Flickable :: FIELD_OFFSETS . r#content_y ())`
  (empty parens included): 2 `link_two_way` hits, every other one a read, a write, or the pin a
  `ListView`'s repeater is handed.

- **Slint's in-window drag goes round a list, never in a row, and a `DropArea` drawn over another
  takes its drops.** Each half builds, passes a quick drag, and is wrong.
  - **A `DragArea` in a row never hears its drag end.** The runtime holds the source weakly and a
    `ListView` destroys rows rather than recycling them, so a row scrolled out, rebuilt under the
    drag or moved by the reorder itself misses `drag-finished`. `components/drag-source.slint` is
    the one spelling.
  - **It drags from any left press it contains**, a row's own button and the gap between rows
    included, so what a drag carries can't be "the row last pressed" without every press clearing
    it first. A touch is a left press to it as well, so a clear that watches only `MouseInput`
    misses one. `globals/drag.slint` argues the arming.
  - **A `Drop` goes to whatever the hit test finds, not to the area that accepted the hover**, and
    a `DropArea` takes any `Drop` it receives (`i-slint-core/window.rs`, `items/drag_n_drop.rs`).
    So one drawn in front of a target eats that target's drops even while it answers `none` to
    every move, which passes the moves on and never a drop. Behind the targets, a window-sized one
    sees exactly the moves none of them took.

- **Stock Slint 1.18 + winit 0.30 have zero OS file-drop on Wayland, and no pointer during one
  anywhere.** `DragArea`/`DropArea` is in-process only, with no `PathBuf`, so no OS drag reaches a
  `DropArea`, and no hover moves under one: a file target can't tell it is being hovered. The
  vendored fork reports the pointer and Rust routes the drop against rects the targets publish
  (`desktop-shell.md`). Fix merged upstream but unreleased: winit **#4571**, a new `DataTransfer`
  DnD API superseding #1881/#4009 and the positioned events the fork copies from its betas.

- **`changed <local-prop>` doesn't fire when first layout settles directly on final value.**
  Native-titlebar reaches final grid width in one pass — derived counts never *transition*. Fix:
  pair the handler with a single-shot 1 ms `Timer` firing the body once at mount. **The
  `changed width => { self.<mirror> = self.width; }` idiom is the recurring victim and always owes
  that seed**: a window opened at its final size never *resizes*, so the mirror keeps its declared
  seed forever and every width derived from it is wrong until something moves the window. Bit
  `settings-view.slint`'s `view-w` and `now-playing-bar.slint`'s `bar-w` (the latter hidden by
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
  (`grid-columns-sync.slint`'s `changed watched-cols`, a count off a width). So the rule is about
  the **condition**, not the
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
  `change_tracker<N>.init` whose *eval* closure reads a `_width` / `_height` / `_watched_cols`.
  The shape to copy is `library-tab-band.slint`: everything tracker-free rides one
  `hero-shown: root.detail-open || root.hero-t > 0` (which also stops the two halves mounting a
  frame apart), and a piece carrying a layout-watching tracker stays mounted for the life of the
  band, fading by brush alpha — brush alpha rather than `opacity` because a permanent mount
  satisfies neither `need_layer` bail. Pinned by
  `ui::library_tab_band_tests::the_hero_mounts_on_the_detail_or_a_running_morph`.

- **A `Timer`'s `interval` may not reach a layout property, even transitively.** Slint evaluates it
  through a `ChangeTracker` initialised in the component's `user_init`, and for a timer inside an
  `if` that runs from `Conditional::ensure_updated` — *inside the parent's layout pass*. So a
  cadence gated on anything that reads a scroller's height re-enters the layout cache being
  computed and panics `Recursion detected` (`properties.rs::access`) on the frame the branch
  mounts. The same expression is fine in any ordinary binding, and the backtrace names layout
  closures either side of the property you wrote, so it reads as a layout bug. Cure: keep the
  interval's inputs to plain properties and let the timer's **handler** compute the
  layout-dependent half into one of them — `lyrics-panel.slint` decides `settled` (has the glide
  reached the sung line) at the end of each tick, where reading `target-y` is free, the handler
  running nowhere near the pass.

- **`init` runs *after* bindings resolve to final values — useless for *flipping* an entry
  animation.** Setting `shown: true` in `init` makes `true` the *initial* value, so
  `animate opacity` never runs. Fix: single-shot 1 ms `Timer` flips `shown` once at mount. **On
  1.18 the Timer is half of it**: an `if` branch is instantiated eagerly, so the Timer can fire
  before any animated binding has been evaluated, and one first evaluated after the flip has
  nothing to ease from. `init` reads each animated property once, so it starts from its
  `shown: false` value (`components/view-transition.slint`). **The layers' own `init` is enough
  even where they sit in an `if` nested below the Timer**: `Conditional::ensure_updated` runs a new
  branch's `init` and builds its nested `if`s in the same call, before the event loop can fire the
  Timer. Bit the miniplayer backdrop on some presses and not others, until `HeroBlurBackdrop` and
  `AuroraBackdrop` read theirs. Pinned by `ui::mount_fade_tests`, the bare reads looking like dead
  statements.

- **Glyphs outside Vazirmatn inflate `Text` line-box and break patched-metrics centring.** Unicode
  arrows or any glyph the bundled font lacks pulls a fallback font; its taller `typoAsc/Desc`
  defines the line-box, so patched glyphs drift down. Fix: render the foreign glyph as a sibling
  `MaterialIcon` (collapsed em-box).

- **Slint 1.18 draws a Hangul syllable with no final consonant as two loose jamo, and the trigger
  is the generic family it appends to every font stack.** `textlayout/sharedparley/shaping.rs`'s
  `ranged_builder` pushes `[<default-font-family>, Generic(SansSerif), Generic(SystemUi)]`, and
  fontique takes each generic as fontconfig's whole `sans-serif` chain. parley counts a font as
  covering a syllable when it covers the *decomposed* form and stops at the first that does, so a
  face holding the jamo and not the syllables wins: Droid Sans Fallback, on Fedora. The same line
  through *any single* named family shapes correctly, Vazirmatn included, which covers no Hangul at
  all, so there is nothing to reach from a `.slint` file. A syllable with a final consonant splits
  into a smaller syllable that face lacks and falls through to a CJK face, which is why half a
  Korean line looks blocky and half looks spaced out. The exception is the 27 built on `가`, which
  that face does carry: they split into three glyphs at 3 em, and the estimate charges them one.
  **The cost is width, not only looks.** Swept across the whole syllables block, an open syllable
  measures 2 em against a closed one's 0.92, so a Korean line lays out up to 1.6× wider than its
  syllable count says. Anything sizing a box from a character count under-measures it, and an
  elided `Text` **drops** the line that overflows its box rather than clipping it, so words go
  missing rather than looking cramped. `ui::now_playing::lyrics`'s `OPEN_HANGUL_EMS` is the one
  place that pays for it; delete it when a Slint release composes the block, and not before.
