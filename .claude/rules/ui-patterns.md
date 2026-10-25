---
paths:
  - crates/melodia-ui/ui/**/*.slint
  - crates/melodia-views/src/ui/**/*.rs
  - crates/melodia/src/boot/**/*.rs
  - crates/melodia-core/src/utils/toast.rs
  - crates/melodia-ui/build.rs
---

# UI patterns — what already exists

Reach for something here before building a second one. The things that *build, look right, and
are wrong* are `slint-pitfalls.md`, which loads on the same globs and owns every mechanism this
file points at rather than restates.

A rule rather than a `CLAUDE.md` beside the code: UI features cut across two trees (`.slint`
under `crates/melodia-ui/ui/`, Rust under `crates/melodia-views/src/ui/`), so a per-directory
file would reach one and
silently miss the other.

## Shared components

### Tooltips

- **`Tooltip`** (`components/tooltip.slint`) — an absolutely-positioned pill, not a
  `PopupWindow`, so it captures no input and the host keeps its hover; a popup would also take
  keyboard focus off `ShortcutScope` on every show. Takes `host-width`/`host-height` explicitly, a
  component root having no `parent`, and **both on every mount whatever its `side`**, since the
  pill can flip to any of them. **A variant is only half of a side**: the `x`/`y` ternaries fall
  through to the *centred* arm, so a new `side` without its own arm puts the pill on the host and
  looks deliberate.

- **`side` is a preference, and the window's edge is the component's to handle, never the
  host's.** The pill flips to the opposite side where only that one has room, and slides along the
  host's edge to stay inside the shell rect `AppWindow` mirrors into `WindowChrome.shell-*`. So no
  host picks a side to dodge the window. What a host still owns is the two things the pill can't
  see: **a clip between it and the window**, and **a sibling painted after it**. **The rect it
  fits is the client's, not the panel's**: `WindowChrome.edge-clearance` is `Theme.pad-xs` and so
  is the panel's own right gutter, so a pill wide enough to reach the clamp parks its trailing
  border on the clip line rather than inside it. Publishing the panel rect instead is not the fix,
  `globals/shell.slint` sizing the miniplayer's artwork off the same two properties. Keep a wide
  pill off the limit, which is what the alignment below does for the hosts that reach it.

- **A host at the content panel's edge aligns its pill rather than centring it**, the panel
  clipping inside the window where the slide can't see it. `align-end` pins the pill's trailing
  edge to the host's, and three things reach it. `IconButton.tooltip-align-end` is the opt-in,
  taken by both Now-Playing view toggles, where what decides is a translated label: the English
  one fits and says nothing about the six that follow. `TooltipFrame.align-end` is the same
  answer for a host whose pill is drawn on the top layer. **An unlabelled `PillButton` takes it
  on its own**, and that one is not a knob: the chrome collapses to 28 px while the label stays
  several times that, so the centred arm has nothing left to centre against, and the chips these
  sit in are right-aligned. `PillButton.icon-only` is the single statement of it, a pill whose
  label `compact-t` closed included, and **both mount shapes read that property rather than
  restating the test** (in-tree binds it directly; a top-layer host publishes it beside the rect,
  as `tab-pills`' `tip-align-end` and Browse's `view-toggle.icon-only` do). The text has its own
  single statement, `PillButton.tip-text`, which is the dropped label wherever the pill names no
  tooltip of its own, and the top-layer `tip-label`s read that.

- **A pill landing on a later-declared sibling is lifted with a constant `z`**, on the host's
  branch at the level where it and the occluder are siblings, where lifting it overlaps nothing
  that takes a press. `z` re-sorts siblings after
  `lower_layouts`, so a layout keeps its order. The miniplayer stacks three that way
  (`mini-player.slint`'s captions row over the head over the timeline, and `MiniTimeline`'s
  shuffle slot over the seek row), and `CustomTitleBar` sits over the sidebar and panel. The clip
  half is `MiniCaptionsRow`'s, which clips only while it opens or closes so an open row's captions
  can hang their pills below it.

- **Two mount shapes.** In-tree is the default. **Top-layer** is for hosts whose pill lands where
  Slint paints later (bands, header strips): `components/tooltip-frame.slint`'s `TooltipFrame`,
  declared *after* the occluder, tracking the host via `absolute-position` deltas the host spells
  itself. `sidebar-tip` is the deliberate exception and should stay one — its `x` rides the live
  rail width and it owns a `held` latch.

- **A band publishes an anchor and draws no pill.** Pinned by walks, not lists —
  `no_page_or_shell_mounts_a_bare_tooltip` over `views/` **and `layout/`**, and
  `no_shared_band_draws_its_own_tooltip` over `components/hero/`.

- **An anchor crossing a component boundary has three answers**, a frame reading only ids in its
  own file: `out` properties (the hero bands), a global (the sidebar rail, whose boundary the
  frame's file doesn't contain), and a zero-width collapse rather than an `if` (`tab-pills.slint`:
  an id inside an `if` is unreadable from outside, and a collapsed box takes no events, so the row
  is as unreachable as an unmounted branch while its ids stay readable). Collapse through
  `visible` wherever the contents draw past the box: `slint-pitfalls.md` has why a `clip` doesn't.

- **The rail's tooltip needs a hold the tab bar doesn't**, its rows sitting 4 px apart so a
  travelling pointer is over nothing for a frame or two: a `held` bool cleared by a 150 ms
  `Timer`, with `hovered: row-hovered || held`. **`held` extends and can't arm** (written from
  `changed`, which skips the first evaluation), hence `row-hovered` beside it — and
  `changed watched-mini-render` clears the stale index, the miniplayer swap destroying the rail
  with no unmount hook. A new unmount path that strands the rail owes the same clear.

- **The volume readouts anchor to a *point***, so each mounts the pill in a zero-size `Rectangle`
  with `0px` host dimensions and drives it off **`force-shown` rather than `hovered`** — a value
  readout is up on the frame the drag starts, where the reveal delay is for a label you linger
  for. **Both pass `in-popup: true`**, being the only mounts inside a `PopupWindow`, whose
  `absolute-position` would have the pill fit a window that isn't there (`slint-pitfalls.md`). A
  third mount inside a popup owes the same. **The disc both popups open from is
  `components/now-playing/volume-trigger.slint`'s `VolumeTrigger`**, which also owns the step
  readout: the `Player.volume-step-tick` countdown, bumped by a volume key or a wheel notch,
  force-shows the disc's own tooltip. It stands down while the disc's popup is open, a popup
  keeping only its slider's readout and the miniplayer's flyout leaving the disc's pill uncovered.
  **The disc and both sliders take the wheel through `Player.wheel-volume`**, on the pointer area
  that tracks their hover (`IconButton` and `SliderTrack` forward a `scroll-event` for it), so the
  hover holds through a spin. `placeholder_tests` holds each file to one percentage.

### Pills, chips, sort rows

- **`PillButton`** (`components/action-pill.slint`) — `danger: true` tints icon **and** label
  only; the filled destructive treatment is `SectionButton`'s, and the two are not one pattern.
  Compose inside `ActionPill`, separated by `PillDivider`; `IconButton` for round controls
  *outside* chips. **`compact-t` closes the label onto the glyph and moves it to the tooltip**, a
  compact tab's morph, and a text-only pill ignores it. Only the band decides when and eases it
  (`LibraryTabBand`, below), and a pill bound to it needs a tooltip something actually draws.

- **A multi-selection grows no pill.** The count, every batch action and both ends of the set are
  the right-click menu's, on rows and cards alike: "Select all" ungated wherever the host has a set,
  "Clear selection" wherever it holds one, the clicked item in it or not, while the batch entries
  still need the clicked item in the set, so a menu raised beside one acts on that item alone.
  **Clear is gated on the set rather than the item because a set can sit out of reach**: a card has
  no second activation to get back to one, and a library-sized list can hold its picks thousands of
  rows away. `card-context-menu.slint` and `track-context-menu.slint` argue it. The other exits
  are Escape (`Selection.clear-live()`), a section leave, and unpicking the last item. A surface
  that grows a "{n} selected" chip is putting a second affordance in front of the menu that
  already carries all of them. **A body click that picks fires on `clicked`**, for the
  grab-invalidation reason `slint-pitfalls.md` argues.

- **Escape reaches a selection *after* it has closed Now Playing, and a section leave drops one.**
  Both halves answer the same thing: with no pill, a set the user cannot see is indistinguishable
  from no set, and `Selection.any-live()` asks ten globals without knowing which surface is on
  screen. So the arm sits at the bottom of `shortcut-scope.slint`'s chain, where a set held by the
  page *under* Now Playing cannot swallow the press meant to close it, and every list surface
  hands its selection back on the way out. **`Tracks` and `Browse` are the two that keep their row
  model across the leave and so owe the clear on its own**, the rest emptying `selected-ids` beside
  the rows in their teardown; each of those two is guarded on there being a selection, the unstamp
  otherwise walking a model held precisely because it survives. **`CardSelection` goes back on the
  same leaves**, its scope guard answering a different question: that guard stops one grid reading
  another's set, and says nothing about a set no grid is showing. One `clear()` in each of the four
  section-gate closures covers all ten scopes, since the state is one global keyed by one scope
  string; the four My Library grid tabs share one closure, `callbacks::grid_tab`'s.
  **Radio is the one that clears without dropping its rows**:
  its leave keeps every list, but its station cards pick in `CardSelection`. A card grid is the
  surface that needs it most, having no pill and no second activation: a set left live turns every
  click into a pick. Held by `crates/melodia/tests/card_selection.rs`'s
  `every_section_leave_hands_its_card_selection_back`, an **equality** over the four because a
  floor cannot see one stop clearing; the walk reads the closure itself, opened by
  `on_section_active_changed(` or by `GridTab`'s `::bind_section_active_changed(`, so the clear
  goes in that closure and not in a function it calls. The same file holds
  the dispatcher's ten surfaces and the Escape arm's place at the bottom of the chain.

- **A card menu offers both directions only where it hasn't got the state.** An album, artist,
  genre or playlist card is a set of tracks, so the `row-is-favorite` a row menu toggles on has no
  honest answer over one half favourited, and resolving the truth costs a query per menu open per
  card. Add and Remove sit beside each other there, both well defined whatever the mixture.
  **A card that carries its own state takes the row menu's toggle instead**: one entry naming the
  direction away from the clicked card and applied to the whole set, so a selection of favorites
  is only ever offered their removal. That is a track card on a grid drawing its heart
  (`favorite-known`, which the grid ties to the same flag as the heart) and every station card.
  Showing Add over a set already favourited is the bug the split retired.

- **A menu entry is gated on what the operation needs, never on the card's kind.** The menu is the
  right-click path to what the card's hover buttons already do, so an entry it hides while the
  button beside it stays live reads as a bug on the card the user is pointing at. It shipped that
  way once: `card-is-smart` gated Rename and Edit Artwork as well as Edit Rules, where
  `playlist-grid.slint` mounts the pencil and the image button for every playlist and the detail
  band offers both too. Only Edit Rules is a smart playlist's alone. Check the hover trio and the
  detail band before adding a gate, all three being the same operation reached three ways.
  `card_selection.rs`'s `a_card_menu_entry_is_gated_on_the_operation_and_not_the_kind` holds
  `card-is-smart` to that one entry.

- **`MetaChip`/`MetaChipStrip` are decorative** — no `TouchArea`, no selected state. The
  *interactive* pill is `components/filled-chip.slint`'s `FilledChip`, under `ChipGroup`'s `Chip`,
  Radio's `FacetChip` and its `SuggestionPill`; its header says what is the shell's and what each
  host keeps. Deliberately not one component with `MetaChip`: one states a fact, the other
  configures something.

- **`QualityChip` is the third, and neither of those**: it states the signal path's verdict and
  opens the Output card, in the player bar only. It is outlined on the bar's ground so its dot is
  the only colour it carries; an accent fill reads as a toggle left on beside repeat and shuffle.
  With one mount it reads `SignalPathUi` and `Theme` directly, where a second mount on another
  ground would take its brushes as inputs, `MetaChip`'s idiom. **The dot is `GradeDot`**
  (`components/grade-dot.slint`), the one grade-to-colour mapping, shared with the Output card's
  stage rows; a verdict's Fallback falls into the red arm with a conversion.

- **`PopupSurface`** — every `PopupWindow` body wraps it. `pill: true` for vertical-pill.

- **A popup whose rows can outgrow the window places itself through `PopupFit` and lists them in a
  `PopupColumn`** (`components/popup-fit.slint`, `components/popup-column.slint`), never a
  hand-rolled scroller. `PopupFit` holds the two placement rules: raised at a point, a popup slides
  to stay in the shell; hanging off a control, it opens on the side with room so it never covers
  the control. The column scrolls whatever its own box can't hold and publishes what its host
  sizes off. Its hosts are `ContextMenuPopup`, `Dropdown`, `ColumnTogglePopup` and the bar's
  overflow menu with both its flyouts; `FacetChip` takes `PopupFit` alone, its virtualized list
  scrolling itself. `slint-pitfalls.md`'s oversized-popup entry is why.

- **A surface that floats over the app edges with `Theme.surface2`, never `Theme.border`.** All
  four Catppuccin variants define `border` as the same hex as `surface0`, so a `surface0` card
  bordered with it has no rim at all. The test is the fill underneath, not the element's kind:
  lift a surface off its background.

- **A control resting on a card fills with `Theme.card-control`, and its hover or rim with
  `card-control-hover`, never `surface1`/`surface2`**, which most palettes keep all but level with
  the card. Switches, dropdowns, outline buttons, progress bars and dialog fields take the pair by
  default, mounting only on cards. `FilledChip` and `SliderTrack` keep `surface1` defaults for
  their hosts off a card (Radio's chips, the seek row, the volume popup), so a card host passes the
  pair itself, as `ChipGroup` and `SettingSlider` do. `SearchBar` floats by default and takes the
  pair under `on-card`, which the Settings panel's box sets. Both are palette slots rather than a
  Slint mix of other brushes so each theme sets its own; `themes::toward` is the rule the generated
  palettes fill them by.

- **A sort row's Rust half is two shared helpers** — `next_sort` decides the pick (same field
  flips, a new one starts ascending), `persist_view_sort` writes it, `persisted_sort` seeds the
  pills at `wire_*` time. `next_sort` reads the direction the way `SortDir::from_token` does —
  **only `"desc"` is descending** — where the eleven hand-rolled copies tested for `"asc"` and so
  couldn't reach descending from an unrecognised token.

- **A view whose natural order is a *synthetic* field owes a third click.** `"position"`,
  `"rank"` and `"recency"` are orders no header cell asks for, so once anything else is clicked
  they are unreachable — and the pick persists. Not cosmetic where something is *gated* on that
  order: one click on Title retired Playlist Detail's drag-to-reorder for the whole install.
  **`next_sort_with_natural` is the cycle** (ascending → descending → natural) and Playlist Detail
  is the only view naming a natural order, through `TrackDetail::NATURAL_SORT`; nothing paints
  the third state, no header cell matching, which is the honest reading of "unsorted". Reach for
  it only where the order is one the user can leave and come back to; where the order *is* the
  page, `sortable: false` is the answer instead.

- **`SortPillRow`** — the `TabBar` shape (parallel `labels`/`fields` + `request-sort`), at all four
  pill rows; every other sortable surface goes through a `TrackList` column header. Pills carry
  `reserve-sort-slot` + `sort-direction` for the trailing `arrow_drop_*` slot — never concatenate
  `↑`/`↓` into the label. It exists for what it makes *unspellable*: the per-pill slot, ternary and
  `active` comparison could each go missing at one site and look right until the sort moved. What
  the *mount* still owes is the two arrays lining up — a label with no matching field sorts by the
  empty string — so both pins check lengths through `melodia_testkit::sort_mount_arrays`. `labels`
  stays an inline `[@tr("…"), …]` literal.

### Rounded edges

The one-pass-per-curve rule and what a rounded clip costs are `slint-pitfalls.md`'s. These are the
three components that answer it, and each argues its geometry at its own file.

- **`EdgeOutline`** (`components/edge-outline.slint`) is the stroke along a rounded clip's inside
  edge: the window outline, the artwork dialog's preview, and a candidate tile's hover hairline and
  selection ring. Mounted last inside the clip, at its full size. Its root has a child, so a fade
  goes through `stroke-color` rather than `opacity`, which would layer it.

- **`RoundedFrame`** (`components/rounded-frame.slint`) draws a panel's border and corners without a
  rounded clip, the corners masked in a `ground` colour over a radius-less `clip: true`. One mount,
  the content panel. **Only over an opaque ground that is exactly `ground`**: over a hero, a blur or
  anything the corners can't name in one brush it paints them visibly, and a rounded clip over a
  square fill with an `EdgeOutline` is the answer there, texture and all.

- **`TuckedFill`** (`components/tucked-fill.slint`) is the fill for a host stroking its own rounded
  border, inset to the stroke's middle: where the renderer puts a fill under an *opaque* border,
  and where it doesn't under a translucent one. Mounted first, at full size. Two mounts: the lyrics
  pill, whose border is translucent, and the toast, whose accent bar has to sit between the fill
  and the border. **Not under a drop shadow**: a fill stopping half a pixel in lets the shadow
  darken the rim, so a shadowed element keeps its `background` out to the edge.

### Grids, strips, cards

- **Every grid is `EntityCard` in a virtualized chunked-row `ListView`**: Rust hands it grid-row
  structs of N cards, and **`GridColumnsSync`** computes N from the width and fires
  `columns-changed`, so a resize re-chunks without touching the database. Detail views own
  no header — the four under My Library are bodies, the banner being the page's own band. The
  `ListView` sits in a copy-only `DragSource`, so a card drags out as what Add to Queue on it would
  take (the drag entry under Lists and playback).

- **The seven library grids are paged, and `ui::card_grid` is their one load path.** My Library's
  four, Favorites' Most Played and Favorite Artists, and Recently Played's Most Played each install
  a `paged_rows::grid::PagedGrid` once: the ids in SQL order, the cards near the viewport fetched
  by id, and chunk rows reading them through. Each is a `CardGrid` on a marker type, stating its
  query, its store calls and the facts it writes beside the cards, and `card_grid`'s
  `reload` / `requery` / `refresh_cards` / `release` are how any of them loads. **A chunk keeps its
  `ModelRc` for as long as Slint holds it**, the repeater comparing by pointer
  (`slint-pitfalls.md`), so a card that lands repaints its own cell. A column change keeps every
  cached card, and a load re-reads the column count as it lands, so a grid released while hidden
  never enters at a stale N. The launch loads only the shown tab's grid (`spawn_initial_*_fetch`),
  every other tab loading on its first enter. A grid over another card type implements `CardGrid`
  rather than growing a second load path.

- **`EntityCard` goes inside `grid/card-cell.slint`'s `CardCell`, which owns everything a card owes
  its selection.** The menu has to be the card's *sibling* in the card's own coordinate frame, so
  it can't live on the `EntityCard` mount; the card itself arrives through `@children` because
  Slint has no generics and each grid iterates its own row struct. **A mount states its scope and
  nothing else** — `selected`, `selection-live`, `menu-open`, the menu's `effective-selection` and
  its count are all derived there once, and a mount reaching past them for a `CardSelection` call
  is re-spelling what the cell already answers.
  Per-kind menu arms (the playlist four, `play-single`) are callbacks the grid forwards, inert on
  a kind that never raises them. "Go to Artist" is not one: it calls the row menu's `Nav` trio.
  **It mounts two menus and opens one by kind**: `card-kind: "station"` raises
  `StationContextMenu`, whose actions are `Radio`'s and resolve to no tracks, and every other kind
  raises `CardContextMenu`. The second mount is free until it opens, a `PopupWindow`'s body being
  built inside its `show()` (verified in the generated tree). A station's `card-id` is its
  `select_key`, Browse's stations all holding `id == 0` (`radio.md`).
  **`BrowseCardGrid` is the one exemption**: its
  cards are its list rows, so it reads `Browse.selected-ids`. `card_selection.rs`'s
  `a_card_grid_mount_states_its_scope_and_nothing_else` walks the `.slint` tree for the four files
  that may name `CardSelection` at all, so the exemption has to stay one.

- **A strip and a grid are different components on purpose.** `HorizontalCardStrip` walks a plain
  `for` — affordable for a capped carousel, not for an uncapped page, where every card is built and
  every cover requested. `EntityCardGrid` is the virtualized counterpart over the same row struct,
  and each of its three instances **dropped its query's cap on arrival**, the reason for a cap
  being the strip's plain `for`.

- **`GridGeometry`** turns `avail-width` + `min-card-w` + `gap` + `card-text-h` into
  `cols`/`card-w`/`card-h`/`row-h` — **feed it the *body*'s width**, which the layout fixes
  independently of the cards, so a card sizing itself from it is a derivation and not a cycle. My
  Library derives it once for four mutually exclusive tabs.
  - **A grid that fits on one row draws that many columns rather than what the width packs**, and
    the mount says so through `lone-row-cards`. Admitting a column a short grid cannot fill shrinks
    every card on screen to make room for nothing, which is what widening the window did to a page
    holding five playlists. Only `card-w` reads the count: `cols` stays the width's own answer and
    so the chunk width Rust is handed, and nothing about the chunking or `columns-changed` moves.
  - **The count comes off the row model, not a count property written beside it** —
    `rows.length == 1 ? rows[0].<field>.length : -1`, spelled at each of the five mounts against
    whichever model the mounted tab drew. `.length` lowers to `track_row_count_changes()` and an
    index to `row_data_tracked(…)`, so the binding re-runs whenever the rows under it change; a
    published count would be a second thing to write, in order, beside every chunking. Pinned by
    `card_selection.rs`'s `a_lone_row_grid_asks_its_model_how_many_cards_it_drew`, which reads each
    mount's binding value rather than listing the shapes the five spell it in.
  - **`max-card-w` is where an under-filled row stops growing**, and `grid_prewarm::MAX_CARD_W`
    mirrors it so the tier covers what is drawn. The pin holding the two spellings together is
    `grid_prewarm_tests::the_card_constants_are_the_ones_the_component_declares`, which reads the
    `.slint` source, so a length default added here owes an entry there.

- **`GridEmptyState`** is the centred glyph + heading + copy block, and **not only for grids** —
  Browse's three states and Playlist Detail's empty list mount it too, which is what the optional
  `action-*` properties are for. Properties rather than an `@children` slot: an empty `@children`
  still claims a `pad-md` gap and shifts the vertical centring.

- **A grid that builds its cards chunks them through `ui::grid_rows::chunk_built_rows`**: Browse
  and Radio's three, which build a flat `Vec` in the walk that filters it and move it into the
  rows, then hand those to `model_diff::replace_rows` (generic over the row type, so the recent
  searches use it too). `columns` is floored at one there and in `PagedGrid::set_columns` alike: a
  grid mid-layout can report zero, and one card per row is a visible wrong where a zero-width chunk
  is a panic. Don't re-roll either.

- **The play-count badge is its own leaf** (`grid/play-count-badge.slint`) with **one** mount,
  `EntityCard` — Radio's `StationCard` hosts one now, so what a station card owes is the count and
  not the markup. It renders a `MaterialIcon` rather than a `"▶"` in the string (the fallback-font
  line-box pitfall) and hides itself at a count of zero, so no host restates the suppression.

- **A card's hover controls are the *host's*, through `EntityCard`'s `@children` overlay slot** —
  the Playlists CRUD trio and Radio's four both. `show-overlay-actions` opens the slot and gates
  nothing else: it named a set of buttons the card drew *and* the slot once, so the second host to
  raise it got three controls it had no callbacks for, on its own corners. The host owes
  `overlay-hovered` back for every control it owns, the star outside the slot included (the card's
  `touch` goes false under any higher-z button), and must frame its
  controls off the published `tile-size` rather than `parent`, which in that slot is not what it
  reads as — `slint-pitfalls.md` argues why. A control stacked under the selection box frames off
  `select-box-bottom`, published beside it for the same reason (the station card's website link).
  A control that stays up while the card is idle is not a hover affordance: it goes *beside* the
  card on `IconButton.fade`, since the slot carries one fade for everything in it.

- **A card's heart is `grid/card-favorite-button.slint`'s `CardFavoriteButton`**, one spelling
  for the station cards, both Most Played grids and Browse's file cards. Top right, beside the
  card rather than in its slot, and up while idle wherever being a favorite says something;
  `marks-kept: false` makes it a hover control where every card is one (Favorites' Most Played,
  Radio's Favorites). The host owes `overlay-hovered: <heart>.has-hover`. **Its state comes from
  where the card's row already keeps it**: `EntityStripRow.is_favorite` on the Most Played grids,
  and on Browse the list row the card *is*, read through `row_index`, so the list's own toggle
  patches both presentations at once.

- **`MosaicHeroTile`** is the 140 px artwork square both curated heroes draw, and it draws **one
  composed collage, never a live `CoverMosaic`** — `media::image::artwork`'s `COMPOSITE_LAYOUTS` owns the
  1/2/3/4 arrangement, so the square, the banner blur and a playlist thumbnail are all laid out the
  same. **What they do not share is what an unreadable source costs**, and the asymmetry is
  deliberate: the heroes go through `compose_cover`, which drops that source and picks the layout
  from what survives, because they recompose from whatever the database's top four currently are
  and a cover deleted underneath should cost its slot rather than the banner; a playlist thumbnail
  goes through `compose_artwork`, which refuses, because the mosaic picker previewed that file slot
  for slot and `CoverMosaic` is a promise. Collapsing the two onto one function for DRY persists a
  collage nobody chose. **They differ on cost as well as on strictness**, and the layout table is
  in half-canvas units so one set of rects serves both: the persisted thumbnail composes at
  `COMPOSITE_SIZE` through Lanczos3, where a hero takes its side from the caller — `COVER_SIZE`,
  the tile `pair_from_image` reduces it to on the next line — through Triangle. A hero canvas any
  larger is resized twice and drawn once. **Every brush in it is a `HeroBackdrop` tier and none may
  become a `Theme.*` token** — with no collage the hero is the dark gradient floor where a light
  theme's accent lands dark-on-dark.
  **What decides the arm is the cover, not the count**: a set whose every entry lacks
  artwork is populated and has nothing to paint, so `count` picks between the page's own glyph and
  the placeholder note rather than between the two arms.

### Hero bands

- **Anything painting on a hero reads `HeroBackdrop`, never a `Theme.*` brush** — including where
  the tier now *carries* a theme token, since a consumer reaching past it is how the layers start
  disagreeing. Six views share **one** global, only one hero being mountable at a time.
  `hero_backdrop.rs` publishes it from the same `backdrop.rs` answer the Now Playing `np-*` tier
  runs, and **`backdrop::kind` decides which of two unrelated answers that is**: the blur measures
  the cover and solves a scrim driving the *composite* into a known dark band, seeding every tier
  from the artwork's hue; the aurora publishes `Theme.base` as a flat floor under a
  **neutral chrome** — black or white at partial alpha, the wash beneath supplying its colour, which
  survived two attempts to derive that tier from the artwork — and does
  **nothing at all** to the washes over them, which is the other half of that answer: a tone band
  holding the composite legible everywhere is a band every wash above it lands *on*, so a
  four-colour sleeve paints as two and the surface reads as a tint. Contrast is the cover's to
  decide and the neutral chrome is what carries the readable half. `kind` is
  the sole **Rust** reader of the flag, so no publisher can answer for a surface the mount isn't
  painting — the Settings switch reads `Theme.aurora-backdrop` too, as Native Title Bar reads its
  own, rather than carrying a `Settings` mirror beside it.
  **Which is also why the setting is restart-gated rather than live**: the two arms publish
  unrelated tiers, so a flip mid-session leaves one stack painting the other's colours, and the
  artwork tiers decide at construction whether to build a blurred half at all
  (`boot::ui_setup::chrome::apply_backdrop_style` raises the flag ahead of `install_views` for that
  reason; `boot::tests::ui_setup_tests` pins the order).
  **Producing the `BackdropSample` is the decoder's job, never the publisher's** — it runs in
  whichever `spawn_blocking` already decoded the cover, the quantize being the heaviest thing on
  that path and `apply` running on the UI thread.
  `on-backdrop` for title and secondary line, `on-backdrop-muted` for empty-state copy, `chrome`
  for a chip label, `chip-fill` for the pill behind it —
  **`chip-fill-at(fade)` when the surface is morphing**, so the pill's weight is stated once.
  **A coverless square takes `placeholder`, not `chrome`**, and that split is the tier's whole
  reason: on the blur the two hold one value, on the aurora `chrome` is a neutral ink — right for a
  chip or a disc the wash reads through, a pale slab with a lamp on it across 140 px. `placeholder`
  is `Theme.accent` there, so the square matches the grid card and the track row painting the same
  glyph, and the hero still owns no `Theme.*` binding. Three mounts (`library-tab-band`'s defaults,
  `my-library-view`'s non-genre arms, `mosaic-hero-tile`), pinned by
  `hero_backdrop_tests::every_coverless_hero_square_paints_from_the_placeholder_tier` — reverting
  one builds and is wrong only under a setting CI never turns on.
  **Anything derived from `chrome` fades with `transparentize`, never `with-alpha`**: the tier
  carries alpha of its own on the aurora arm, so *setting* it paints the neutral ink opaque and the
  wash stops reading through. Both spellings agree on the blur, which is what makes the mistake
  invisible where it is written; `hero_backdrop_tests` walks the tree for it.

- **Two backdrop stacks, and neither knows which tier it is painting from.** `HeroBlurBackdrop`
  and `AuroraBackdrop` take every colour as a defaulted `in property` — `MetaChip`'s idiom — which
  is what lets one component serve `HeroBackdrop` on the six bands and `Player.np-*` on Now
  Playing and the miniplayer, two globals kept separate because a band stays mounted behind an
  open Now Playing. Reading either global from inside a stack ties it to one tier and puts the
  other site's inline copy back; `hero_blur_backdrop_tests` pins both directions. **Two mount
  sites, one wrapper per tier, each with one `aurora-shown` property and only the live arm
  mounted**: each stack sits behind its own branch of that property, the two conditions each
  other's negation.
  **The branch is safe because the condition is a process constant**: `Theme.aurora-backdrop` is an
  `in` property written once by `boot::ui_setup::chrome::apply_backdrop_style`, ahead of
  `install_views` and `app.show()`, so no frame sees it move and neither leaf carries a `changed`
  tracker. The pair was mounted unconditionally with the loser painted transparent for as long as
  an art-less track could move the arm with the view on screen — that term is gone, and **a
  transparent loser was never the free option it reads as**: `Brush::is_transparent()`
  (`i-slint-core/graphics/brush.rs`) answers `false` for every gradient whatever its stop alphas,
  and femtovg's `draw_rectangle` tessellates the path before `brush_to_paint` looks at it, so a
  stack faded to nothing still filled the whole surface every frame — four of them on the blur arm.
  What the gate does **not** retire is `shown`: the mounted stack still drains to its idle colours
  through it (below), which is why each stack's `shown: false` arm stays a *gradient of the same
  shape* — `Brush::interpolate` blends stop-for-stop and only between matching types, so
  "simplifying" that arm to a solid `transparent` kills the drain.
  `backdrop_mounts::every_backdrop_site_mounts_only_the_live_arm` anchors each gate at the
  head of its mount line, `if root.aurora-shown:` being a substring of the negation.

- **No view mounts that pair directly; each tier has a wrapper.** The six bands take
  `components/hero/hero-backdrop-stack.slint`'s `HeroBackdropStack`, bound to `HeroBackdrop` and
  replacing the twenty-five lines `MosaicTabHero` and `LibraryTabBand` had each. Now Playing and
  the miniplayer take `components/now-playing/np-backdrop-stack.slint`'s `NpBackdropStack`, bound
  to `Player.np-*`, which they can share because entering the miniplayer force-closes Now Playing.
  Two wrappers rather than one parameterised by tier is the whole reason the leaves take their
  colours as inputs. Each wrapper's `shown` is the **host's** question:
  - **ungated** for the two mosaic bands and Now Playing, which never stop painting;
  - **`detail-open`** for `LibraryTabBand`, whose globals outlive the tab that filled them;
  - **the drain** for the miniplayer, whose `MiniBackdrop` fades the stack out through `shown`
    and unmounts it once drained, the backdrop being off for every install that never presses
    the button. A press mounts it unpainted and paints it a tick later, which eases in only
    because both stacks read their faded layers in `init` (`slint-pitfalls.md`'s `init` entry).

  That makes the gate a two-file deal and each half its own pin: the band passes the term
  (`library_tab_band_tests::no_hero_tier_outlives_the_banner_it_was_solved_for`) and both
  wrappers forward it to each child
  (`backdrop_mounts::the_wrapper_forwards_its_hosts_gate_to_the_mounted_stack`). Drop either and
  both files still read correctly while My Library paints a detail's backdrop flat.

- **Every hero has washes, so the mount gates on the setting and nothing else.** What differs is
  where the three colours come from: a cover's quantize, or — for the two heroes that have no
  cover — a substitute seed. Genre Detail hands over `genre_accent`'s **saturated tile pair**
  (`hero_backdrop::GenreStops`), the one its own square and grid card paint; the dimmed *hero* pair
  in the same struct is the blur floor's alone, and swapping them builds and paints a plausible band
  on both arms, hence a pin. Anything else with no artwork substitutes `Theme.accent`, and does it
  **as two seeds rather than as a rotation origin** — left as the origin every wash comes out at
  `FILL_WEIGHT` and the surface reads as unpainted `Theme.base`; seated as one, the sweeps have a
  single colour to run three ways and the band reads as a flat tint. A pair plus a fill is exactly
  the shape a genre hands over, which is the point: they are the same case and had visibly stopped
  looking like it. **And the pair is seated under `aurora::WASH_MAX_TONE` first** — an accent is
  picked to be legible as *ink on the app's surface*, so it sits well above anything a record
  quantizes to and washed at its own tone reads as a lamp beside a genre's hashed stops. A ceiling
  rather than a tone, so an accent already that deep (Latte's) passes through carrying its own
  chroma; `WASH_MAX_TONE` is the one number here to tune by eye. This retired a `has-tints` /
  `np-has-tints` pair both mounts had to fold in; a term put back strands one site on the blur
  under a setting its sibling honours, which
  `backdrop_mounts::no_backdrop_site_gates_the_aurora_on_anything_but_the_setting` reads
  off each binding's own text. **A genre publishes through `apply_gradient`, which picks its arm
  from `backdrop::kind` itself** — it has no `BackdropSample` for `solve` to pick from — and is in
  `republish_for_palette` for the first time, its tiers having been theme-independent only while it
  was permanently on the blur.

- **`ActionPill`/`SearchBar` inside a hero are the deliberate exception** and stay on
  `Theme.floating-chrome-bg`, still mostly their own surface — safe only because the backdrop is
  *pinned*. A `chrome`-tinted placeholder is the opposite case: translucent enough that whatever
  fills the rectangle behind it is most of what its glyph composites against, so that fill has to
  be a hero token too. Genre Detail's square is the other exception, keeping the name-hashed
  gradient and white glyph it shares with the grid card, already theme-independent.

- **No layer may ease *out of* a held tier.** On My Library the globals routinely describe a hero
  the band stopped painting several tabs ago, so an eased layer bound to them settles on it and
  interpolates out the moment a hero opens — a genre's pink under a playlist. **Make the idle
  value honest rather than suppressing the animation at the right instant**: the palette mirrors
  fall back to their idle half on `root.detail-open`, and both stacks swap their stops for a
  transparent pair on a `shown` input defaulting `true` — the blur's floor and scrim, and the
  aurora's base plus all three washes. **`detail-open`, not `hero-shown`** — gated on the latter
  they drain toward idle for `dur-med` *after* the collapse ends; the band folds it into `shown`
  beside the arm term rather than owning that input outright. **The scrim used to be the one
  exempt layer** and is gated now: draining it could only brighten the artwork while this stack
  was alone on screen, and it now cross-fades against the aurora, where a scrim at full strength
  under a half-faded pair darkens the midpoint of every crossing. The base and the three washes are
  the aurora's whole stack, so gating them leaves only its **dither, the one layer behind an `if`** —
  alpha living in the tile leaving nothing to fade, and a one-level grain being invisible enough to
  afford the cut.

- **`has-cover` is how a host says "not this one".** The `cover:` ternary must bind *some* global
  on every arm, Slint having no empty-`image` literal, and Genre owns no cover — so the Genre hero
  painted whichever other detail was open, which a tab left with its detail open makes routine.
  `ArtworkImage` takes `has-cover` defaulting `true`. Neither the blur quartet nor the washes need
  an equivalent: `has-blur: false` is exactly what a procedural backdrop is for, and a tint is
  always a colour — an entry with nothing of its own substitutes a seed rather than going absent.

- **A hero may publish into either shared global only while it is the one on screen.** Every My
  Library tab keeps its detail across a leave, so several are routinely open and only the mounted
  one may write. The gate is passed to `apply_detail_artwork` (guarding the `HeroBackdrop` write **and only that** — the cover and
  blur slots either side are the view's own, and writing those while hidden is what leaves the
  page ready to paint), `apply_genre_hero` and `hero_chips::publish`.
  - **On the four details it is a live `tab_is_mounted` read *after* `on_applied`, and both halves
    are load-bearing.** A section shadow updates only on the next frame, so a cross-section drill —
    which moves the tab from inside the closure that publishes — answers for the tab being left;
    and the shadow is the wrong *question* besides, going false when Now Playing covers the band.
    Hoisting the binding above `on_applied` compiles, reads correctly, and puts the bug back.
  - **A launch reopens only the shown tab's detail.** A hidden tab's reopen could publish nothing
    shared and would decode a hero and covers no frame draws, so its seed parks the id
    (`SectionState::defer_restore`) and the tab's first enter reopens it
    (`take_deferred_restore`, dropped if a drill landed a detail first). The seed asks
    `tab_is_mounted` rather than the nav index, five sections sharing index 3.

- **A leave is not a teardown on a tabbed page.** Nothing clears a detail id on a tab switch, so
  the band is either collapsing out of that banner or one pick away from morphing it back open,
  and handing the globals back at the leave plays the exit morph over a fallback glyph on an
  accent-seeded floor. **The invariant is that `*Detail.*-id >= 0` means "this banner is in the
  globals"** — `open_*` writes the id *last*, in the same tick as the cover, blur pair, solve and
  chips.
  - Colours and image slots gate on **`the_band_is_up`** (nav is still 3 — deliberately *not* the
    section gate's predicate, which also goes false behind Now Playing). What hands them back is
    **`hero-collapsed`**, per id, plus the page's own teardown.
  - **The chip row stops one step earlier, and the asymmetry is the point**: a colour held across
    a hand-off is the outgoing hero's *tone*, a count held across it is its *facts* under the
    incoming title. `clear_if_stale` reads a `ChipOwner` every `publish_*` stamps; the pure
    decision is `should_clear(recorded, band, still_open)`. A predicate taking the *departing tab*
    cannot answer this — a cross-tab drill fills the strip in the tick that moves the tab.
  - `clear_hero_blur` still calls `reset` directly and is correct, its own `section_active()` bail
    meaning it can only run where `the_band_is_up` is false. The tempting inversion — "an
    on-screen hero is what the gate is *for*" — is what a later edit acts on; the gate asks whether
    **My Library's band** can still reach these globals, not whether anything is painted.
  - **The hold costs up to three details' `(cover, blur-a, blur-b)` triples** for a page visit,
    handed back by the page teardown and per-detail by any genuine close. Rust-side caches are
    untouched — every leave still drops the LRU, the grids and the row models.

- **`HeroChips` publishers take their facts as arguments or off their own section handle, and read
  back no Slint property** — lifting a count off the global the caller just wrote makes write order
  part of the contract. **Facts a stats row doesn't carry are answered on the worker, never in the
  closure**: in SQL where the page holds only ids (`queries::track::scope_facts` and
  `genre_lines`, folded by `ui::hero_folds`), off the rows themselves where it holds them, their
  `Copy` results riding into `upgrade_in_event_loop`; **Most Played's totals are an aggregate
  beside its ranking** (`queries::track::most_played_totals`), answered with each load's ids, the
  grid holding no rows to sum. **Moving a fold onto the
  worker moves its teardown with it** — `release_section_state` resets the folds beside the caches
  they summarise, a derived value outliving its source being the one thing the band can state that
  is *wrong* rather than merely absent.

- **A spread of one is stated as nothing**, and **a set of none as no band at all** — an empty
  hero publishes zero chips and leaves the copy to what the body already paints, so no page says
  "nothing here" twice. Each tab gates on its *unfiltered* count, a filter matching nothing being
  the empty states' business. **A band states facts about the set the page is about, never the
  current filter**: forced rather than chosen, an album's chips being unable to follow its track
  filter without lying about the album.

- **`HeroChipStrip`** is where a hero mounts `MetaChipStrip`: it fixes the `HeroBackdrop` brushes,
  the chip list and the row cap, making the wrong mount unspellable rather than merely tested
  against. `MetaChip`'s `Theme.*` defaults are correct nowhere and never taken —
  they exist so the file imports neither `Player` nor `HeroBackdrop`, which is what lets one
  component sit on a blurred cover *and* a blurred banner.
  - **`fade` multiplies both brushes** and only `LibraryTabBand` sets it, keeping the strip
    *mounted* across the collapse, where an `opacity` under 1 would cost a layer every frame
    (`Opacity::need_layer` bails only at exactly `1.0`).
  - **The strip wraps in a `FlexboxLayout` at the width it is drawn, and the host decides how many
    rows show** through `max-rows`: none for the Now Playing column, two for a band, which clips the
    rest away whole, the chips being one height. **Two is measured rather than picked** and argued
    at `HeroChipStrip`'s binding. If a wrapped row ever clips wrongly, the fix is a per-hero cap,
    **not** a taller band — the band clips, so the failure is bounded.
  - **The flex is the root's direct child and the root's `height` is read off it**, never through
    an `if` (`slint-pitfalls.md`'s entry on a flex inside an `if`), so `show-rows` hides it rather
    than unmounting it. The height is bound so a host short of height can't squeeze a row over
    what follows, which also keeps the strip out of any `FlexboxLayout` (`slint-pitfalls.md`'s
    flex measuring entry).
  - **`Theme.hero-title-size` is the one number all six titles read.** Pinned over **`HERO_VIEWS`**
    (two bands, six banners), with **`MOSAIC_HOSTS`**/**`BAND_HOSTS`** holding each page to mounting
    its band and growing no title, chip strip or artwork size of its own — the detail bodies being
    the half worth pinning, one regrowing a header passing every other check.

- **The curated heroes' `MosaicGuard` means "this collage is what's painted", so the claim moves
  only *past* the check that decides whether anything is** — inside each view's
  `publish_hero_artwork`, never at the fetch that kicked it. Both bail when the section went
  inactive mid-compose, so a set claimed beforehand records a paint that never happened and every
  later refresh for the same top-4 early-returns, leaving the banner on the accent-seeded floor
  until a section leave's `forget_mosaic`. **`claim` is check-and-set under one lock**, the second
  of two in-flight composes for one set being a no-op rather than a second paint.
  **The publish is gated whole, where a detail view fills its own slots even while hidden**: this
  page's leave wipes its models and forgets the guard, so slots written behind it have nothing to
  be ready for and their claim would suppress the re-enter's recompose.

- **Now-Playing accent tiers are derived on the `Player` global, not at the call site** — one
  solved brush plus three named translucent tiers off it. Reach for the tier, never a fresh
  `.with-alpha(…)`. **Only alphas used twice or more earn a name**; a one-off stays inline, naming
  it would move the number without sharing it.

### Lists and playback

- **The row right-click menu is `components/track-list/track-context-menu.slint`, and the only
  place its entries are mounted.** Four hosts: `TrackListRowItem`, the queue panel's
  `QueueRowItem`, Up Next's row (Now Playing's only, through `menu-enabled`, the miniplayer's sheet
  having no room for a menu) and the now-playing bar's track card. It owns the entries, the
  `multi-active` gate and the `effective-ids` every handler reads; the host only says where, through
  `show-at`. **The gate's rule is `Selection.acts-on-set` and `effective-ids`**, the item selected
  *and* more than one selected, so a right-click on an unselected item falls back to that item
  alone; the card and station menus read the same two, and a menu spelling the rule again is the
  copy those replaced.
  **`view-context` is most of what a host configures** — it suppresses the entry naming the
  surface you are already on and picks the remove arm, and `"queue"` drops Play Next and Add to
  Queue outright, the queue being what they name. `"up-next"` is the queue's arm less the selection
  group, Up Next picking nothing. `"now-playing"` drops those two and Play as well,
  its row being the track already playing out of a queue that holds it, and the selection group
  with them, the bar having no set. A remove label handed over by the host would ship
  untranslated, `@tr` resolving literals at codegen, so a third list's arm is one more `if` here
  rather than a property.
  **Its rating entry is `components/rating-menu-row.slint`'s `RatingMenuRow`, which the bar's
  overflow menu mounts too**, each host sizing it to its own rows. **A star leaves either menu
  up**, so a pick can be corrected in place: no host dismisses on one, and the row takes every
  press its strip misses, which would otherwise reach the host's `PopupDismissCatcher`. Over a
  selection it shows the clicked row's stars and the host applies the pick to the whole set,
  `FavoriteMenuItem`'s rule. The two are the queue rows' only way to rate and favourite, their
  title carrying no marks.
  **The bar, the one host with no list row, reads the "Go to" ids through `ui::track_links`**,
  `TrackSummary` carrying no foreign keys: onto `Player.track-{album,artist,genre}-id` per track
  change and again per library tick, a retag keeping the id. The queue panel's rows carry theirs
  from the track-list projection, as a list row does.
  **"Go to" calls `Nav.go-to-*` straight, as Open Containing Folder calls `Nav`**, with no host
  forwarding: the origin a drill records is read off where the user stands, so there was nothing
  per host to say. `cross_tab_nav::land` uncovers the content panel as the drill lands by closing
  Now Playing, the bar staying reachable over it, and the floating queue with it. The docked queue
  panel stays open, sitting beside the page rather than over it.

- **The five right-click menus share one shape**: the row menu, `CardContextMenu`,
  `StationContextMenu`, the radio history list's and `TextEditMenu`, every single-line field's.
  `components/context-menu.slint` holds the
  parts (`MenuItem`, `FavoriteMenuItem`, `MenuFlyout`, `ContextMenuMetrics`, `CopyField`) and
  `ContextMenuPopup`, and `components/track-copy-items.slint` the track Copy fields the row and
  card menus share; each menu argues its own entries.
  - **The row, card and station menus open `ContextMenuPopup`, never a `PopupWindow` of their
    own.** Its header says what it owns and what stays on the menu's root. **The flyout's rows are
    a `[CopyField]` list** (`TrackCopyFields.list`, the card's album fields, the station's), for the
    reason that header gives, and `crates/melodia-app/src/library/tests/clipboard_tests.rs` reads
    each list's `token:` literals against the parser behind it.
  - **A menu opens on the right button's release and takes its anchor once, there.** Every
    surface asks `ContextClick.raises-menu`, whose comment argues the release, and stores where it
    landed: the four menu components through `show-at(x, y)`, the history list in its own
    `menu-x`/`menu-y`. None binds its anchor to a live `mouse-x`, which an open popup would follow
    for as long as the button is held (`slint-pitfalls.md`). `crates/melodia/tests/context_click.rs`
    holds the first half: only the predicate may name the right button.
  - **The item a menu was raised on stays lit while it is up**, an open popup taking its hover
    (`slint-pitfalls.md`). The three item menus publish their popup's `is-open`. A row reads it
    into the one `hovered` its hover bindings share, and a card is told through
    `EntityCard.menu-open`, which `CardCell` answers for both its menus, Browse's grid for its own
    and `StationCard` passes through. `context_click.rs` holds both halves: a row reads its raw
    hover in that one binding and nowhere else, and every mount raising a card's menu binds
    `menu-open`, which `card-hovered` counts.
  - **Copy opens a flyout inside the menu's own popup** where the surface has more than one field
    to offer, and is a flat entry where it has one. The flyout opens on whichever side
    `ContextMenuMetrics.opens-left` finds room, so the column stays under the pointer: rightward
    the popup grows by it while it is open, leftward it holds the room from the start, an open
    popup moving its origin a frame late (`slint-pitfalls.md`). A second `PopupWindow` is not an
    option (same file).
  - **`ContextMenuPopup` takes `close-on-click-outside`, and every entry but the rating row calls
    the menu's `dismiss()` after acting.** `close-on-click` closes on any release inside the
    popup, the click that opens the flyout included. After, because `pop.effective-ids` goes with
    the popup. The history list's menu and `TextEditMenu` are flat and keep `close-on-click`.
  - **The width follows the rows**, `ContextMenuMetrics.menu-w` over the rows' `preferred-width`,
    declared on the popup since only it can read its descendants. Labels elide past the ceiling. A
    fixed width is what clipped a long Remove label; the chevron sits in a fixed box, its glyph
    being picked by a side the width decides.
  - **A menu taller than the window scrolls** (`slint-pitfalls.md`'s oversized-popup entry). The
    column caps at `PopupFit.max-h` in a `PopupColumn` and the popup places its own `y` through
    `PopupFit.fit-y`, which is why each menu passes `anchor-window-y` beside `anchor-window-x`. A
    scrolling menu widens by the bar's gutter rather than taking it from a row, and the Copy flyout
    follows its row through the scroll.
  - **An item menu's Copy acts on the effective set, one line per item, in the order handed over**, never
    re-sorted: on a list or grid that is the selection's pick order (a range in display order, a
    ctrl-click appended), so it matches what Play queues, and on the queue panel it is queue order,
    the set being read back off its rows. An item without the field adds no line. Why line breaks
    and never a space is `library::clipboard`'s. Menus pass ids to `CopyActions` and Rust resolves
    the text there; stations copy through `Radio.copy-stations` by `select_key`, resolving against
    the slice's own caches.
  - **There is one clipboard**: `AppWindow.copy-to-clipboard`, a hidden `TextInput` reaching the
    platform clipboard Slint's backend owns, which on Wayland is the app's own display connection
    where a clipboard crate would land on XWayland. `ui::clipboard::write` is its one Rust caller,
    raises the confirmation, and never logs the text, a stream URL being able to carry a token.

- **A list drags through `components/drag-source.slint`'s `DragSource` round its `ListView`, and
  reorders through `components/list-drop-target.slint`'s `ListDropTarget` over it, never a gesture
  of its own.** Three lists reorder: the queue panel, Playlist Detail's `TrackList` (the one mount
  naming an `origin`) and Up Next, in Now Playing and the miniplayer alike. The rest drag out only,
  copy-allowed: the other eight track surfaces' `TrackList`s and the five card grids. The row
  reports its press, `src.arm(idx)` where the drag only reorders and `src.arm-items(…)` where it
  carries something out, and a reorderable row reads `lifted: src.lifts(idx)`, dimming by
  `Theme.fade-lifted`; the target owns the slot, the line, the edge scroll and the no-op slots,
  handing the host `reorder(from, to)`, `move-picks(slot)` and `scroll-by(dy)`. A host whose rows
  fall into two runs that a move may not cross names the second's first row in `split-row`, and
  the slots across it draw no line and drop nothing.
  - **A press on a picked row while others are picked lifts them all**, the menu's rule
    (`Selection.acts-on-set`). The row arms as usual, then `src.lift-picks(…)` with the host
    global's `picked-run()`, Rust being the only side that can find the picks in a list it pages
    (`picked_rows` on `PagedRows` and on `QueueRows`, `ui::drag::lifted_run`). Each picked row adds
    `|| (row.selected && src.lifting-picks)` to its `lifted`, the no-op slots widen to the run
    (scattered picks have none, any drop gathering them), and the drop arrives as
    `move-picks(slot)`. The queue panel and Playlist Detail take it; Up Next has no picks.
    **Every tier moves through `melodia_core::utils::block_move`**, one row being a block of one,
    so the play order, the shown keyset and `playlist_items` can't land one drop three ways.
  - **What a drag carries is `Drag.payload`, written from two trees** (`ui::drag` names the three
    writes): the pressed row arms it, every press disarms it first in
    `winit_filter::disarm_drag`, a touch's start included, and a list whose rows move under a drag
    revokes its own origin (`queue_panel::rows::show_queue`, `PlaylistsUi::rows_moving`, Up Next's
    `show_slice`). **A new reorderable list owes the revoke**, the one write nothing fails without:
    the drag goes on over the rebuilt rows, and the index it lifted lands on another track. A revoke
    keeps the items, which are ids and survive the rebuild, so the drag can still copy out.
  - **What another list takes is `Drag.payload.items`, Add to Queue's `(kind, ids)`.** A track row
    builds it in `TrackListRowItem.drag-items()` and a card in `CardCell.drag-items(title, cover)`,
    both through `Selection.effective-ids`, so a drag carries what the item's menu acts on.
    `EntityCard.pressed` is a left press on the card body; one on its box or a hover control arms
    nothing. The queue panel's target `takes-items` and lands them as a copy ahead of the row they
    were let go above through `Queue.drop-in`, which names that row by its `QueueEntry` before
    resolving as Add to Queue does (`card_actions::spawn_with_track_ids`) into
    `queue_insert_before`; its header gives way to what letting go would do while `holds-items`. The
    bar's `components/now-playing/queue-button.slint` takes a drop as `CardActions.add-to-queue`
    verbatim and opens the panel after a 500 ms hover, so a drop can still be placed at a slot.
  - **The chip under the pointer is `components/drag-ghost.slint`'s `DragGhost`**, for a drag that
    carries items and not over its own list's reorder target (`Drag.follow`'s third argument). A
    target positions it from its `can-drop`, and elsewhere a window-sized `DropArea` at the back of
    the root's stack does, which can't move to the front (`slint-pitfalls.md`'s in-window drag
    entry). `Drag.live` mirrors the source's `dragging`, the one end every drag reaches.
  - **A touch never starts a drag**: every `DragSource` stands down while `Drag.touching`, which is
    its `enabled`, so no mount binds `enabled`.
  - **A `DragSource` hands back the click its rows don't take**, as `background-clicked`. A list or
    grid on a page with a filter box forwards it as `request-blur-search`; the queue panel and Up
    Next have no box and leave it unbound.
  - **Up Next's rows are play order only modulo the repeat-all wrap**, so its target hands
    `NowPlaying.reorder` row indices and `up_next::play_order_move` maps them, refusing a move
    across the wrap. Up Next's `split-row` sits on that wrap, so no line promises a move Rust then
    refuses, and Rust's refusal stays the one that holds. The queue panel and the playlist reorder
    in their own rows.

- **A file dragged in from the file manager is not that drag, and no `DropArea` sees it.** Its
  banners read `FileDrag.target`, which the winit filter writes from a hit test in Rust, and a
  target says where it is drawn through `components/window-rect-report.slint`'s
  `WindowRectReport`. A new file target mounts a report outside any `if` but the full-UI branch
  and gains an arm in `window_chrome::file_drop`; `desktop-shell.md` has the flow. **Its banner
  is `components/file-drop-banner.slint`'s `FileDropBanner`**, never one drawn by hand.

- **A list that can hold the whole library shows it through `ui::paged_rows::PagedRows`, never a
  `VecModel` copy.** It keeps the ids in their SQL order and only the rows near the viewport, and
  its module doc argues the window, the one fetch in flight and the seed a load fetches where the
  list is scrolled. Both Songs tabs and the four details are six of them, all
  `track_rows::TrackRows`. The queue panel is the seventh, a `queue_rows::QueueRows` built over one
  as `PagedGrid` is, and it is the one list that **picks by row rather than by track**: a track
  queued twice is two rows, and the picks name the engine's `QueueEntry`s, so each copy is picked,
  dragged and removed on its own, and the menu counts rows and acts on each track once. The
  entries come off the state through `queue_panel::rows::read_queue`, never off `QueueViewModel`,
  whose doc says why. A favourite or rating change patches the
  cached row, where the `VecModel` lists go through `model_patch`; the `list_selection` helpers
  take either model. A list over another row type implements `PagedRow` for it rather than growing
  a second model.
  **The six track lists load through one path, `track_rows`' over `PagedTrackList`**, the twin of
  `card_grid`: each states only where its query comes from, when a load may still land and what it
  writes beside the rows, and the four details do it through one impl over `TrackDetail`. A track
  list over another scope implements `PagedTrackList` rather than growing a second load path.

- **The queue panel sits after the content panel in the shell's row**, and five things about it
  reach across files.
  - **`Queue.shown` is the docked panel open with room for it, or the overlay open, less what
    covers either**: Now Playing, and the miniplayer, which unmounts it. It is computed and written
    on the `AppWindow` root alone, a global being unable to watch its own properties and a mirror
    inside the panel missing the miniplayer swap. Rust's rows and the private cover tier follow
    it through `shown-changed`, so a hidden panel holds nothing past `lifecycle::TEARDOWN_DELAY`,
    whatever hid it, and the file-drop route reads it at every move. The root writes `shown` before it fires
    `shown-changed`, which is what lets the panel's seat read the playing index Rust has just
    written, and `Queue.floating` in the same handler, so the two never describe different panels
    for a frame.
  - **`Queue.toggle()` is Rust's** (`ui::queue_panel::lifecycle`), the one path the bar, its
    overflow entry and Q share. Under Now Playing the panel is open but stepped aside, so a press
    closes Now Playing to bring it back, and `globals/queue.slint` importing `Nav` would break the
    rule that only the dispatchers import sibling globals. It opens whichever mode the window has
    room for. `Queue.close()` needs nothing outside the global and stays a Slint function.
  - **Its inner edge is `components/edge-resize-handle.slint`'s `EdgeResizeHandle`, and so is the
    sidebar's.** A third resizable side mounts it with its own `grows` rather than copying the
    absolute-coordinate drag.
  - **A narrow window takes room back in a fixed order, so the content keeps
    `Theme.content-min-w`**: the sidebar draws its rail, the panel narrows to its minimum, and then
    it steps aside through `shown`. **The floor holds without the panel too**: the sidebar folds
    once the window alone would leave the content under it, which makes the floor the narrowest
    content panel any page has to seat. The shared header and the band answer to it rather than
    to the window (`TabBar` and `LibraryTabBand`, below). The `AppWindow` root works the steps
    out, being the one element that sees the row, and argues them there. The flags carrying them
    out of that file (`Nav.sidebar-yields`, `Queue.fits`, `Queue.floating`) are each written from
    a `changed` mirror rather than bound, since a binding over the window's width restarts the
    sidebar's width ease on every resize move (`slint-pitfalls.md`'s structural-dirt entry).
    **`Nav.sidebar-yields` feeds `Nav.sidebar-rail`, and every part of the sidebar draws from the
    rail.** `sidebar-collapsed` is the user's own fold and the only one persisted, so a renderer
    reading it draws an expanded sidebar over a forced rail. The fold toggle and Ctrl+B stand down
    on `sidebar-yields`. The panel's half of the fold is keyed on the docked panel rather than
    `open`, so the sidebar comes back once the panel has stepped aside: keyed on `open`, a panel
    too wide to show would hold the sidebar folded with both fold controls standing down.
  - **Past that last step the panel opens as an overlay, and its controls stay live.** Two states
    carry it: `Queue.open` is the docked panel's and the persisted one, `Queue.overlay-open` the
    overlay's and session-only, so a narrow launch with the panel saved open shows nothing.
    **Narrowing hides a docked panel rather than floating it** (Kenan): the overlay dims the page
    and takes the keys, which a resize should never raise unasked. Widening with the overlay open
    docks it, in `changed queue-fits`, with `shown` held through the hand-off. **It is the same
    `QueuePanel`**: a second would share the rows' fetch anchor, the drag state and `pointer-in`,
    and couldn't sit behind an `if`. Floating, the root takes no width, stops clipping (a
    clipping item takes every press outside its own box) and draws its body to its own left, inset
    by `float-inset` off the content panel's edges so `RoundedFrame`'s corners stay clear.
    `QueueScrim` dims the page from inside the content panel, ahead of `RoundedFrame`, and closes
    the overlay on a click. An overlay folds no sidebar, taking no room in the row, and owns the
    queue's keys while it is up.

- **A track's title and its marks are `components/track-list/track-title.slint`'s `TrackTitle`**:
  the heart a favourite keeps up, and the star strip a track list row reveals on hover, each
  switched by the host. A track list switches both per view from its columns pill, beside the
  heart its Length cell swaps in on hover, and keeps all three in `views.json`'s `view_marks`
  rather than `view_columns`, which `ViewStateData` argues. The queue rows mount it for the title
  alone, their favourite and rating being the row menu's, so the title keeps that room. A third
  row showing a track's title mounts it too, for the elide and the LTR base its `Text` carries.

- **The four track-list details share one wiring, `callbacks::track_detail`**, driven through
  `track_detail::DetailGlobal` (the generated global) and `TrackDetail` (the view handle). Each
  lists its entity as a `TrackRows`, its `DetailCache` holding the sort and filter the loads run
  under. `track_detail::load` is the fresh open, the one load running under a query of its own
  rather than the page's; every other load is the shared track-list path. A close or a
  section leave releases the list and revokes every load still on its way to it
  (`track_detail::release_rows`). A fifth detail implements both traits rather than copying a
  `callbacks/detail.rs`.

- **The four grid tabs share one section lifecycle, `callbacks::grid_tab`**, driven through
  `GridTab`, which extends `TrackDetail` with the tab's `CardGrid` and what its leave and enter
  differ in: the globals they write, the hero release, what the tab keeps resident, and its
  detail's open, refresh and restore. The ordering a leave and an enter depend on is the shared
  file's alone. A fifth grid tab implements `GridTab` rather than copying a
  `callbacks/lifecycle.rs`.
  **A library change under an open detail reloads the detail and leaves the grid owed**, every
  star and heart inside a detail being one, so the reload lands once on the close. That close is
  each tab's own `wire_close` in `callbacks/detail.rs`, outside the shared file, and it calls
  `grid_tab::reload_owed_grid` after `forget_closed`. A fifth tab's close owes the same call, or
  its grid comes back showing what it held before the change.

- **All nine track lists share Play Next, Add to Queue and the column toggle**, written once in
  `callbacks::track_list` over `track_list_view::TrackListActions`, which `track_list_views!`
  implements beside the column state, plus the `play-row` tail as `play_displayed`. A list
  supplies only what differs: its id source, and Browse its non-zero id filter. **Selection is
  shared there too, by the four flat list views and the four details**: `wire_row_selection` over
  `list_selection::RowSelectionView`, ranging over the rows on screen. Browse keeps its own
  wiring for its disk-only rows, over the same `compute_click_selection`, whose anchor is the id
  of the row last clicked. **So is the drag out**: `TrackList` mounts a `DragSource` round its
  `ListView` (the drag entry above), and a disk-only Browse row arms nothing, its body being
  disabled.

- **`play-row` replaces the queue with the view; there is no single-track play path, and no
  Play-All pill.** Every row activation resolves the view's *displayed* ids and hands them to
  `player_play_tracks(ids, start)`. The eight Play All pills made that same call pinned to
  `Some(0)` — exactly what activating the first row does — and are gone; don't reintroduce one.
  Appending without wiping is the *context menu's* job.

- **The pill that remains is Shuffle**, through `spawn_play_then_shuffle`: play from a **random**
  slot, then `queue_set_shuffle(state, true)`. Random because the shuffle anchors the current
  track at the front, so a head start opens every press on the same song; a set rather than a
  read-then-toggle because the pill means "on", and a toggle racing the transport's button would
  turn it off.

- **The start slot is `play_row_start(&ids, id, idx)`** — trust the index Slint passes when it
  lines up, otherwise look up by id, Browse's disk-only rows making its two index spaces differ.
  `player_play_tracks` re-resolves by **track id** against what the DB returned, the fetch dropping
  ids that no longer exist. A new list view wires `play-row` to its own `*_track_ids()` helper; the
  two Most Played grid tabs have no row index and resolve by id against the grid's own keyset
  (`card_grid::shown_ids`), so the cards and the queue can't disagree about what's on screen,
  the filter included.

- **Track-list insets belong to the list, not the page.** `TrackList` takes `inset-left` and
  `inset-right`, defaulting to `Theme.track-list-inset` and `Theme.scrollbar-slot`, and resolves
  its columns inside them. So the Songs tabs and the four detail bodies mount the list at full
  width with no side padding of their own, a detail's header sitting flush under its band, and the
  right edge stops at the bar's slot so a row's pill never runs under the track. Two hosts differ:
  Browse keeps a `page-inset` for its header strip and card grid, its folder rows following the
  list's column, and Search's Songs section passes `0px` on both sides, being a column inside the
  page's own inset beside the Top Result. Overlay scrollbars stay at `parent.width - self.width`,
  and bottom padding is never on the root (the dead-strip pitfall).
  **Three hosts size a list to what it holds** through `TrackList.content-height`: Browse's and
  Artist Detail's capped at the viewport, Search's Songs section outright. The sum lives there, so
  a change to the list's own chrome is one edit.

- **The Now Playing right column is one column with three arms, not a place two things share.**
  Up Next, the lyrics panel and the station panel, swapped on `Player.vm.has_station` and
  `Lyrics.enabled`; a station keeps its panel whatever the switch says, and the header's lyrics
  pill (`LyricsControls`, the switch and the menu glyph together) is gone rather than disabled
  there, a stream having no track to look a sheet up for.
  **Don't answer a fourth thing with a fourth column.** `content-width` is what the chip wrap, the
  strip width and the cover slot all derive from, so another region reflows the artwork column
  every time a track without lyrics comes on, which is the worst property a panel can have when
  availability is data rather than intent. The lyrics arm sizes its rows in Rust and reads its
  whole type scale off the `Lyrics` global, both halves argued at those two anchors: neither number
  is one to redefine at a call site. One `TouchArea` covers that sheet, so there is no per-row
  `has-hover` to reach for.

- **A button whose hold does something other than its click goes through
  `components/hold-touch-area.slint`'s `HoldTouchArea`**, never a press-armed `Timer` of its own:
  that one place decides when a press stops being a click. `IconButton` and `PlayButton` both sit
  on it, off at a zero `hold-delay`, so every other mount stays a plain button. The sleep timer's
  `StepperButton` is the other shape and keeps its own timer: it acts on the press and a hold only
  repeats it, so there is no click for a hold to replace.
  **The transport's hold half is read off `Player`, never spelled at a mount.** The bar and the
  miniplayer take the hold times, the scan interval, both tooltips, `scan-step`, `stop` and the
  stopped glyph's predicate from it, so a third transport mounts the same reads and none can come
  to behave differently. The scan walks the seek keys' relative run, which is why that run's
  functions live on `Player` too and `ShortcutScope` keeps only the timer landing it.

- **A slider's pointer goes through `components/slider-pointer.slint`'s `SliderPointer`**, never a
  hand-rolled `TouchArea`; its header says what it decides. `SliderTrack`, the volume popup's
  vertical slider and `EqBandSlider` each draw their own track over one, the EQ turning
  `grabs-thumb` off. One place deciding it is what lets a fix land for all three.

### Dialogs, pickers, toasts

- **A dialog's text field sits in `components/field-frame.slint`'s `FieldFrame`**. Three hosts,
  each padding its input as it needs:
  `LabeledInput` (a caption over a two-way field), `ControlledInput` (a value the host owns, one-way
  in and every keystroke out through `edited`, which its header argues) and `MultilineInput` (the
  one wrapping box). `FieldLabel` is the caption, shared with the credit and genre editors, whose
  rows close on `components/dialog/remove-row-button.slint`'s `RemoveRowButton` as the smart
  playlist rules do.

- **Selectable-picker dialogs share one toolkit** (`components/dialog/selectable-picker.slint`).
  Both the Export and Add-to-Playlist pickers are thin wiring over it, toolkit components staying
  data-agnostic. Both commit through the `Dialog.accepted` dispatcher gated on a selected count;
  Add-to-Playlist disables fully-contained playlists and counts only enabled rows. Toggles and
  commit live in `files/export.rs` and `files/add_picker.rs` (commit needs
  `Rc<NotificationsUi>`). Each opens from its own file too.

- **A raise that crosses an `await` owes a `DialogClaim`, and `Dialog.open` is not one.** Six
  openers deliberately leave `open` false while Rust fetches the body, so through that whole window
  two menus both read "nothing is up", both fill the same properties, and the second one's
  synchronous write rides out under the first one's rows: the Add-to-Playlist picker committing a
  row's single track under an overlap count solved for an album. `Dialog.request-generation` is
  what separates them, bumped by `claim-request()` at the head of every opener; take
  `DialogClaim::take_from(&weak)` **synchronously at the click** and ask `holds(&ui)` past the
  await. It keeps the `open` test as its other half, every remaining kind raising `open` in the
  same handler that writes it. **The capture has to follow the opener**, which is why
  `open-tag-editor` and `open-edit-smart-rules` invoke their fetch as their own last statement.

- **A deferred raise is an opener too, single caller or not**, `prepare-export-playlists` and
  `open-new-smart-playlist` being the two that earn one on nothing else. Chrome written inline is
  a block, and a block cannot claim: it leaves `open` false for as long as the fetch or the
  event-loop hop takes and bumps nothing on the way, so a slower flow it should have retired
  passes `holds` and fills the global underneath it. Taking a claim is the half a one-tick hop can
  skip, nothing being able to overtake it; bumping one is not. `dialog_openers.rs` walks for both
  triggers.

- **Notifications stack** routes a row's `NotificationKind` through
  `ui::shell::notifications::run_action`, a Rust `match` rather than a Slint dispatcher like
  `Dialog`'s (the function argues why). A new action is one arm plus one `show_localized(…)` call.
  Cap 5. Per-card props use `data:` not `row:` (Slint reserves `row` as the iter var), and
  translated strings reach Rust via `pure callback`s wrapping `@tr(…)` literals.

- **Backend-thread toasts via `utils::toast`.** `NotificationsUi` is `Rc`, so failures on tokio
  workers surface through a neutral `OnceLock<UnboundedSender<…>>` — no-op when uninstalled,
  holding no `ui::*` types, which preserves the `tasks`-no-`ui` rule. `install_toast_bridge` drains
  the `mpsc` (**not** a `watch` — errors must not coalesce), resolving the localized **title** by
  `ToastKind` and leaving the dynamic **detail** untranslated. Routine failures (favorite, rating,
  nav) keep `spawn_logged!` — don't toast-spam.

### Filtering

- **Every filter box narrows through `Needle`, and none of them spells
  `to_lowercase().contains(…)`.** `melodia_core`'s `utils::needle` owns the `fold_needle` pair
  folding case **and accents**, and its `entities::track_search::SearchFields` what a track is
  searchable by (ordered like the `tracks_fts` column list it mirrors). A list held in memory
  matches through `crates/melodia-views/src/ui/row_match.rs`; a library-sized one asks the store,
  which matches the same `Needle` against a folded column: `tracks.search_text`, which is
  `SearchFields::pack()`, for the track lists, and each entity table's `search_name` for the card
  grids (`library-data.md` owns that side). Sixteen surfaces take one; on all three tabbed pages
  the needle is **one shadow shared across the tabs**, cleared on both sides by a tab pick.

- **Radio's Browse box narrows nothing, and is the one box with no row predicate to find.** The
  directory searches one field per request, so the needle *is* the query, and what else it could
  have meant is offered as a scope pill rather than folded in — `ui::radio::filter` routes it,
  `ui::radio::suggest` builds the pills. The constraint spanning both, which neither file owns
  alone: a pill and a filter chip write the same `StationSearch` fields, so `facets::apply_pick`
  stays their only writer and a pill's edit has to land as **one** `edit_query` — the needle and
  the chip filter the same request, so a pill that set the chip and left the name behind would
  fetch a guaranteed-empty page on its way to the right one.

- **A needle is folded exactly once, and `Needle` is the type that makes anything else
  unspellable** — `fold_needle` is the only constructor and the predicates take nothing else. The
  filter box's fold deliberately doesn't match the FTS side's: it is looser everywhere (which only
  ever *widens* a substring filter) except years, which are a substring here against a prefix
  search there (`library-data.md` owns that side). **A card grid sorts on its name and matches on
  its `search_name`**, two columns rather than one: folding the sort key would change the order it
  drives.

- **Nine of the sixteen are fed by one box, dispatched in Rust rather than bound.** My Library's
  band owns the only `SearchBar`; `my_library/filter.rs::dispatch` routes a settled keystroke to
  whichever surface is up. It must be a *write*: an element can't declare a binding on another
  global's property, and the only form that would work puts `MyLibrary` in five globals' imports.
  - **The two contracts stay different underneath** — the five grid/list globals fire
    `apply-filter(text)` and Rust reads the property back as the load's query; the four
    detail globals fire `filter-changed(text)` and Rust uses it. All nine arms `set_filter` before
    invoking, which also keeps `reorder-enabled` honest, that being the one live *Slint* reader of
    a detail's filter.
  - **Anything that moves the surface under the box reseats it** through `detail-scope-changed()` →
    `filter::sync_box`, deliberately not the tab pick's clear-both-sides: clearing on the way out
    would drop the grid filter the user is coming back to. That is **five** `changed` mirrors in
    the sheet, not four — `dispatch` clears only the *entering* tab's needle, so the arrivals that
    aren't picks land on a tab still filtered under a box that says nothing about it.
  - **The mirrors can't cover a re-open**, which has no edge to fire on, a section re-enter
    re-running `open_*` with the *same* id. Each of the four invokes `detail-scope-changed()`
    itself as the **last** statement of its closure, `sync_box` picking the surface off the live id
    and tab.

## Covers

- **No row struct carries a decoded cover; every one asks for it.** `TrackListRow` has no `image`
  field — `TrackListRowItem` resolves per *instantiated* row through
  `RowCovers.request(path, generation)`, wired once in `boot/ui_setup/views.rs`. New TrackList consumers
  need zero cover plumbing. `CoverThumbs::prewarm` dedupes and caps at LRU capacity — pass paths in
  **display order** so the kept prefix paints first.

- **One tier behind every card grid, and it is `ui::grid_prewarm::tier()`.** Seven of them sat in
  six view handles before — Favorites held two — each built at the same fallback, capped by the
  same `cover_cap_for_window`, tuned by the same retune and told by its own notifier; the split
  bought nothing but a section leave releasing its own, which is what made every re-entry cold. A module singleton rather than a
  `ViewCtx` field, there being exactly one of it and no view owning it — the shape
  `ui::nav_history::nav()` takes. **The tiers that stay private are private on decode size**:
  Search's two card strips and the row tier. Two are private on other grounds: the queue panel's,
  on lifetime (the `QueueRow` entry below), and Radio's logo tier, below.

- **A leave hands the pixels back and keeps the picture.** `grid_prewarm::hand_back_covers` shrinks
  every entry to `PROXY_COVER_DIM` square in place rather than calling `clear`, so a re-entry paints
  the right cover on frame one and the prewarm already on its way sharpens it behind.
  `get_or_schedule_opt` needed nothing new for it: serving an entry whose recorded size isn't the
  one being drawn *and* re-queueing it is the retune case, already argued there. The shrink clears
  `settled` for the reason `set_thumb_size` does — every path the last burst handed to the pool is
  worth handing over again, and left standing a card scrolled to past the next prewarm keeps the
  proxy — and bumps no epoch, there being no released memory for an in-flight batch to land in.
  - **The guard sits ahead of the decode, where it used to sit after one.** A prewarm for an
    off-screen section was answered by decoding and then releasing; against a tier nothing clears,
    that leaves a screenful resident for a page nobody opened. Browse is where it bit, being the
    only view whose boot seed fetches while the section is off screen, so a launch landing anywhere
    else paid for a screenful it never drew. Every other prewarm site gates its fetch on
    `section_active()`, and the three grids a detail can cover (Albums, Artists, Playlists) on
    `SectionState::grid_on_screen`, which argues why. A leave landing *inside* a decode is no
    longer a reason to hand anything back: those buffers are what the re-entry paints.
  - **Radio's logo tier is not this tier, and the reason is its card rather than its decode.**
    `station-card.slint` derives `logo-native-size` from `cover.width / logo-decode-size`, so the
    decoded *extent* is load-bearing for that card's layout: a proxy makes every logo read as a tiny
    source, draw inset at half the tile, then pop to fill when the real decode lands. So it keeps
    the old contract — released outright on leave, `covers-generation` rewound to `0`, and its
    decode size retuned only while it holds nothing — and pays nothing for it, a cold station card
    showing its own monogram on its own name-hashed tile rather than a placeholder.

- **No *uncapped* cover lookup decodes on the thread that asks.** `get_or_schedule_opt` answers
  from the tier and hands a miss to the decode pool, so the caller — a Slint model getter, so the
  event loop — gets the placeholder on that frame. What makes the placeholder temporary is a
  generation the same binding reads: the tier fires one notifier per *batch* and
  `ui::cover_generation::notify_on_decode` bumps the counter on the UI thread. Prewarm still covers
  the common case; this is for the misses it structurally can't reach. A prewarm is capped at the
  tier's own `CACHE_CAP` and walks **in display order**, so a library with more unique covers than
  that keeps its visible prefix and nothing else — and every row scrolled to past it used to decode
  inline, one at a time, on the event loop.
  - **Per batch, not per cover, and per batch *drained* rather than per batch landed.** A
    screenful of misses coalesces into one pass over the mounted bindings, and a miss arriving
    mid-drain joins it rather than spawning a second. A batch another writer already cached, and
    one a release discarded, announce too: every binding that took a placeholder when it queued
    has nothing else to re-run it, so a drain that stays silent leaves those cards on the glyph
    until the next model rebuild, which on a grid is the next resize.
  - **A cached failure is a hit**, so a broken cover re-queues nothing and can't spin against its
    own notifier. A retune re-asks it once, the failure being remembered against the size it was
    attempted at.
  - **The notifier is a feedback loop, and two things stop it running away.** The queue is capped
    at the tier's own capacity, newest-wins, so a scroll can't queue more than the cache holds and
    then evict the visible prefix with the tail of its own batch. And a path the burst already
    handed to the pool is refused — a grid drawing more cards than its tier holds would otherwise
    re-queue whatever the last batch evicted, forever, at frame rate. A miss the burst hasn't seen
    clears that state, which is what tells a moved visible set apart from a thrashing one; so does
    a retune, the burst's knowledge being about the size it was gathered at.
  - **A release invalidates whatever is mid-decode. A retune and a shrink do not, and that
    asymmetry is the point.** `clear` bumps a tier epoch and a batch that captured the old one
    drops its buffers rather than landing behind the section leave that released them. **The bump
    belongs under the *cache* lock**, that being the one a finished batch takes to insert: emptying
    the tier and invalidating the batch have to be one step, or a batch reading the epoch between
    them lands in the tier the release just emptied. `set_thumb_size` and `shrink_to_proxy` are
    outside all of it: each moves what a lookup asks for, not whether there is still a tier to ask,
    so the mounted cards keep painting what they had while the replacements decode. Emptying it
    instead strands every card whose binding runs in the gap on a placeholder, and on a grid the
    next thing to re-run that binding is the next column change.
  - **A surface with no generation to come back on takes `grid_prewarm::grid_cover_blocking`
    instead** — Artist Detail's Albums strip, whose callback carries no counter, and the Edit
    Artwork dialog's cover slot, which is a one-shot property write with no binding to re-run.
    Both are bounded and prewarmed, so the inline decode is a single sub-frame cost; scheduling
    there instead leaves a placeholder nothing ever replaces. `ui::cover_generation::tests` walks
    for both halves — the mismatch is invisible in review and only shows up on a library big
    enough to miss.
  - **The bounded strips and one-shot slots were never on this path and stay inline** — Search's
    two card strips, Now Playing's up-next list, and `Playlists.request-row-cover` behind the three
    picker dialogs. Each draws a capped set against a tier its own fetch warmed, so "never on the
    calling thread" is a rule about the *uncapped* surfaces: grids, and the shared row tier.

- **`QueueRow` goes through two globals rather than `RowCovers`**, each wanting a different tier:
  the queue panel's *private* `CoverThumbs` (so hiding it drops every buffer without yanking
  covers the track lists still need) and the shared row tier. **The private one is
  `CoverThumbs::backed_by` the shared one**: a miss borrows the row tier's buffer before decoding,
  and its size is the row tier's, retunes included, so a cover both draw is one allocation. That
  is what makes a queue the size of the library affordable.

- **`covers-generation` is the repaint token, and on a card grid that is all it is.** A `pure`
  callback's result is cached until a dependency is dirtied, so an `int` the binding also reads is
  the whole mechanism that brings a scheduled decode to the screen. Its *value* decides nothing
  there: `grid_prewarm::grid_cover` schedules a miss unconditionally, which is what the three My
  Library grids always did and what every other grid does now.
  - **One notifier, because there is one grid tier.** `set_decoded_notifier` is a `OnceLock`, so
    `boot::ui_setup::views::install_grid_covers` is the single install and its closure bumps every
    grid global. A bump on an unmounted page is a property write with no bindings to dirty, so
    answering for all six costs nothing and saves the tier having to know which grid asked.
  - **The `0` ⇒ cache-only arm is gone, and its premise is why.** It existed so rows mounting on a
    tab pick "don't queue work for a tier a leave is about to clear" — no leave clears one now, and
    keeping it left a card that missed the retained set scheduling nothing and waiting for the
    prewarm's announce. With it went `should_announce_warm`, the `warmed_tab` plumbing through both
    curated pages' fetch-and-apply, the `mark_covers_warm` / `repaint_covers` split, and the `bool`
    every `prewarm_tab_covers` returned.
  - **Two surfaces still gate on the value, and both hold a tier that really is released**: the
    queue panel's **show**, whose private tier is dropped once it hides, and Radio's leave.
    There `0` still means cold, the lookup still answers cache-only, and the rewind still sits
    beside the clear. `radio::covers::logo_cover` is that branch's one remaining home.
  - Browse rebuilds **without hopping the event loop** — `invoke_from_event_loop` posts even from
    the UI thread, and a redraw winning that race paints an empty grid. It keeps its card model
    across the leave, which is why the covers under it are shrunk rather than dropped and why it
    still owes a `mark_dirty()` on leave and a wire-time `mark_dirty()` while hidden, having no
    enter-time fetch.
  - **The Rust half is `grid_prewarm::grid_cover(path)`** — one tier, one lookup, no branch. Reach
    for it at a new surface: a copy that grew a **decoding** arm reads correctly and puts the
    UI-thread decode straight back, which is the one thing it will not do.

- **Cache cap via `grid_prewarm::cover_cap_for_window(app)`** — one band for every grid, they all
  draw the same card. Derives its cap from the **window's** own logical size against
  `GridGeometry`'s pitches, deliberately not the monitor's: the monitor caps against a screen the
  window may occupy a corner of, and asking costs a `with_winit_window` round trip that answers
  `None` for the whole window-less boot. A zero extent lands on `GRID_COVER_CAP_FALLBACK`, which
  the module owns now that one tier takes it.
  - **The pitches have to be the grid's, not a footprint estimate**: the cap has to come out at or
    above the cards actually mounted, or the scheduling lookup leaves the overflow on placeholders
    until a scroll. Margin comes from measuring the window rather than the grid's own box;
    `grid_prewarm_tests` pins the two against each other.
  - **The ceiling is bytes, not entries.** A decoded buffer costs the square of the tier size, so
    one entry count is two different budgets — and it was wrong the expensive way round, a large
    logical desktop being by construction a 1× one, where the buffers are a fifth the size and the
    same bytes buy several times the entries. **It is a ceiling on a tier that has not retuned**:
    the cap is derived from the size being moved *to* while `resize` trims by entry count, and a
    retune leaves the old buffers in place on purpose (the release/retune asymmetry above), so a
    drag across a step can hold two sizes at once until each is replaced or evicted. Bounded at
    one entry per path, and a section leave shrinks every entry to `PROXY_COVER_DIM` rather than
    dropping it, so the idle case is that side length squared per cover and not the tier's.
  - **Read after `app.show()` and again on every resize**, through `WindowChrome.display-changed`
    off the winit `Resized` filter. Read once, a cap sized for the launch window is one a later
    maximize overruns. It sets the cap exactly rather than growing it: a smaller window really
    does draw fewer cards.

- **Decode size via `grid_prewarm::cover_size_for_window(app)`, and it is derived from the card,
  not stepped off the scale factor.** `GridGeometry` packs toward `min-card-w`, so a card is
  *smallest* on the panels mounting the most of them: the two constants this replaced sized the
  wide case generously and doubled it for `HiDPI`, which made the displays paying for the most
  buffers hold each one at roughly twice the pixels it drew. `widest_card × scale` is the whole
  question, clamped to `MAX_CARD_W` before the scale and to `STORE_MAX_DIM` after it, the second
  being what caps a single-column panel's sharpness once a scale factor is on it.
  - **The bound, never the card itself.** `card-w` sweeps `min-card-w` up to
    `min-card-w + pitch/cols` inside *every* column band, so a tier tracking it crosses a step
    boundary twice a band — and `display-changed` re-derives on every winit `Resized`, so each
    crossing re-decodes every grid's visible covers mid-drag, on one pool every tier shares.
    Sizing to the band's upper bound is what makes the answer flat across a drag rather than
    merely close, and the step is what collapses the column counts a desktop passes through into
    a handful of sizes. What a crossing does not cost is the covers themselves; that half is the
    release/retune asymmetry above.
  - **The window is not the body, and `BODY_CHROME_W` is the gap.** The sidebar and the queue
    panel either side of the body are the user's to drag, and the panel to open, a range no window
    measurement sees, so the estimate assumes both at their *widest*: over-subtracting costs a tier one
    step too large, under-subtracting puts every card on an upscale, and `FemtoVG` minifies
    bilinear with no mipmaps.
  - `GRID_COVER_FALLBACK` is what a tier is *built* at, before any geometry exists. A fallback,
    not a tier size, but sized for the run where the deferred retune never schedules and it stays
    the live tier.

- **The cap and the size are read together**, in `grid_prewarm::tune_for_display` — two halves of
  one budget, and both are answers about the display. The derived size replaced a `448` copied
  into five files, each justifying it in its own doc comment off the same claim that flex-filled
  cards "run well past 260 px". They don't: `GridGeometry` packs toward `min-card-w`, so a card
  is **largest in a narrow panel**, where `max-card-w` is now what stops it, and lands near 190 px
  on a wide one. A tier spelling its own size is the thing to reach for this instead of. Needs no
  winit round trip, the scale factor being Slint's own, and shares the cap's zero-extent bail.
  `cover_thumbs::row_cover_size` is the row tier's twin, applied in the same `display-changed`
  retune. The queue panel's tier takes the row tier's size through its backing rather than being
  wired a second time.

- **Prewarm path dedup via `grid_prewarm::unique_artwork_paths(paths, cap)`**, first-seen-ordered
  and non-empty. **Every prewarm site goes through it**, the grid owning only the projection
  (`CardGrid::artwork`, Browse's `first_screenful_paths`); don't lean on `prewarm`'s internal dedup as a reason to skip it, the two sites that
  did each grew a divergent copy. **`cap` bounds kept *paths*, not input items** — capping the
  input is what stops a detail over a 20 000-track genre allocating a `PathBuf` per unique cover to
  keep 512, and `prewarm`'s own `.take(cap)` sits **after** its already-cached filter, so over a
  partly warm tier an uncapped call decodes a full capacity from anywhere in the list, evicting the
  visible prefix to do it.

- **Artist Detail's Albums strip has nothing to borrow any more** — it draws from the one grid
  tier like every other card, so the handle that used to hold Albums' is gone and with it the
  release on both Artists section-leave and `on_close_detail`. Collapsing the Albums sub-section
  still hands the pixels back, the scroller being unmounted and nothing querying them.

## Releasing what the UI pins

- **Detail-close releases global Image properties.** `detail_view::release_detail_hero_images`
  resets `cover` + `blur-img-a/b`, clears `has-blur`, and re-solves the two shared globals,
  alongside `clear_detail` + `release_detail_artwork`. Without it `SharedPixelBuffer` Arcs pin the
  cover tile and **both** blur slots, on the heap and again as Mesa textures. It runs on each
  view's **section leave**, gated per the hero-teardown rules above.

- **A close doesn't run it, and that is the contract the morph forced.** Every fact the band paints
  is a ternary over the detail id, so clearing on the frame the id does leaves the band spending
  its whole collapse painting a fallback glyph over a reset gradient. The teardown moves onto
  **`hero-collapsed`**, fired by a `dur-spatial` `Timer` in the band (the `Dialog.closed()` shape),
  armed and cancelled by the one edge that drives the morph so a re-drill can't land the previous
  hero's teardown on the new one. `release_collapsed_hero` asks **all three** image-bearing globals
  — the band can't say which closed and doesn't need to — but hands each back only **on its own
  id**. The backstop is the page's own teardown, a nav away mid-morph killing the timer. The band's
  own half of the deal, latching the arm it paints so the collapse fades the banner rather than a
  placeholder, is `library-tab-band.slint`'s.

- **Dialog-close releases Image properties + scalar state via exactly one handler.**
  `Dialog.closed()` fires once close-anim `t` returns to ~0, and there is **one** `on_closed`
  registration in the tree. It does both halves: `invoke_closed_teardown()` (the Slint side;
  restore `confirm-label` to `"OK"`, don't clear to `""`), then `current_artwork` **and**
  `TagEditor.cover` reset to `Image::default()` — the two `image`-typed globals, which have no
  Slint default literal — plus `allocator::trim`. **The teardown is a `public function`, not a
  callback body**: a callback has a single handler slot, so a default `closed => { … }` body is
  installed at construction and then silently replaced by the Rust `on_closed`.
  `CompositeScroll.reset()` takes this shape for the same reason. Do **not** clear in
  `accepted`/`cancelled` (unmounts the body mid-fade), and do **not** re-register `closed` from
  inside the handler (`Callback::call` `take()`s it and asserts). A new dialog kind pinning an
  image extends the single `on_closed`.

## Popups, native dialogs, input

- **PopupWindow auto-dismiss on OS focus loss** is `FocusLossWatcher`, mounted ungated inside the
  popup's own `PopupWindow` (or through `MenuSurface`, or `ContextMenuPopup` for the three
  right-click menus with a flyout), whose body Slint builds in `show()` and drops in `close()`, so
  only the open popup has a live watcher. A trigger lights off its own popup's `is-open`, which
  every close path resets, Escape and an outside click included.
  `crates/melodia/tests/popup_dismiss.rs` walks every popup for the watcher, a component that
  `inherits PopupWindow` being read through its own body.

- **An empty region inside a popup takes `PopupDismissCatcher`**, never a bare `TouchArea`: the
  room a menu holds for its flyout, the volume popups' strips. Its comment argues which button
  closes on which edge, and a bare one gets the right button wrong.

- **Native dialogs (rfd) — always through `ui::file_dialog::parented(&weak, title)`.** The helper
  owns the `weak.upgrade()` + `.set_parent(…)` half; the caller chains its own
  `add_filter`/`pick_*`/`save_file`. **Never build one inline**, and the reason is that the bug is
  invisible here: without the parent it z-orders behind Melodia on Windows and macOS, while Linux's
  XDG portal parents OS-side regardless. Held by a test that **walks every crate's `src/`** rather than naming
  the five sites, and carries a caller floor so a caller that stops opening a dialog trips it too.

- **Keyboard shortcuts** — the root `FocusScope` is `ShortcutScope`, which owns every binding and
  takes the whole main layout as its children. The bindings are in that file; what isn't visible
  there:
  - **Ten `changed` mirrors regrab focus**, Slint not handing focus back to an ancestor scope when
    the focused item is destroyed, so a view unmounting with focus inside it leaves shortcuts dead
    until the next click. Nine are one per branch selector of an `if` chain that mounts a view —
    the same reason `CompositeScroll.reset()` needs its five, so a new always-mounted mirror
    unmounting a *composite* view owes both (this set is the wider). The tenth,
    `Queue.keys-owned || Queue.floating`, is not about destruction: the queue panel mounts nothing,
    but *nothing moves focus onto the scope* when a press hands the queue its keys or the overlay
    opens, so without the grab a filter `SearchBar` keeps the keyboard and eats the `a`. **It grabs
    on the rising edge only**: the press that takes the keys away is one Slint is about to hand a
    field, and a grab there lands after it.
  - Non-Esc gates on `!Dialog.open`; typed keys in TextInputs never reach root. Q is gated on
    `!MiniPlayer.active`, the miniplayer having no panel to open and a narrow window opening the
    overlay, and Ctrl+B on `!Nav.sidebar-yields`, a forced rail having nothing to expand into.
  - **`ShortcutScope` is the only `FocusScope` in the tree, and the queue is why that's worth
    stating.** A `FocusScope` takes focus on **mouse press**, never on mount, so the old sheet's
    nested scope left Esc and Ctrl+A dead on a sheet opened with `Q`. Esc, Ctrl+A and Delete (with
    Backspace, the key a Mac labels delete) sit at the root, gated on `queue-keys`
    (`Queue.shown && (Queue.keys-owned || Queue.floating)`), which also fixes the priority: Esc
    reaches the *dialog* first. A second scope reintroduces all three problems; put new key
    handling here.
  - **Whether the docked panel's keys are the queue's is decided at the press, in the winit
    filter.** The panel sits beside the page, so its keys are live only while it was the last thing
    pressed. A hover sentinel round its content publishes `Queue.pointer-in`, and
    `winit_filter::claim_queue_keys` copies that into `Queue.keys-owned` on every press, ahead of
    Slint's dispatch, and only a shown panel can claim them. Slint alone can't answer it: a
    `TouchArea` never sees a press a child took, and a nested `FocusScope` is the bullet above.
    **The sentinel takes the wheel its children leave**, or a tick nothing under it scrolled would
    clear its hover until the pointer moved, and a click in place after it would claim nothing
    (`slint-pitfalls.md`'s `has-hover` entry).
    **A row's menu hands the keys back as it closes**, its popup having taken the sentinel's hover,
    so the press on an entry read as one elsewhere. The overlay owns the keys while it is up, a
    press or not. With the keys, Esc clears the queue's picks first; then the overlay closes, and
    the docked panel never does, the press falling through to the arms below.

- **Animated view transitions** — content branches mount via `ViewTransition`: enter-only fade +
  32 px axis slide, no exit anim, Slint `if` destroying the outgoing branch instantly. Direction on
  `Nav.pending-enter-from`, written **synchronously just before** flipping the `if` via
  `ui/nav_transition.rs`.
  - **An unwritten edge is not a default — it is whatever the last navigation left in the global**,
    and `mark_drill_back` fires on every detail close, so the value sitting there is reliably
    `left`. *Every* Slint-side mount writes its own, `above` by construction, the two non-lateral
    directions being Rust's; the pin **walks** the tree. The miniplayer's mark is for the Now Playing
    close it performs. The full UI it rebuilds on the way *back* mounts under the suppression below
    and never reads the edge.
  - **Two inputs turn parts of it off and answer different questions**: `enabled: false` is "does
    this view animate at all", `slide: false` is "is anything *else* already translating it" —
    fade, don't move. Both default on, so a mount that owns its motion says nothing.
  - **The launch mount has a third off-switch, and it is a global rather than an input.**
    `Nav.suppress-enter-animation` is declared raised and handed back by the first mount that
    settles, so the window opens on its content and every later navigation still slides.
    **Declared, never written from Rust before the show**: `AppWindow::new()` already instantiates
    the branch for the declared nav index, whose `init` reads its opacity at 0 under a lowered flag,
    so a raise from `boot::ui_setup` faded that page in anyway. `app.show()` instantiates again
    after the nav index is hydrated, still ahead of any `Timer`, so a launch elsewhere mounts its
    own page under the flag too. A **live read inside `settled`**, and the hand-back **sits one
    statement after `shown` flips**: a clear landing on a frame where `shown` is still false fades
    the settled page straight back out, which is also why the hand-back can't be Rust's. Gated on
    **`enabled`**, since a nested body mounted at boot runs the same `Timer` and would otherwise
    drop the flag for the page above it. Pinned by `ui::startup_motion_tests`.
    **A restored detail arms no body fade either**, on My Library or Radio
    (`components/detail-arrival.slint`), its layer otherwise stacking on the launch's first frames.
    **`MiniPlayerSwitch`'s swap timer raises the same flag** on the way out of the miniplayer, ahead
    of the flip that rebuilds the full UI. That rebuild is already crossfading, and a page sliding in
    under it put a second full-window layer over the most expensive frames of the swap. The setting
    has no say there.
  - **A page with sub-views nests a second one and must disarm it at mount**, the page's own enter
    still playing when the first tab body mounts and a horizontal slide composed with a fade-down
    reading as a diagonal. The host arms it in the tab bar's `selected` handler; starting `false`
    is what makes the page re-disarm for free, being rebuilt on every entry. Direction comes off
    `bar.previous-index`, so the host keeps no tab state and needs no mount seed.
  - **My Library gates its direction on `morphing` rather than dropping it**, being the one page
    with a *third* animation — its own band's height, which moves `body.y` by the whole distance
    between the two floors every frame. All nine branches carry `enabled: root.body-anim-armed`
    and `slide: !band.morphing`, and none reads `Nav.pending-enter-from`; the five **tab** bodies
    take `enter-from: band.tab-enter-from` (the bar's own left/right, forwarded off the band as on
    `MosaicTabHero`) and the four **details** keep `above`, a drill not being a move along the bar.
    So a tab pick with the band still slides like the sibling pages, and anything the band morphs
    through is a cross-fade with both offsets zeroed — which is what makes `slide` the load-bearing
    line rather than the fixed axis it replaced. **`morphing` is
    `hero-t != (detail-open ? 1.0 : 0.0)`** — deliberately not `hero-t > 0`, which is `false` on a
    drill-in's first frame, the one frame the answer must be `true`; same-axis is not enough to
    skip it, the body's 32 px being made of a curve that disagrees with the morph's, so the sum
    runs the wrong way before it turns. **`body-anim-armed`** is written from `DetailArrival.arrived`
    **and** the `watched-tab-idx` mirror: the seed answers "a tab has been picked in this mount",
    and the two arrivals that aren't picks each need one handler, a cross-tab drill writing the id
    and moving the tab in *one* tick so `detail-open` never transitions.

## Settings and nav wiring

- **Section-visibility hooks go through `SectionActiveGate`**, mounted in `app-window.slint` in
  nav-index order — **once per section, and once more per tab only where a tab leave has to be a
  section leave**. It owns the predicate (index selected *and* neither Now Playing nor the
  miniplayer covering it) and fires `active-changed(bool)` into that section's Rust hook. **It must
  mount on the always-alive `AppWindow` root, not inside the view**: the *leaving* transition is
  the edge that matters and a view cannot observe its own. A new section adds one mount, not
  another copy of the predicate; pass `mini-active: mini-switch.active`, the live derivation, which
  leads the global by a beat.
  - **A tab switch can be a section switch, and the gate is where that becomes true rather than
    each view.** `tab-index`/`current-tab` are an optional sub-predicate defaulting `-1`, so the
    tabless mounts are untouched. My Library mounts five at `index: 3`, so a tab leave fires the
    departing view's existing hook and **every lifecycle path works unchanged** — hence the page
    never needed the cold-gate machinery the curated pages carried, the entering tab's own fetch
    prewarming before it writes rows. A `0` default on either property silently deactivates every
    section.
  - **What the five cannot answer is the page's own leave** — only the *mounted* tab's fires — and
    **the answer is not a sixth mount**, which is the obvious shape and compiles: a gate fires on
    transitions of *its own* predicate, that predicate is already false while Now Playing covers
    the band, and `sidebar.slint` clears `now-playing-open` and writes the new index in **one**
    handler, so the leave that matters most goes false → false. `page-active-changed` rides
    `changed watched-nav-idx` instead, which also makes "covered" and "left" different events.
  - **A mirror fires on every nav change where a gate fired only on its own predicate, so the hook
    owes a latch** — the half a later edit reads as redundant. A `changed` handler cannot ask which
    index it moved *from*, so unlatched the teardown also runs on Favorites → Recently Played and
    clears the `HeroBackdrop` and `HeroChips` those two bands publish into. The latch is
    a `Cell<bool>` in Rust, not a `prev-nav-idx` mirror: `Nav.selected-index` is declared `: 3` and
    the persisted index lands after `AppWindow::new()`, so a Slint seed is either a constant firing
    one spurious teardown per boot or a binding that re-evaluates to the *new* index inside the
    handler.
  - **The gate fires on transitions only, so each section's synchronous `section_active` shadow
    must be seeded correctly — a boot-ordering constraint, not a `wire_*` detail.** Every `wire_*`
    seeds by reading `Nav.selected-index`, so `install_views` writes the **persisted nav index
    before `wire_all`**, and `seed_tab` beside it: the five My Library seeds read `tab_is_mounted`,
    so a tab seeded after `wire_all` leaves all five answering for the declared `0`.
  - **Everything above about "the five" is My Library's history, not what a tabbed page owes.** Its
    tabs were five sidebar sections with five hooks, and five gates are what keeps those hooks
    reachable. Favorites' three tabs, Recently Played's two and Radio's three each mount **one**
    gate and share a handle, so their seed reads the nav index and never the tab. For Radio that is
    load-bearing rather than economical: a per-tab gate makes a glance at the kept stations a
    section leave, handing back a directory answer bought with a network round trip. **Ask what the
    leave hands back before mounting a second gate.**
  - **What decides whether a wrongly-seeded gate ever corrects itself is the tracker's baseline.**
    `ChangeTracker::init` evaluates inside `AppWindow::new()` and adopts the result **silently** —
    it never calls the notify half, and `init_delayed` appears nowhere in the generated tree. So
    the boot reading becomes the baseline, and a gate whose baseline already equals the value it
    settles on has no edge left to deliver. With `mini-switch.active` true on that pass (width 0
    until the geometry restore) every gate baselines `false`: **the restored section
    self-corrects**, while a section seeded `true` against a `false` baseline **stays wrongly
    active all session** — not a cosmetic stale tier, `install_library_changed_refresher` then
    taking its ungated arm and re-fetching the whole library per song.
  - **That same 0×0 pass is a size reading to `MiniPlayerSwitch` itself.** `active` has to keep
    answering `true` there — the baseline above is built on it — so the first real layout arrives
    at `changed watched-active` looking exactly like a swap out of miniplayer mode, and ran the
    shell's 100 ms fade-out → swap → fade-in on every launch with nothing mounted to fade *to*.
    The guard is **`render-active != watched-active` on the swap**, not a latch and not anything
    on the predicate: `update_timers_and_animations` runs `maybe_activate_timers` *before*
    `run_change_handlers`, and the geometry restore lands before `app.show()`, so on the loop's
    first pump the seed `Timer` has already settled `render-active` by the time this handler
    fires — any latch either of them sets, this handler finds closed. Asking whether the mounted
    branch has to change is the same question the fade exists to answer, and it holds either way
    round. Pinned by `ui::startup_motion_tests`.

- **Nav state persistence keyed by view-id**, all in `views.json`. Sidebar nav does **not** reset
  detail ids; only the back button does. Adding a detail = a `view_id::*` const + open/close
  `set_last_detail_id` + a seed fn. **A persisted view flag is an int/string/map, not a bool** —
  `ViewStateData` sat at clippy's `struct_excessive_bools` cap, and each tab index sidesteps it by
  being an index rather than a booleans-per-section set. **`browse_view_mode` is where it bites
  hardest**, being the one genuinely binary flag: as a clamped `i32` it costs nothing today and
  buys a third presentation for free, where a `bool` would put the struct back on the cap *and*
  make the widening a migration. Dropping a field is safe on shipped installs, serde ignoring
  unknown keys.

- **Audio installers seed from `ui::settings_bind::read_or_default`** — the three each opened with
  the same `match read_settings(…)`, spelling their defaults out again in the error arm. Its
  sibling `toggle_binding` owns the apply-then-persist shape. **Not every installer wants it**:
  `playback_settings`/`file_watching`/`updater_settings` use `if let Ok(s)` deliberately, leaving
  the **Slint-declared** defaults in place on a read failure rather than re-deriving them in Rust.

### `TabBar`

**Mounted once — by `components/hero/tab-search-header.slint`'s `TabSearchHeader`**, the row
carrying the bar and the filter box together. **Reach for the *row*, not the bar**: a page wanting
tabs beside a filter wants all five of the fixes in it — a `page-w` mirror written from
`changed width`, the 1 ms mount `Timer` re-running it, a seed at the row's own floor, the
`search-w`/`avail-width` budget, and a top-layer tooltip frame after the scroll chrome.
Data-agnostic: parallel `labels`/`icons`, `avail-width`, `selected-index`, `selected(int)`.

- **It publishes `previous-index` because a cell writes `selected-index` before it emits
  `selected`** — it has to, the `<=>` being what carries the pick out. So a host's handler runs
  already reading the tab just picked, and the outgoing one is recoverable nowhere else. Same
  ordering is why a pick writes rows through a non-hopping apply (`apply_filtered_tracks_now`). A
  host wanting the old
  index reaches for `previous-index`, never a mirror seeded at mount.

- **Every brush it paints with is a defaulted `in property`**, which is what lets it sit on a hero.
  The defaults are a flat page's; the two bands hand over `HeroBackdrop` tiers for **all four** —
  **through four locally eased mirrors, never raw**, the bar being unable to ease a brush and the
  solve landing late either way. `MosaicTabHero`'s **own title takes `hero-label` too**, painting
  that same tier, so easing only the bar leaves the two halves of one text tier arriving apart.
  **`active-color` is the one worth spelling out**, being one input for the selected label, its
  FILL=1 icon *and* the underline: `Theme.accent` carries **no contrast floor** against a pinned
  band — Latte's mauve lands near 1.7:1, under even the 3:1 non-text bar — where
  `HeroBackdrop.chrome` clears 3:1 whatever the cover, by a solve on the blur and by being the
  theme's own ink on the aurora. The trade before "fixing" it back: `chrome` is the 3:1 tier and
  `on-backdrop` the 4.5:1 one, so the honest repair is a second input separating label from
  indicator, not a token swap at the call site.

- **The cells are equal width, sized to the widest tab, and that is what makes the underline
  arithmetic** — a `for` loop exposes no per-tab element to read a position off, so a content-sized
  row would snapshot each tab's geometry on selection and go stale on every resize. It is also why
  **the selected label can't be heavier**, the cell having been measured against one weight.

- **`compact` is measured, not thresholded** (a hidden ruler of real cells, so measured and drawn
  shapes can't drift and the running locale's width is what counts), and **the decision had to move
  into the bar** — a component may read `preferred-width` off its own descendant but not off a
  child component's internals, so the host hands over a *mirrored* `avail-width` and reads the
  verdict back. **The seed is the header row's own floor, not a plausible page width**: seeded wide
  the bar draws five full-width tabs into a panel that can't seat them, where seeded at the floor
  it draws the icons it would have drawn anyway and widens once — and a miniplayer → full swap
  remounts the page narrower than any plausible guess. **The seed obliges the mount `Timer`**,
  `changed` not firing when the first layout settles directly on the final value.

- **Sharing the row means the two widths are budgeted, not independent**, and **the filter yields
  before the tabs do** — tabs are navigation, search only refines what a tab already shows. Both
  ends of the clamp read a published floor (`compact-w`, `min-w`) rather than a restated literal,
  which keeps each from drifting and makes the row's minimum a sum the page can check. **Past that
  sum the box folds to its glyph** rather than drawing past its cell, and opens across the row
  over the bar while it holds focus or text, a tab pick clearing the filter anyway. Both ease on
  the bar's own morph, written from `changed` for the bar's reason. It is one `SearchBar`
  throughout, so a resize hands no focus across, which is where it parts from Settings' two boxes.
  The fold and the open state are `tab-search-header.slint`'s.
  **`tip-x`/`tip-y` carry the bar's own `absolute-position` offset inside the row**, the half a
  host cannot supply — dropped, the pill lands a whole `lead-w` left of the tab it names, on the
  one mount with a leading slot.

- **The Rust half is one function: `clamp_tab(tab, tab_count)`.** Every page persisting an index
  clamps on *read* through it, against its global's own count (Settings' `category-count` and
  Browse's `view-mode-count` included) — the bar can only produce a valid index, but a file left
  by a build with more tabs would select a branch that mounts nothing. The component's two
  source-level invariants (the written-not-bound `compact-t`, the root's `clip`) are pinned there
  too, no host owning the file.

- **The rest is one file's internals** — the latched threshold, the two eased sources, the label
  collapsing onto its icon, the `min`/`preferred`/`max-width` split. `tab-bar.slint` and
  `ui::tab_bar::tests` carry them.

### `LibraryTabBand`

**The band that *morphs*.** Idle it is a flat pane carrying the tab bar, the shared filter box and
the mounted tab's count; with a detail open it grows into that entity's hero at `MosaicTabHero`'s
`hero-height` exactly — the same formula, so the two bands agree on what a hero is. One animated
`hero-t` drives the height, backdrop reveal, back slot, palette and count exit. **The two bands are
siblings, not one parameterised component**, differing in which artwork square they draw (a
`MosaicHeroTile`, whose fallback is the page's own Material Symbols glyph, against an
`ArtworkImage`, whose fallback is an SVG asset) and in fixed-height-versus-morph, neither of which
Slint can abstract over — so the header-row fixes are *ported
verbatim* and `ui::library_tab_band_tests` holds the copy to a contract a copy is exactly how you
lose. **Two things stay at each host**: the per-tab `ActionPill` rows (`@children`, in a slot
floating over the foot of the meta column, which reserves the row's height so the chip strip's
two-row cap is measured against the same slack) and the tooltip frame.

- **The height is a `min`/`preferred`/`max-height` split with `clip: true`**, never a bound
  `height` — the animated-root-dimension pitfall one axis over. **`hero-t` is *written* from
  `changed detail-open` and only seeded by its binding**, so a page entered with a detail already
  open lands at hero height instead of growing into it.

- **What may hang off an `if` is decided by change trackers, not taste.** Everything tracker-free
  rides one `hero-shown: detail-open || hero-t > 0`, and a piece carrying a tracker over a layout
  property the morph re-dirties every frame stays mounted and fades by brush alpha, dropping its
  branch being a panic. The chip strip sits outside the gate and fades that way, though it carries
  no tracker and could join it.

- **Nothing carrying content answers `opacity`** — the layer is sized to child *geometry* and a
  text run's ink leaves its line box. Fade through the brush instead, via **`transparentize`, which
  multiplies alpha — never `with-alpha`, which sets it**; where the element has brushes feeding its
  own `animate` blocks, make it satisfy `need_layer`'s second bail rather than folding the fade in.
  Full argument in `slint-pitfalls.md`.

- **The pill row's fit is the band's, and the width it weighs is the host's.** The slot spans
  only the room beside the cover, so a row that can't fit runs off under the clip rather than over
  the artwork, and short of that room `pills-compact-t` closes every labelled pill onto its glyph,
  eased on the tab bar's curve and landed while the band morphs. The labelled width can't come off
  the drawn row, which stops reporting it once its labels close, so each host mounts a hidden
  second copy of its pill row outside every layout (`pill-ruler`) and hands the band its
  `preferred-width`: `TabBar`'s ruler, one level up. **A pill in a clipping
  cell keeps its label unless its tooltip is drawn from the top layer**, an in-tree one landing
  under the clip. My Library's Playlists row publishes its anchors and compacts; Radio's Favorites
  row doesn't, and keeps its label.

- **Everything else here is one file's internals** — the two morph curves, the count line's anchor
  and axis flip, the back disc's doubled bias. `library-tab-band.slint` and
  `ui::library_tab_band_tests` carry them; read those before changing the morph.

## The three tabbed pages

Favorites (nav 2), Recently Played (nav 8) and My Library (nav 3) share one contract. **Read this
block first; each page's section below is deltas only.** The nav-index map is in the root
`CLAUDE.md`.

- **`<Page>.tab-count` is the sole definition of how many tabs exist.** Rust clamps the persisted
  index through `clamp_tab`, never its own const, and a per-page test `include_str!`s the `.slint`
  to pin the number against the `tab-*` constants, the body branches and both inline `@tr` arrays.

- **Load only the mounted tab.** Sub-views are mutually exclusive `if`s, so anything loaded for an
  unmounted tab reaches nothing. A tick that reaches the page with another tab up marks that tab
  dirty, and the pick that mounts it reloads.

- **Counts hold `UNFETCHED_COUNT` (`-1`) until fetched**, and the section leave puts them back
  beside the model clears. A count outliving its model suppresses an empty state over an emptied
  model; resetting to `0` asserts "nothing here" for the length of the re-fetch. `-1` matches
  neither `== 0` nor `> 0`, so every existing gate keeps working — the one reader splitting on both
  is `MosaicHeroTile`, whose two mounts clamp it. The five library counts are interpolated into
  gettext plurals,
  so each is read through a `>= 0` ternary (a ternary, not an `if`, so the `Text` keeps its slot).
  **Tracks is the exception, and the exception is the rule read precisely** — what obliges the
  rewind is the leave dropping the rows, and its leave doesn't. **Rewind if and only if you clear,
  and if you clear, mark dirty.**

- **A pick writes no count.** The entering tab paints from what it already holds: the curated
  grids and Favorites' Songs keep their lists across a pick, and Recently Played's capped Songs
  list rebuilds from its cache on the pick's own tick. A reload the tab owes refreshes behind it,
  under the count its list was shown with. **The one pick a kept list can't answer is leaving it
  filtered**: the pick clears the shared needle, so the tab being left is marked dirty on the way
  out. My Library reads around this, its tab leave being a section leave.

- **A gated load owes three things**, each a bug on the way in: the loading branch **consumes**
  its dirty flag (seeded `true`, else a boot onto that tab loads twice); `release_section_state`
  **re-arms** it beside the wipe rather than leaving it to the leave's `mark_dirty` two files
  away; and a load that stores nothing re-arms it, the pick having consumed the flag *before*
  asking. Songs re-arms on a failed query or a leave landing mid-flight (Favorites' through
  `PagedTrackList::load_dropped`), a grid through `CardGrid::load_failed`, the leave's revoke
  covering its other half.

- **Section guards sit *after* the slow part** — before it the leave hasn't happened yet, so a
  Songs load that `.await`s a cover prewarm asks twice. A grid's load asks its ticket instead,
  which the leave revokes. Every store `release_section_state` wipes goes under the gate.

- **A tab pick can't await a prewarm the way a load does**, `TabBar` writing `selected-index`
  before it emits `selected`. Rows a pick builds go in through the **non-hopping**
  `apply_filtered_tracks_now`: `invoke_from_event_loop` posts even from the UI thread, and a redraw
  winning that race paints a `TrackList` of headers over an emptied model.

- **Covers ride one tier that no leave clears**, so a pick paints whatever the tab painted last
  visit and the prewarm sharpens it. `covers-generation` is the repaint token and nothing else
  here — see Covers.

- **A reload repaints only what moved.** `PagedRows::show` keeps every cached row the new keyset
  still holds and a re-read repaints only the cards whose data changed, so a `stats_changed` tick
  over an unchanged ranking costs a query and no rebuild. That is the job a content hash per tab
  used to do; nothing here should hash a tab's contents again.

- **A leave owes `mark_dirty()` for exactly what it hands back** — a tier, a model, a count.
  Tracks' leave releases nothing, so it owes none.

- Shared helpers: `ui::tab_bar::{clamp_tab, UNFETCHED_COUNT}`, `ui::card_grid` over
  `ui::paged_rows::grid::PagedGrid`, `ui::most_played` (the card and rows both Most Played grids
  draw), `ui::paged_rows` with the `track_rows::TrackRows` both Songs tabs and the four details
  show and the `PagedTrackList` path that loads them,
  `ui::grid_prewarm::{tier, grid_cover, prewarm, prewarm_off_thread, hand_back_covers}`,
  `ui::mosaic_hero::{compose_off_thread, MosaicGuard}`.

### Per-page deltas

Each page's own tree is the reference for how it fills; what follows is only the decisions a later
edit would otherwise reverse.

- **Favorites** (three tabs) — `refresh_hero` stays **ungated**, answering the count, running time
  and collage the band states on all three tabs. **All three tabs sort and filter in SQL and keep
  their lists across a pick**, a keyset and a few hundred cached rows being nothing to hand back,
  so each carries a dirty flag of its own (`FavoritesUi::mark_tab_dirty`). **A tab pick warms
  `GRID_PREWARM_AHEAD` covers, not the tier's capacity**: the ones the entering grid's last load
  opened on, warming everything evicting its own work.

- **Recently Played** (two tabs) — **neither has a sort, and that is a decision rather than an
  omission**: the order **is** the page, so the synthetic-field cycle above is deliberately *not*
  what it took, and there is no sort state at all (nothing on the global, no `ViewSort`, no
  `view_sort` key). `sortable: false` runs `TrackList` → `TrackListHeader` → `HeaderColumn` →
  `HeaderCell` and defaults `true`, so the other eight mounts opt in by omission — **the middle
  links are the ones worth pinning**, since dropping a forward leaves the mount still reading `sortable: false`
  while the page sorts again. **The band states the recency set, not the mounted tab's**, a
  play-count ranking under this banner naming the wrong page. **One needle over two lists bounded
  differently**: the capped Songs list re-walks its cache in memory, and the Most Played grid
  filters in SQL, only while it is the tab on screen, the pick onto it clearing the needle anyway.

- **My Library** (five tabs, drilling into a detail *morphing the band* rather than routing) — a
  tab switch being a section switch is what buys it out of the cover machinery, at the cost of a
  full re-query per pick; the one hook that doesn't come free is the hero teardown, which is what
  `page-active-changed` exists for. **The band's back arrow means "close this detail"**, so a drill
  that started on this page stamps `origin-nav-index` `-1` and the close restores nothing — the tab
  bar names the detail's own tab for the whole visit, and restoring the origin made the arrow
  contradict the bar beside it. Only the `tab` half of `Origin` still earns its keep, as
  `still_current`, the guard stopping a mid-fetch tab move from yanking the user, which the nav
  index alone can't answer with five views on one index. **When a detail is coming, none of the
  navigation is written up front** — it rides a `PendingNav` into `open_*_with`'s `on_applied` hook
  so it lands in the same tick as the id, which obliges *every* path skipping that hook to land the
  navigation itself or do nothing at all. **`on_tab_changed` is the one site that records history**,
  a pick being the only tab move that is the user's own navigation.

## The Settings page

- **A category panel beside one column of cards.** `settings-view.slint` is page chrome only,
  `settings/settings-nav.slint` the panel, `settings/settings-router.slint` the router and
  `views/settings/pages/*.slint` one page per category, each owning its card list *and* an aggregate
  `has-matches`. **Search escapes the categories**: a non-empty query mounts every page at once,
  which is how the flat cross-category list comes back with no filtering logic of its own, and why
  both modes mount the same pages, the card-to-category mapping being stated once.

- **The panel is `TabBar`'s contract turned on its side**: parallel `labels`/`icons` and a
  mirrored `avail-w` in, the `compact` verdict and the rail's tooltip rect out, the pill painted by
  a `TooltipFrame` the view mounts last. Its rows are the sidebar's `NavRow`. The measured width,
  the latch and the morph are `settings-nav.slint`'s.

- **The search box takes two properties**: `search-input` for the `SearchBar`s, `search-query` for
  the sections, joined by a `FilterThrottle`. Mounting every page is what makes a keystroke
  expensive here with no model in sight. Both are cleared together on nav-away and on a category
  pick. `on_matches` memoizes the fold against the raw needle, this being the one `Needle`
  caller that can't hold a folded shadow: it is invoked per *field*, not per pass.
  **Two boxes write `search-input`**, the panel's own and the field the rail's button opens. Both
  stay mounted, each carrying a `changed blur-trigger` tracker an `if` would strand, and
  `settings-view.slint` hands the focus across when the fold swaps them.

- **A new category** is one page file + a `category-*` constant + one router branch and one
  search-mode mount + an entry in each of the panel's two inline arrays + a `SettingsCategory`
  variant with its `category_index` arm + a `category-name:` on every section the page mounts, with
  `category-count` the sole definition of how many there are. The panel's `labels` must stay an
  inline `[@tr("…"), …]` literal in `category-*` order, and `settings_page_tests` holds every count
  to it. **Reordering the categories renumbers what `views.json` holds**, which takes a new key
  rather than a fold; `ViewStateData::settings_category` says why.

- **A link into a category from outside the page is `SettingsPage.open-category`**, the Slint face
  of `settings_page::open_on`, never a callback named for its destination.

- **The category's own name is part of each card's search term**: search escapes the categories,
  so a category name is what a user types, and "Integrations" appears in no card's text otherwise.
  It reaches `row-visible` as a **term of its own**, not a prefix spliced onto each title: the join
  lives in the global instead of a copy per card, and no card can match a substring spanning the
  seam. A mount that forgets `category-name:` still matches its own title, so the page looks right
  and only the category's own query comes up empty.

- **A row's label and description are declared once, as properties the visibility term and the row
  both read.** The section's local `row-visible(label, desc)` is fed `label + " " + desc`, so a pair
  spelled at both sites lets a reworded description stop its own row matching — invisible until
  someone types a word from it. The other half is the one to check on a **cluster**: a toggle whose
  sub-rows hang off its own visibility owes every sub-row's pair as a term (`playback-section`'s
  crossfade `||` chain, `discord-section`'s concatenated haystack), or a sub-row is reachable only
  by scrolling to it. Untranslated brands and SPDX ids take a property too — the drift hazard is
  the double spelling, not the `@tr`. Nothing enforces any of this, and a fresh section is exactly
  where it comes back.

- **The body's geometry is `SettingsPage`'s, and nothing in a card measures.**
  `settings-view.slint` publishes `body-avail`, the column right of the panel, off the width it
  already mirrors (a live `root.width` read feeding a child's size re-enters layout), and the
  global derives `card-w`. Every card spans the column, so that is exact. A control inside a card
  that has to fit a width (the chip groups, the accent grids) wraps at the width it is drawn rather
  than being handed one, so it needs no width mirror, no mount `Timer` and no `parent`
  reach-around.

- **`card-cap` (720 px) caps one column, and the panel and a capped card centre as a group**, so on
  a wide window the panel stays beside the cards rather than at the page's edge. Below the cap the
  column shrinks with the page, and the panel folds to its rail once the body would drop under
  `nav-collapse-body-w`. The two-column flip is gone: with one to three cards per category it
  mostly stranded a lone card off-centre.

- **The panel and the cards start level, and three files hold it.** `settings-view.slint` gives
  both columns the same `pad-lg` above them, `SectionCard` draws its title *inside* the fill (a
  label above it dropped every card's box below the panel's), and `SettingsPage.card-header-h`
  sizes that title band off the panel's row height and inset, which `settings-nav.slint` reads from
  the same global. A heading put back above a card, or a panel metric spelled locally again, breaks
  the line with nothing failing.

- **Every card child that wraps text is measured at `card-w` through a pinned cell.** Each sits
  behind an `if`, its own or its card's, and Slint measures a repeated element's height at the
  width its contents ask for, which no pin on its root reaches (`slint-pitfalls.md`).
  `setting-row.slint` argues the wrapper and `SectionCard` carries the same pair on its body cell;
  a new row shape behind an `if` owes one, or its card is sized for a single line of what wraps.

- **Each card carries the gap under it**, so pages and the router spell no `spacing`: a section
  that filters itself away still holds a layout slot, and a gap between slots would leave a hole
  for every card a search hides.

- **Nothing has a width floor any more**: the content column's `min-width` is gone with the
  horizontal scrollbar that existed to pan it, both row components' labels take `wrap: word-wrap`
  (a no-wrap `Text` reports its full width as its layout *minimum*), and both strips wrap. Don't
  reintroduce the bar — a settings page that needs one is a page with a row that has stopped being
  able to wrap.

- **The wrapping strips are `FlexboxLayout`s over their flat list** (`ChipGroup`, `ColorDotGrid`),
  breaking lines where the real chips stop fitting. Two lines on each root are load-bearing:
  **`min-width: 0px`**, without which the flex floors the card at its widest chip, and a `height`
  bound to the flex's `preferred-height`, without which a host with height to spare spreads the
  lines apart (`slint-pitfalls.md`'s two flex entries). An accent swatch picks its tooltip side
  off its cell's laid-out `y`, so the side follows wherever the flex broke the line.
  **The body itself stays box layouts**: a flex body would measure each card at a width it hasn't
  committed to, and these strips, `SettingRow`'s wrapped labels and the other `Rectangle`-rooted
  controls answer at the width they were drawn, a cycle Slint panics on.

- **A colour that follows something is a switch above its grid, never a swatch in it.** Material
  You Accent and System Border Color both work this way: while one is on, its grid's
  `selected-index` is -1 so nothing is checked, and picking a swatch turns it off, so a colour is
  one click either way. Rust writes the switch and the index together (`accent_picker::seat_accent`,
  `window_border::seat`), which is what keeps the two from reading as picked at once. A mode drawn
  as a swatch is what this replaced: it came and went with the artwork, shifting every index past
  it, and showed a colour it couldn't promise. Both grids draw `Settings.accent-colors`.

- **A one-of-many set on a card picks its control by length.** A short preset set (theme, variant,
  color style) is a `ChipGroup`; a set too long or too ragged to read in order as chips is
  `components/settings/choice-list.slint`'s `ChoiceList`, one row per option, which keeps
  `ChipGroup`'s contract so a mount swaps between the two. The Language card is its mount, and
  **Language is a category of its own** so a reader stranded in a language they can't read finds
  the way back by the panel's `translate` glyph, not by a card at the foot of another page. The
  first-run card keeps chips for the same names, a welcome panel having room for one strip and
  not a page of rows.

- **A slider on a card is `components/settings/setting-slider.slint`'s `SettingSlider`**, never a
  `SliderTrack` and a `Text` laid out by hand. The crossfade duration and the resync hold mount it
  directly, and both effects' preamps through `preamp-slider.slint`'s `PreampSlider`, the one copy
  of the dB mapping, on their cards and in their dialogs alike. `SettingSlider`'s header says what
  stays the host's.

- **A setting with a quick dialog as well as a card shares its controls, never its chrome.** The
  equalizer and ReplayGain each keep a `components/settings/*-controls.slint` holding every control
  bound to their global and a `*Text` global of the words both surfaces print. The Effects cards
  lay those controls out in `SettingRow`s, and the overflow menu's dialogs in `DialogToggleRow` and
  `DialogLabeledRow`. Mounting the card's rows in the dialog is the shortcut that looks like more
  sharing: `SettingRow` measures itself at `card-w`, so the dialog would be sized for the Settings
  column rather than its own. What each surface does while the effect is off is its own call,
  argued in `equalizer-controls.slint`'s header.
