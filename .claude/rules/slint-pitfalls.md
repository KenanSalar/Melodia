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
this file is what builds, looks right, and is wrong. Source paths are Slint 1.18's. The generated
`app-window.rs` (`target/*/build/melodia-ui-*/out/`) is what a construct compiled to: verify
there, never in the `.slint`.

## Layout

- **`visible: false` doesn't remove from layout**: the hidden child still claims stretch
  (slint#7377). Use `if !collapsed: …`.

- **A box collapsed with `clip: true` also cuts what its contents paint past it while open**: a
  focus swell, a shadow, a tooltip. Collapse through `visible`, which `passes/visible.rs` lowers to
  a `Clip` of `!visible` after layout lowering, so the element's own width still collapses it.

- **`parent` isn't reachable from a component's root binding.** Take the host metric as an
  `in property`.

- **An element passed into `@children` is drawn in the slot but resolves names where it was
  written**, so `parent` there is the mount, not the slot's container, and nothing warns. The
  container publishes its own metric (`EntityCard.tile-size`) and the host frames against that
  (`entity_card_overlay.rs`, which reads the mount's braces only).

- **`height: 100%` on a layout child under a fixed-height parent is unbounded**: the row swallows
  the body and a sibling `ListView` renders nothing. Pin fixed rows with `min-height` +
  `max-height` + `vertical-stretch: 0`; never `width`/`height: 100%` on a layout child.

- **A bound `width` on a component root is reported as both its `min` and its `max`**, and a
  `min-width` beside it is rejected. Animated, the window's floor rides it: an inward drag stutters
  and a breakpoint fires late or never. Spell `min-width` (constant) / `preferred-width` (animated)
  / `max-width`. Tell: `layout_info`'s `min` reading the root's own `width`. **An animated
  `height` takes the same split and owes a `clip`**, the element drawing shorter than its contents.

- **A `Rectangle`-rooted component folds only unrepeated, unpositioned children into its layout
  info.** `default_geometry::gen_layout_info_prop` skips a repeated child, so
  `Rectangle { if x: Card {} }` reports 0×0 (a `min-height` beside it hides that until the content
  wraps): wrap in a layout, also the one thing outside the `if` whose height the root can read. It
  skips a child binding `x` or `y` too, where the same content in a centring layout folds its whole
  constraint set, animations included, into every host's layout. The fold runs on both axes and
  `layout_items` never shrinks a cell past its `min`, so a wrapper's `min-width` floors its host; an
  explicit `min-width` **replaces** the merged one. Tell: a missing `+ …layoutinfo_v` term, or a
  `+ …layoutinfo_h` term reaching an animated property.

- **A box layout clamps a stretched child to that child's own cross-axis `min`/`max`, and the std
  `ScrollView` (so `ListView`) floors itself at 50 px.** `clamp_cross_stretch_size`
  (`passes/lower_layout.rs`), so a std scroller overflows a narrower column, along with everything
  sized off its `content-width`. Set `min-width: 0px` at the mount. Tell: `.max (50f64`.

- **Centring one cell of a box layout moves the whole layout onto another solve.**
  `cross-axis-alignment: center`, or `cross-axis-self-alignment: center` on a cell, goes through
  `solve_box_layout_ortho` (`i-slint-core/layout.rs`): a stretched sibling is clamped to its
  implicit max (self-align only where every one is unbounded: a `Text`, a `Rectangle` of `if`s), an
  explicit `x`/`y` on a cell is a build error, and dropping an `alignment: start` wrapper can cap
  the row, whose cross max is its cells' least. A lone `Text` centres with `vertical-alignment` and
  a sized child of a plain `Rectangle` is already centred. Tell: `solve_box_layout_ortho`.

- **`VerticalLayout` divides surplus equally when every child has `vertical-stretch: 0`.** Append a
  `Rectangle { vertical-stretch: 1; }`, or `alignment: start`, which hands out none, a stretching
  child included. In a `GridLayout` one is mandatory, a row's shorter cell stretching to the taller.

- **A `GridLayout` can't do masonry**, cells top-aligning to the row's tallest. Make each cell a
  `VerticalLayout` column of contiguous halves, and give an item that must not fill an empty
  column a `max-width`. A flex can't stand in (Flexbox).

- **`GridLayout` reads a runtime `row`/`col` every layout pass**, so a grid can re-flow between
  side-by-side and stacked with every item mounted once. **Auto-numbering and a runtime expression
  may not mix for one property** (`Cannot mix auto-numbering and runtime expressions`,
  `passes/lower_layout.rs`): once one child spells `row`, every child does, a literal counting. A
  `for`'s items can place themselves, `add_element` following a repeated child's binding into its
  sub-component, where a mix is only a warning.

- **A `GridLayout` column starts at its cells' `preferred` width** (`layout_items` sizes to `pref`,
  then shares surplus by stretch), so equal stretch on unequal cards gives unequal columns. Pin
  every cell's `preferred-width` to one number, leaving `min-width` alone; never `width`.

- **A view root's bottom padding sits outside its scroller's viewport: a dead strip**, clipping the
  last row mid-glyph. Inset left, right and top only, with end clearance inside the viewport
  (`ui-patterns.md`'s track-list insets).

## Flexbox

- **A `FlexboxLayout` measures its items at a width it hasn't committed to, and only a layout- or
  `Text`-rooted item can answer.** taffy asks a height at a trial width through
  `layoutinfo-v-with-constraint`, synthesized on layout elements only (`passes/lower_layout.rs`;
  `is_height_for_width_cell`, `llr/lower_layout_expression.rs`). A `Rectangle`-rooted item whose
  height follows its width is read at its drawn height, which waits on the solve: `Recursion
  detected` (`i-slint-core/properties.rs`) at run time on the first layout, the build and tests
  green. A `Text` reads the trial width as its `cross_axis_constraint` (`items/text.rs`). So a flex
  holds fixed-height, layout- or `Text`-rooted items, a body of cards stays box layouts, and a root
  whose `height` is its own flex's `preferred-height` can't sit in a flex either.

- **A flex reports no maximum height and stretches its lines into what it is given**
  (`cross-axis-line-alignment: stretch`). Bind the root's `height` to its `preferred-height`, set
  `cross-axis-line-alignment: start`, or hand it no surplus. **Its `preferred-width` is a
  near-square block** (`flexbox_layout_info_main_axis`), so a strip given only that wraps early.

- **A flex inside an `if` reports its height at that near-square width, not the drawn one**, the
  repeated `layout_info(Vertical)` handing `fn_layoutinfo_v_with_constraint` its own
  `layoutinfo_h.preferred`. A box solve still places it (`layout_item_info_at_cross_width`); what
  breaks is anything reading the reported height. Make the flex a direct child, hidden through
  `visible`. Tell: the repeated arm ending on `…_layoutinfo_h ()` … `.preferred`, where a direct
  child's `flexbox_layout_info_cross_axis` ends on the root's `…_width ()` (`hero_chips_tests.rs`).

- **Any component behind an `if` or `for` measures its height at its child layout's width, and a
  root pin can't steer it**: a root's `preferred-width`/`max-width` reaches only the horizontal arm
  (`get_layout_info_v_constrained_for_repeated`), so wrapped text reports one line. Pin a **cell**,
  whose `preferred-width` replaces what it asks where a root's only raises it (`LayoutInfo::merge`).
  Tell: the `Vertical` arm reading `(… + …_layoutinfo_h ()).preferred`.

## Bindings and animation

- **A component writing its own `in-out property` detaches a one-way `name: source` binding.** Use
  `<=>`, or a `manual: true` that only fires the callback (`ToggleSwitch`, `Dropdown`).

- **`changed` takes no path on a global**: mirror it,
  `property <int> watched-nav-idx: Nav.selected-index; changed watched-nav-idx => {}`.

- **A property both toggled and dragged may not ease the drag**, every micro-update taking the full
  curve. Gate it (`enabled: !Nav.sidebar-dragging`); a disabled animation lands at once.

- **An animated property with a binding restarts whenever a dependency is marked dirty, not when
  its value changes.** `AnimatedBindingCallable::mark_dirty`
  (`i-slint-core/properties/properties_animations.rs`) checks only `original_binding.dirty` and
  re-bases on the current frame, and dirt is structural, so a bool held `true` through a resize drag
  restarts on every width it reads and converges at the input rate. Cure: write the property
  (`Property::set_animated_value` replaces the binding), keeping the expression as the seed so
  mount lands in `NotAnimating`. **`states`/`transitions` half-fix it**:
  `StateInfoBinding::evaluate` compares values, so `change_time` survives while `from_value` is
  still re-based. That half cures a discrete late target, a transition ending at
  `change_time + duration` where `animate` restarts.

- **An animated property can't land on the frame that dirtied it, so it may not pair with a snapped
  sibling off the same flag.** `mark_dirty` only sets `ShouldStart` and the first `evaluate` runs on
  the next render, so the curve opens on the frame its partner finished. Snap both
  (`card_selection.rs`).

- **A shared component may not `animate` a brush its host hands it**: a host easing that brush
  re-dirties the leaf every frame, which then lands in one late rush, both ways. A
  `touch.has-hover ? fill : transparent` is no escape, the clicked tab being the hovered one. Ease a
  float and paint `fill.transparentize(1.0 - t)`, which multiplies alpha where `with-alpha` sets
  it. `mix()` has no brush overload (`ColorMix: (Color, Color, Float32) -> Color`), so crossing two
  host brushes is two identical layers with the bottom at full alpha, `t` over `1 - t` thinning to
  three-quarter coverage (`tab_bar_tests.rs`). **The mirror image bites the host**: a layer may not
  ease out of a value nothing was painting, so a held tier's idle value has to be honest
  (`detail-open ? <tier> : <idle token>`).

- **`Theme`'s palette brushes are written every frame of a Material You fade, so an `animate` that
  can read one carries `enabled: !Theme.recoloring`.** Slint can't animate a global, so
  `ui::appearance::theme_paint` writes a blend per frame: the entry above, window-wide. "Can read"
  includes a defaulted input, a brush a leaf eases defaulting to a `Theme` token. **No gate**: a
  float, a tier Rust writes once a solve (`np-*`, `HeroBackdrop`), a `states` transition. **One
  binding switching kinds gates on the switch** (`root.on-backdrop || !Theme.recoloring`), a host
  handing a leaf a tier setting its `on-backdrop`. A miss shows only mid-fade (`palette_fade.rs`).

- **Animating a binding derived from another animating property phase-lags.** Animate the source.

- **Concurrent `animate` blocks are each re-evaluated every frame.** A periodic visual takes one
  shared `Timer` and counter with derived bindings.

- **A `for` over a `pure callback`'s model rebuilds every instance whenever an input is dirtied**:
  `Repeater::model` (`i-slint-core/model/repeater.rs`) compares `ModelRc`s by pointer. Return the
  same `ModelRc` for the same answer, or keep the model flat and let a layout do the arithmetic.

## Scrolling

- **Nested `ScrollView` + `ListView` need `content-height: self.height` on the outer**
  (`content-width: self.width` the other way round).

- **A nested `ListView` swallows the wheel at its scroll edge** (`flickable.rs::process_wheel_event`
  returns `EventAccepted` in the capture window), and no pure-Slint interceptor works. Reach for
  tabs first. The composite views route the wheel through winit (`CompositeScroll`), and **every
  content-view switch owes `CompositeScroll.reset()`**, Slint giving no unmount hook. **A Rust write
  watched only by a `changed` handler owes `request_redraw()`**: handlers run from `new_events`,
  which `WindowRedrawTracker` schedules off what the render read (`winit_filter_tests.rs`).

- **A `Flickable`'s drag-pan steals a mouse drag, and the default is the style's.** A bare
  `Flickable` defaults `mouse-drag-pan-enabled` to `true`; every style's `ScrollView` (so
  `ListView`) pins it `false`, and with no style set `typeloader.rs` picks fluent. `interactive`
  alone decides a touch. Where the pan is on, `handle_mouse_grab` (`input.rs`) intercepts a press
  that moves `DISTANCE_THRESHOLD` (8 px) within `DURATION_THRESHOLD` (500 ms), sending what is
  below a `Cancel`: a `DragSource`'s 8 px, so the two compete intermittently. A click-to-act or
  draggable list pins `false` (`playlists_tests.rs`, `scrollbars.rs`). **`!reorder-enabled` is the
  trap**, enabling the pan on every sort that retires the drag. Verify in the generated file.

- **A `<=>` on a `Flickable`'s `content-y` disables Slint's out-of-bounds correction**, which runs
  only when `!y.has_binding()` (`items/flickable.rs`). An imperative write leaves no binding, but
  `Property::link_two_way` survives one, so content shrinking under the offset paints blank. A
  two-way scroller re-clamps itself (`CompositeScrollbars`). Audit:
  `r#Flickable :: FIELD_OFFSETS . r#content_y ())`, every `link_two_way` hit unguarded.

## Pointer and hover

- **A grabbing item that unmounts mid-gesture hands its release to whatever is under the pointer as
  a fresh event**: `handle_mouse_grab` finds the grabber gone and `process_mouse_input` hit-tests
  the event again from the root, so any `TouchArea` there reading `PointerEventKind.up` sees a
  release for a press it never had. Hover-mounted controls that rewrite their host are the shape,
  and so is a notification card dropped mid-drag. Act on `clicked`, gated on the item's own
  `pressed` (`items/input_items.rs`); it carries no modifiers, so snapshot them on `Down`, and runs
  before `pointer-event(Up)`. A drag that commits on release wherever it ends clears its own flag
  there (`SliderPointer`).

- **`has-hover` isn't continuous, so a translucent fill may not read it raw.** `input_items.rs`
  drops it on a wheel event the `TouchArea` doesn't take, and `send_exit_events` compares stack
  index rather than membership. Each lasts a frame, hidden by an opaque fill. Arm on the raw flag,
  release on a latch (`pointer-inside || hover-held || has-focus`, a short `Timer` lowering
  `hover-held`), the raw flag kept since `changed` skips a first evaluation. **Where nothing takes
  the wheel, the flag stays down until the pointer moves**, so a control or sentinel that must keep
  its hover answers `accept` to every wheel event, an empty one included.

- **A `TouchArea` grabs on a press of any button; only `pressed` is left-only.** `mouse-x`/`y`
  follow a held right button, so a menu takes its anchor once (`show-at`), and a slider filters
  the button itself (`SliderPointer`).

- **A child with no `x`/`y` in a non-layout parent is centred**, Slint generating
  `(parent.width - self.width) / 2`. A zero-size wrapper meant as a coordinate frame pins
  `x: 0px; y: 0px` inside the component.

- **Drag handles inside a resized element need window-absolute coordinates**, the handle's frame
  moving with the panel (`EdgeResizeHandle`).

## Popups

- **`PopupWindow.y: -self.height - …` needs an explicit `width`/`height`**, `self.height` being 0
  before the first layout.

- **An open `PopupWindow` re-lays its content at once and moves a loop pass later**:
  `WindowInner::update_popup_properties` re-reads origin and box on the zero-delay timer its
  `PropertyTracker` schedules, after the frame is drawn. Keep the origin fixed (reserve the widest
  flyout, `ContextMenuMetrics`) or hold the content still while open (`Dropdown`'s `about-to-open`).
  **So a flyout lives inside its menu's one `PopupWindow`**, never a nested one (`OverflowMenu`).

- **A `PopupWindow` taller than the window is drawn past its edge**: `place_popup` shrinks it, but
  both callers in `i-slint-core/window.rs` keep only the origin. Use `PopupFit` and `PopupColumn`.

- **A press outside an open `PopupWindow` closes it and reaches nothing else**
  (`WindowInner::process_mouse_input`, for a non-`Menu` popup); the release does arrive. So a menu
  opens on the release (`ContextClick`), and a popup closed on a press hands its release onward
  (`PopupDismissCatcher`).

- **An open `PopupWindow` takes the hover and the keyboard from everything outside it.** Read its
  `is-open` beside the flag (`IconButton.force-bg`, `SingleLineInput.has-focus`).

- **`absolute-position` inside a `PopupWindow` is measured from the popup's corner**:
  `map_to_window` stops at popups (`StopAtPopups`) and no binding reaches `map_to_native_window`.
  Hence `Tooltip.in-popup`.

## Lifecycle: `changed`, `init`, `Timer`

- **`changed` doesn't fire when the first layout settles straight on the final value.** Pair the
  handler with a one-shot 1 ms `Timer` at mount; a `changed width => { self.w = self.width; }`
  mirror always owes it.

- **A `changed` handler inside an `if` outlives its branch, and whether that panics depends on what
  it watches.** The generated `ChangeTracker` holds a `VWeakMapped` to its repeated component, and
  `ChangeTracker::evaluate` opens with `self_weak.upgrade().unwrap()`; `VRc` keeps the allocation
  for the weaks, so the tracker stays in `CHANGED_NODES`. One on a property driven inside the branch
  dies quietly; one on a layout property is re-dirtied by the parent re-flowing without it. So a
  branch with a layout-watching tracker drops only on a frame nothing re-dirtied that property:
  never on an animated predicate, nor on anything a `changed` handler writes, nor safely from a
  `Timer` or Rust callback. Symptom: `called Option::unwrap() on a None value` in
  `InnerFoo::user_init::{closure#N}`, naming a component far from the edit. Tell: a
  `change_tracker<N>.init` whose eval reads a `_width`, `_height` or `_watched_cols`. Keep the
  tracker mounted and fade by brush alpha (`library_tab_band_tests.rs`).

- **A `Timer`'s `interval` may not reach a layout property, even transitively.** It is a
  `ChangeTracker` that for an `if` initializes in `Conditional::ensure_updated`, inside the parent's
  layout pass: `Recursion detected` (`properties.rs::access`) on the mounting frame. Keep its inputs
  plain and let the handler compute the layout-dependent half.

- **`init` runs after bindings resolve, so `shown: true` set there is the initial value and nothing
  eases.** Flip it from a one-shot 1 ms `Timer`, and read each animated property once in `init`: a
  branch is built eagerly, so the Timer can fire before an animated binding is first evaluated, and
  one evaluated after the flip has nothing to ease from. `Conditional::ensure_updated` runs `init`
  and builds nested `if`s in one call, so this covers layers below the Timer
  (`mount_fade_tests.rs`).

## Rendering cost

- **A rounded `clip: true` renders everything under it into a texture, and a clip on a rounded
  element is always rounded.** `passes/clip.rs` copies the radius, and `visit_clip` sends any
  non-zero one through `render_layer`: a texture at physical size, redrawn when anything inside
  changes and filled back through an antialiased path. A radius-less clip is a scissor, and a
  `border` never makes a layer (`draw_border_rectangle`). The texture softens text on a fractional
  pixel and re-renders a view on every scroll frame. Clip a radius-less element with the rounded
  fill a child at `x: 0; y: 0`, or mask corners with `RoundedFrame` over an opaque ground.

- **FemtoVG antialiases every rounded fill, stroke and clip on its own, and the coverages of one
  curve stack**, so a corner drawn twice reads harder than its radius: a `background` under a
  rounded clip filled to the edge, a border over a separately rounded fill, a stroke ending on a
  clip's edge. Cures: the fill a square child inside the clip; fill and border on one element,
  tucked under the stroke; `EdgeOutline` for a stroke inside a clip. **The tuck holds only under an
  opaque border** (`draw_border_rectangle`): a translucent one takes a `TuckedFill`, except over a
  drop shadow, which shows through a tucked rim.

- **A `background: transparent` written out costs a discarded path per element per frame.**
  `resolve_native_classes` picks the native class by which properties are bound, so the element
  leaves `Empty`, and a plain `Rectangle`'s `draw_rectangle` builds its path before reading the
  paint. Delete it. **A plain fill only sometimes transparent pays the same**: put it behind an
  `if` on the eased float rather than its target, so it outlives the fade-out, carrying no
  layout-watching tracker. A rounded or bordered fill doesn't, `draw_border_rectangle` resolving
  its paints first.

- **A gradient stop inset from the extents drops FemtoVG off its two-stop path, at the cost of a
  slab of the GPU driver's pool.** `GradientColors::from_stops` answers `TwoStop` (uniforms, no
  texture) only for exactly two stops at or past `0` and `1`; else `MultiStop` builds a 256×1
  texture per stop set (`GradientStore`). Source predicts the trigger, not the size: the driver
  backs that kilobyte with a slab it keeps, billed while something animates over it
  (`aurora_backdrop_tests.rs`).

- **A drop shadow that changes over an animation leaves a texture per frame resident.**
  `BoxShadowCache` (`i-slint-core/graphics/boxshadowcache.rs`) keeps sixteen keyed by size, colour,
  blur, radius and offset after the animation ends. Switch a shadow rather than easing it or its
  caster's size; only GPU memory shows the miss.

- **A flash-free image cross-fade is two slots, never cleared under a fade.** Rust writes the
  inactive slot and flips a `use-a` bool; a slot is cleared only where no fade can still read it.

## Text

- **Nothing that draws text may be cut to its layout box, which is a line box, not the ink.**
  Vazirmatn is patched to a ~1.05 em line box (`1650/-500`, `USE_TYPO_METRICS`) while its outlines
  reach `yMax 2163` / `yMin -1160`, so a crop removes Arabic marks no Latin review sees:
  - **`opacity` below 1 renders the subtree into a texture sized to child geometry.**
    `Opacity::need_layer` (`i-slint-core/items.rs`) bails at 1.0 and on an element with no
    children; else `item_children_bounding_rect` sizes it, a `Text`'s `bounding_rect` being its
    line box. A fade crops while it runs, a dimmed state for as long as it lasts. Fold the alpha
    into the brush (`with-alpha`), pass a `fade` float where there is none (`ArtworkImage`), or,
    where the brushes feed the component's own `animate`s, leave the faded element childless
    (`IconButton`). Verify by names in the enclosing struct's item fields, a sub-component having
    no `ITEM_TREE`. `view-transition.slint` has no cure.
  - **A `Text`'s default `overflow: clip` pushes a scissor at its box**
    (`textlayout/sharedparley.rs` → `combine_clip`). `overflow: elide` pushes one only where the
    first line overflows the box's height (`first_line_exceeds_height`), so user text wants elide,
    with slack in any fixed height. Elide also lowers the layout `min` to one `…`, so an
    over-packed row compresses and truncates.

- **A faked placeholder `Text` in a non-layout parent paints past its field, `overflow: elide`
  alone not stopping it.** `make_default_implicit` sizes it to `max(preferred, min)` of its
  untruncated `layout_info`, and `text_layout_info` lowers only `min`. Bound the width and elide; a
  locale bug in practice (`placeholder_tests.rs`).

- **A `Text`'s horizontal layout info is intrinsic to its string**, never read back from the width
  it was handed, so sizing a slot off it is a derivation, not a cycle.

- **A glyph the bundled faces lack pulls a fallback face whose taller metrics set the line box**,
  shifting the patched glyphs off-centre. Draw it as a sibling `MaterialIcon`.

- **Material Symbols need a collapsed line box**, `Text` defaulting to ~1.2× `font-size`: a fixed
  square (`MaterialIcon`).

- **Slint 1.18 draws a Hangul syllable with no final consonant as two loose jamo**, through the
  generic family appended to every font stack: `textlayout/sharedparley/shaping.rs`'s
  `ranged_builder` pushes `Generic(SansSerif)` and `Generic(SystemUi)` after the default family,
  fontique takes a generic as fontconfig's whole chain, and parley stops at the first face covering
  the decomposed form, a jamo-only one. Any single named family shapes correctly, so `.slint` can't
  reach it. An open syllable measures 2 em, so a box sized off a character count under-measures and
  an elided `Text` drops the line: `OPEN_HANGUL_EMS` (`ui/now_playing/lyrics/measure.rs`) pays.

## Paths and gradients

- **A function's arguments are unreachable inside a gradient's stops**, so a shared
  `tile-gradient(a, b)` is unspellable: `from_at_gradient` sets `ctx.property_type` to
  `Type::Color` per stop, and `ArgumentsLookup` then yields nothing. Tell: `Unknown unqualified
  identifier 'a'`. `root.a` resolves, so the escape is a mounted component with an
  `out property <brush>`; Rust can't build one, `slint` re-exporting no `LinearGradientBrush`.

- **Generated `Path` geometry crosses as an SVG `commands` string, with an explicit viewbox and
  `fit: fill`.** `for` inside a `Path` is rejected (slint-ui/slint#754); without a viewbox Slint
  fits the path's own bounds every frame, and `contain` letterboxes. **Wind closed figures
  counter-clockwise on screen**: `draw_path` marks a negative area `Solidity::Hole`, and a concave
  fill's fringe falls outside it, stepping the edge. The sum skips the closing segment, so close
  along a vertical edge or one at `y = 0`.

- **A number turned into a string in `.slint` writes the running translation's decimal separator,
  so a `commands` string built with `+` breaks in every bundled locale but English.**
  `shared_string_from_number` (`i-slint-core/string.rs`) swaps in the separator
  `select_bundled_translation` installs, and the path parser rejects `5,5`. Integers pass until a
  fraction lands. Use `.to-string-unlocalized()` on every number something parses; text a person
  reads keeps `+`, and Rust's `format!` is unaffected. Tell: `shared_string_from_number (` in a
  `PathData :: Commands` binding.

- **A round cap on a 1 px stroke draws nothing at scale factor 1, and its two tips differ.**
  femtovg's `expand_stroke` (`path/cache.rs`) floors `curve_divisions` at two, so the round caps
  emit no area, and the hard edge through pixel centres is kept at one tip and dropped at the other.
  HiDPI hides it. Use `LineCap.square`, whose fringe fades both tips alike.

## Drag and drop

- **Slint's in-window drag goes round a list, never in a row, and a `DropArea` drawn over another
  takes its drops** (`drag_mounts.rs`):
  - **A `DragArea` in a row never hears its drag end**, the runtime holding the source weakly and a
    `ListView` destroying rows. `DragSource` is the one spelling.
  - **It drags from any left press it contains**, a touch included, so "the row last pressed"
    needs every press to clear it (`globals/drag.slint`).
  - **It accepts every left press its children don't**, so size it to its rows where something
    behind still wants the press.
  - **A release over no accepting `DropArea` ends as an `Exit`, not a `Drop`** (`window.rs`); the
    end every drag reaches is its source's `dragging` falling.
  - **A `Drop` goes to whatever the hit test finds**, and a `DropArea` takes any it receives
    (`items/drag_n_drop.rs`), so one in front eats a target's drops while refusing every move.

- **Stock Slint 1.18 + winit 0.30 have no OS file drop on Wayland, and no pointer during one
  anywhere**: `DragArea`/`DropArea` is in-process, with no `PathBuf`. The vendored fork reports the
  pointer and Rust routes the drop (`window_chrome::file_drop`); winit #4571 is the upstream fix.

## Filter boxes

- **A filter box binds `text <=> SomeGlobal.filter` and `blur-trigger: SomeGlobal.blur-search-tick`,
  a backdrop `TouchArea` bumping the tick, and clears both on nav-away**, debouncing through
  `FilterThrottle` and matching through `Needle` (`ui-patterns.md`'s Filtering). A box that settles
  a query property (Settings) clears input and query both, or it holds text nothing filters by.
