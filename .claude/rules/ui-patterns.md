---
paths:
  - melodia-ui/ui/**/*.slint
  - src/ui/**/*.rs
  - src/boot/**/*.rs
  - src/services/toast.rs
  - melodia-ui/build.rs
---

# UI patterns — the shared components and what already exists

What to reach for instead of building a second one, and the teardown paths that keep the UI
from pinning GPU buffers. This is the "reuse this" file; the things that *build, look right,
and are still wrong* are `.claude/rules/slint-pitfalls.md`, which loads on the same globs and
owns the full argument wherever this file cites it.

A rule rather than a `CLAUDE.md` beside the code, for the reason the root file gives: UI
features cut across two trees (`.slint` under `melodia-ui/ui/`, Rust under `src/ui/`), so a
per-directory file would reach one and silently miss the other.

## Shared components

### Tooltips

- **`Tooltip`** (`components/tooltip.slint`) is a plain absolutely-positioned pill, not a
  `PopupWindow`, so it captures no input and the host keeps its hover. It owns the 500 ms
  reveal delay and the fade; `side: TooltipSide.{above,below,left,right}` picks the edge, and
  it takes `host-width`/`host-height` explicitly because a component root can't reach `parent`.
- **A variant is only half of a side.** The `x`/`y` ternaries fall through to the *centred* arm
  for anything they don't name, so a new side without its own arm puts the pill on the host and
  fades in exactly as before. Pinned by
  `ui::placeholder_tests::the_tooltip_clears_its_host_on_all_four_sides`.
- **Two mount shapes.** In-tree is the default and works wherever nothing clips or overpaints
  the host. **Top-layer** is for hosts whose tooltip lands where Slint paints later: a frame
  declared *after* the occluder, tracking the host's rect via `absolute-position` deltas.
  Reach for `components/tooltip-frame.slint`'s **`TooltipFrame`**, which owns the
  `host-width` wiring; the host still spells the two deltas, those being its own geometry.
- **`app-window.slint`'s `sidebar-tip` is the deliberate exception and should stay one**: its
  `x` comes off the live rail width so the frame rides the collapse animation, it owns a `held`
  latch across the gaps between rail rows, and it is the only frame passing `gap`.
- **A band publishes an anchor and draws no pill** — two rules, only the second shared. Pinned
  by walks rather than lists, since the site that gets this wrong is the one nobody has written
  yet: `no_page_or_shell_mounts_a_bare_tooltip` over `views/` **and `layout/`**, and
  `no_shared_band_draws_its_own_tooltip` over `components/hero/`. `components/` is otherwise
  out, an in-tree mount being the default there.
- **An anchor crossing a component boundary has three answers.** A frame can only read ids in
  its own file. `MosaicTabHero` and `LibraryTabBand` publish `tip-x`/`-y`/`-w`/`-h`/`-label`/
  `-visible` as `out` properties; the sidebar rail publishes onto `Nav`, the boundary it
  crosses being a component the frame's file doesn't contain; and
  `views/my-library/tab-pills.slint` collapses its Playlists action row to a zero-width
  `clip: true` cell rather than `if`-gating it, an id declared inside an `if` being unreadable
  from outside. `Clip` swallows every event outside its empty rect, so the pills are as
  unreachable as an unmounted branch while their ids stay readable.
- **The rail's tooltip needs a hold the tab bar doesn't.** Rail rows sit 4 px apart with
  dividers between some pairs, so a pointer travelling down the rail is over nothing for a
  frame or two and a naive retract restarts the reveal on every row. The frame keeps a `held`
  bool set on a row's enter and cleared by a 150 ms `Timer` only once nothing has taken the
  hover, with `hovered: row-hovered || held`. **`held` extends a tooltip and can't arm one** —
  it is written from a `changed` handler and `changed` doesn't fire on a first evaluation —
  which is why `row-hovered` is read beside it, and why `changed watched-mini-render` clears
  `Nav.sidebar-tip-idx`: the miniplayer swap destroys the rail with no unmount hook, and a
  stale sentinel would arm a pill at a stale anchor with no pointer near it. A new unmount path
  that strands the rail owes the same clear (the `CompositeScroll.reset()` obligation in
  `slint-pitfalls.md` is the same shape).
- **The two volume readouts anchor to a *point*** — the slider thumb's centre — so each mounts
  the pill inside a zero-size `Rectangle` and hands over `0px` for both host dimensions. They
  drive it off **`force-shown` rather than `hovered`**: a value readout has to be up on the
  frame the drag starts, where the 500 ms reveal is for a label you linger for. The trigger
  disc's keyboard HUD takes the same hook. Pinned by
  `ui::placeholder_tests::the_volume_readouts_are_the_shared_tooltip`.

### Pills, chips and sort rows

- **`PillButton`** (`components/action-pill.slint`) — 32 px outer / 28 px chrome,
  `Theme.surface1` hover. `danger: true` keeps that chrome and only tints icon **and** label to
  `Theme.danger` on hover; the filled destructive treatment is `SectionButton`'s, which
  hover-lightens with `Theme.danger.brighter(10%)`, and the two are not one pattern. Compose
  inside `ActionPill` with `PillLabel`/`PillDivider`. `IconButton` for round controls *outside*
  chips.
- **`SelectionPills`** (`components/selection-pills.slint`) — the "{n} selected" label and
  `close` button every list view's `ActionPill` grows once rows are selected. **Mount it behind
  the host's own `if`, never with a count of zero**: the slots are unconditional inside, so an
  unselected list mounts nothing, where a component hiding its own children still occupies a
  cell and claims one `pad-xs` of the pill's spacing. `divider-trails` follows position, not
  taste — the separator sits between this group and whatever shares the pill with it.
  **Playlist Detail's row stays hand-rolled**, its destructive "Remove from playlist" pill
  sitting *between* the count and the close under an `is_smart` gate.
- **`MetaChip` + `MetaChipStrip`** (`components/meta-chip{,-strip}.slint`) — a 26 px stadium
  with a `font-size-sm` label, decorative only: no `TouchArea`, no selected state. The
  *interactive* pill is `components/settings/chip-group.slint`'s `Chip`, and the two are
  deliberately not one component — one states a fact, the other configures something.
- **`PopupSurface`** — every `PopupWindow` body wraps `components/popup-surface.slint`
  (`Theme.crust` fill, 1 px `Theme.surface2` border, `radius-md`, no entry anim). `pill: true`
  for vertical-pill.
- **A surface that floats over the app states its own edge with `Theme.surface2`, never
  `Theme.border`.** All four Catppuccin variants define `border` as literally the same hex as
  `surface0`, so a `surface0` card bordered with it has no rim at all. `Theme.border` is still
  right for an **input field** (`LabeledInput`, `RuleValueInput`, `MultilineInput`), which sits
  on `surface1` and wants a darker outline. The test is not the element's kind: what is the
  fill underneath, and is the edge meant to lift the surface off its background or recess a
  field into one.

**Sort rows.** Pills carry `reserve-sort-slot: true` + `sort-direction` for the trailing 16 px
`arrow_drop_*` slot; never concatenate Unicode `↑`/`↓` into the label.

- **The Rust half is two shared helpers, not a hand-rolled block.**
  `ui::callbacks::next_sort(cur_field, cur_dir, clicked)` decides the pick (same field flips, a
  new one starts ascending), `persist_view_sort(state, view_id, field, dir)` writes it to
  `views.json`, and `persisted_sort` seeds the pills at `wire_*` time. `next_sort` reads
  `cur_dir` the way `SortDir::from_token` does — **only** `"desc"` is descending — where the
  eleven hand-rolled copies tested for `"asc"` and so left an unrecognised token unable to
  reach descending.
- **A view whose natural order is a *synthetic* field owes a third click.** `"position"`
  (Playlist Detail), `"rank"` (Search) and `"recency"` (Recently Played) are orders no header
  cell asks for, so once anything else is clicked they are unreachable — and the pick persists.
  That is not cosmetic where something is *gated* on the natural order: Playlist Detail's
  drag-to-reorder is, so one click on Title retired reordering for the whole install.
  **`next_sort_with_natural(cur, dir, clicked, natural)` is the cycle** — ascending,
  descending, then `natural` — and `next_sort` is now it with `None`. Playlist Detail is the
  only caller. Nothing paints the third state: with the natural field in force no header cell
  matches `active-field`, which is the honest reading of "unsorted". Recently Played answers
  the same question the other way, with `sortable: false` — reach for the cycle only where the
  natural order is one the user can meaningfully leave and come back to.
- **The Slint half is `components/sort-pill-row.slint`'s `SortPillRow`**, the `TabBar` shape
  (parallel `labels`/`fields` arrays plus `request-sort(string)`), mounted at all four pill
  rows — the three in `views/my-library/tab-pills.slint` and Favorites' Artists tab; every
  other sortable surface routes the same pair through a `TrackList` column header. It exists
  for what it makes *unspellable*: the per-pill `reserve-sort-slot`, `sort-direction` ternary
  and `active` comparison could each go missing at one site and look right until the sort
  moved. What the *mount* still owes is the two arrays lining up — a label with no matching
  field reads past the end and sorts by the empty string — so both pins check their lengths
  through the shared `test_support::sort_pill_row_arrays`. `labels` stays an inline
  `[@tr("…"), …]` literal, the usual reason. Pinned by
  `ui::my_library::tests::{the_sort_row_holds_every_per_pill_contract,
  every_sort_pill_asks_for_a_field_the_comparator_knows}` and its `ui::favorites::tests` twin.

### Grids, strips and cards

- **Every grid is `EntityCard` in a virtualized chunked-row `ListView`**: Rust chunks the flat
  list into grid-row structs of N, and **`GridColumnsSync`** computes N from the width and fires
  `columns-changed` so a resize rebuilds the model without touching the database. Detail views
  own no header at all — the four under My Library are bodies, and the banner is the page's own
  `LibraryTabBand` morphing into it.
- **A strip and a grid are different components on purpose.** `HorizontalCardStrip` walks its
  rows in a plain `for` inside a horizontal `ScrollView` — affordable for a capped carousel,
  not for an uncapped page, where every card would be built and every cover requested.
  `components/grid/entity-card-grid.slint`'s **`EntityCardGrid`** is the vertical virtualized
  counterpart over the same `EntityStripRow`. Each of its three instances replaced a capped
  carousel and **dropped that cap on arrival** (`MOST_PLAYED_LIMIT`, `get_most_played`'s
  `LIMIT` and its `.clamp(1, 100)`), because the reason for a cap is the strip's plain `for`.
- **The play-count badge lives on `EntityCard`** (`badge-text`), not on whichever host wants
  it. It renders the count beside a `MaterialIcon { name: "play_arrow" }` rather than a `"▶"`
  in the string: a glyph Vazirmatn lacks pulls a fallback font whose taller metrics define the
  line box and drop the digits off the patched centring. Shares the top-left artwork corner
  with `badge-icon`; no caller sets both.
- **`GridGeometry` and `GridEmptyState` are the two pieces every grid page repeats.**
  `components/grid-geometry.slint` is non-visual and turns `avail-width` + `min-card-w` + `gap`
  + `card-text-h` into `cols`/`card-w`/`card-h`/`row-h` — **feed it the *body*'s width**, whose
  value the layout fixes independently of the cards inside it, so a card sizing itself from it
  is a derivation and not a cycle. My Library derives it once for four mutually exclusive tabs
  and hands the results down. `components/grid/grid-empty-state.slint` is the centred glyph +
  heading + copy block, and **not only for grids** — Browse's three states and Playlist
  Detail's empty list mount it too, which is what its optional
  `action-label`/`action-icon`/`action-clicked()` are for. Those are properties rather than an
  `@children` slot because an empty `@children` still claims a `pad-md` gap and shifts the
  vertical centring.
- **Every grid's Rust half chunks through `ui::grid_rows`**, a grid's `ListView` virtualizing
  by *row*, so the chunking **is** the virtualization boundary. Which entry point depends on
  whether the caller keeps its source: **`chunk_rows(items, columns, card, row)` borrows** and
  is the four entity grids', which project each card out of a `GridData` they keep;
  **`chunk_built_rows(cards, columns, row)` moves** and is the three grid tabs' and Browse's,
  which build a flat `Vec` in the same walk that filters it and then drop it. Reaching for the
  borrowing form with `Clone::clone` as the projection is the trap the split closes — a second
  full pass cloning every card into the chunk about to replace it. `columns` is floored at one
  in both: a grid mid-layout can report zero, and one card per row is a visible wrong where a
  zero-width `chunks` is a panic. The grid tabs take the shorter route,
  `chunk_entity_rows(rows, columns)` + `write_grid(model, rows, label)`; `write_grid` is
  generic over the row type, so Browse's card grid uses it too. Don't re-roll either.
- **`MosaicHeroTile`** (`components/hero/mosaic-hero-tile.slint`) is the 140 px artwork square
  both mosaic heroes draw, taking `count`/`paths`/`empty-glyph` while each view keeps its own
  mosaic tier. **Every brush in it is a `HeroBackdrop` tier and none may become a `Theme.*`
  token, on both arms of the ternary**: with no mosaic the hero is the dark gradient floor
  where a light theme's accent lands dark-on-dark, and the populated fill matters just as much
  because the mosaic's placeholder slots are translucent. Its `request-cover` is `pure`,
  because `CoverMosaic`'s is. Mounted once, by `MosaicTabHero`.

### Hero bands

- **Anything painting on a hero blur reads `HeroBackdrop`, never a `Theme.*` brush.** The six
  hero-bearing views share **one** global, only one hero being mountable at a time.
  `src/ui/hero_backdrop.rs` (`apply`/`apply_gradient`/`reset`) publishes it from the same
  `src/ui/backdrop.rs` solve the Now Playing `np-*` tier runs: measure the decoded blur with
  `luma_p90`, then solve a scrim opacity driving the *composite* into a known dark band. Both
  tiers seed from the hue quantized out of their own blur, `Theme.accent` only when there is no
  artwork to take one from, and both take it as a `backdrop::BackdropSample`. **Producing that
  sample is the *decoder's* job, never the publisher's** — it runs in whichever
  `spawn_blocking` already decoded the blur, the quantize being the heaviest thing on that path
  and `apply` being called from the UI thread.
  - Use **`on-backdrop`** for the title and secondary line, **`on-backdrop-muted`** for
    empty-state copy, **`chrome`** for a placeholder fill or glyph and for a chip's label,
    **`chip-fill`** for the pill behind it — or **`chip-fill-at(fade)`** when the surface is
    morphing, since `with-alpha` *sets* alpha rather than multiplying it. `disc-hover` is the
    third weight of that ladder, named despite its single reader because that reader is
    `AccentDiscButton`, itself the shared surface. `scrim`/`floor-*`/`chrome`/`on-backdrop`/
    `on-backdrop-muted` are `in-out` and written by `write`; `chip-fill` and `tile-edge` stay
    `out`, derived off `chrome` and `on-backdrop`.
  - **`ActionPill`/`SearchBar` inside a hero are the deliberate exception** and stay on
    `Theme.floating-chrome-bg`, still mostly their own surface, so their contents contrast
    against `surface0` rather than the blur. That is only safe because the backdrop is
    *pinned*: a solved scrim holds the composite in a known tone band. A `chrome`-tinted
    placeholder is the opposite case — translucent enough that whatever fills the rectangle
    behind it is most of what its glyph composites against, so that fill has to be a hero token
    too.
  - **`apply_gradient` is the exception at both ends**: Genre Detail seeds from its own
    name-hashed `start_rgb` and keeps its own floor, its stops being already theme-independent.
  - **The floor gradient eases like the scrim above it** (`dur-med`), which shows on exactly
    the heroes with nothing over it — an artwork-less entity, Genre Detail, and the window
    between any hero opening and its decode landing. Safe to `animate` where a tab-bar brush is
    not: `Brush::interpolate` handles gradient↔gradient stop-for-stop and Rust writes
    `floor-start`/`floor-end` discretely. The Now Playing view is the same three layers written
    a second time; both are pinned by `ui::hero_blur_backdrop_tests`, and a third copy of the
    stack owes a third entry in that array.
  - **No layer may ease *out of* a held tier.** On My Library the globals routinely describe a
    hero the band stopped painting several tabs ago (the hold is below), so an eased layer
    bound to them settles on it and interpolates out the moment a hero opens — reported as a
    genre's pink appearing under a playlist. **The cure is to make the idle value honest rather
    than to suppress the animation at the right instant**: each of `LibraryTabBand`'s palette
    mirrors falls back to its own idle half on `root.detail-open`, and the shared floor swaps
    its stops for a transparent pair on a `hero-open` input defaulting to `true`. Slint
    subscribes to a dependency only on the arm it *evaluates*, so this also stops a write while
    the band is flat dirtying either binding. **`detail-open`, not `hero-shown`** — gated on
    the latter they would drain toward the idle value for `dur-med` *after* the collapse ends.
    The scrim stays ungated, carrying almost no chroma at `SCRIM_TONE`. Pinned by
    `ui::library_tab_band_tests::no_hero_tier_outlives_the_banner_it_was_solved_for` and
    `ui::hero_blur_backdrop_tests::the_floors_hero_gate_defaults_to_shown`. The mirror-image
    rule — a leaf may not `animate` a brush its host eases — is in `slint-pitfalls.md`.
  - **`has-cover` is how a host says "not this one".** `my-library-view.slint`'s `cover:`
    ternary has to bind *some* global on every arm, Slint having no empty-`image` literal, and
    `GenreDetail` owns no cover — so the Genre hero painted whichever other detail was open,
    which `seed_detail_from_settings` makes routine on a cold start. `ArtworkImage` takes
    `has-cover` defaulting `true`. The blur quartet needs no equivalent: `HeroBlurBackdrop`
    already gates both slots on `has-blur`, which is exactly what a procedural backdrop is for.

- **A hero may publish into either shared global only while it is the one on screen.**
  `install_views` calls `seed_detail_from_settings` for **all four** detail views
  unconditionally, so a cold start fetches up to four details regardless of which section it is
  restoring and the last to finish would win. The gate is passed to `apply_detail_artwork`
  (guarding the `HeroBackdrop` write **and only that** — the cover and blur slots either side
  are the view's own, and writing those while hidden is what leaves the page ready to paint),
  to `apply_genre_hero`, and to `hero_chips::publish`.
  - **On the four details it is a live `tab_is_mounted(&ui, MyLibraryTab::X)` read *after*
    `on_applied(&ui)`, and both halves are load-bearing.** A section shadow only updates on the
    next frame, so a cross-section drill — which moves the tab from inside the very closure
    that publishes — would answer for the tab being left; and the shadow is the wrong
    *question* besides, going false when Now Playing covers the band. `make_go_to_genre` takes
    `open_genre_with` for this reason rather than hopping the event loop twice. Pinned by
    `ui::hero_backdrop::tests::the_detail_gate_is_the_live_tab_read_after_the_drill_lands`,
    whose second assertion is the one that matters: hoisting the binding above `on_applied`
    compiles, reads correctly, and puts the bug straight back.
  - **Dropping a publish is only free if something later replaces it**, and the first enter
    after boot is where that stops being true — `SectionState::new` starts `dirty: false` so
    the boot pre-fetch wins the first section-enter, but a pre-fetch that ran off-screen filled
    nothing shared. The four detail sections seed the flag themselves at wire time
    (`if !section_active() { mark_dirty() }`), reading `tab_is_mounted` rather than the nav
    index, five sections sharing index 3. Tracks and Browse keep the cheap path, having no hero
    to lose. `publish_favorites` takes the section *handle* and derives the gate itself, the
    stronger shape.

- **The teardown is gated the way the publish is.** On a tabbed page **a leave is not a
  teardown**: nothing clears a detail id on a tab switch, so the band is either collapsing out
  of that banner or one pick away from morphing it back open, and handing the globals back at
  the leave plays a 400 ms exit morph over `ArtworkImage`'s fallback glyph on an accent-seeded
  floor. **The invariant is that `*Detail.*-id >= 0` means "this banner is in the globals"** —
  `open_*` writes the id *last*, in the same tick as the cover, blur pair, solve and chips, and
  `close-detail` clears it and defers the release to the end of the collapse.
  - The colour set and image slots are gated on **`ui::my_library::the_band_is_up`** (nav is
    still 3 — deliberately *not* the section gate's predicate, which also goes false when Now
    Playing or the miniplayer covers the band). What hands them back is
    **`MyLibrary.hero-collapsed`**, per id, so a tab switch that collapsed the band without
    closing anything keeps its banner, plus the page's own teardown.
  - **The chip row stops one step earlier, and the asymmetry is the point**: a colour held
    across a hand-off is the outgoing hero's *tone*, where a count held across it is the
    outgoing hero's *facts* under the incoming one's title. `hero_chips::clear_if_stale` reads
    a `ChipOwner` every `publish_*` stamps and clears only a row the band has stopped painting;
    the pure decision is `should_clear(recorded, band, still_open)`, testable without a window.
    A predicate taking the *departing tab* cannot answer this — a cross-tab drill fills the
    strip in the tick that moves the tab.
  - `clear_hero_blur` still calls `reset` directly and is correct: it opens with its own
    `section_active()` bail, so it can only run with nav on 2 or 8, where `the_band_is_up`
    answers `false`. The tempting inversion — "an on-screen hero is what the gate is *for*" —
    is what a later edit would act on; the gate asks whether **My Library's band** can still
    reach these globals, not whether anything is painted.
  - **What the hold costs is memory**, bounded on purpose: up to three details'
    `(cover, blur-a, blur-b)` triples resident for the length of a page visit, handed back in
    full by the page teardown and per-detail by any genuine close. The Rust-side caches are
    untouched — `release_section_state` still drops the `detail_artwork` LRU, grid data and row
    models on every tab leave. Pinned by `ui::hero_backdrop::tests` and
    `ui::hero_chips::tests::a_teardown_clears_only_a_row_the_band_has_stopped_painting`.

- **`HeroChips` publishers take their facts as arguments or off their own section handle, and
  read back no Slint property.** Lifting a count off the global the caller just wrote makes
  write order part of the contract. Pinned by
  `ui::hero_chips::tests::no_publisher_reads_its_facts_back_off_a_slint_global`.
  - **Facts a stats row doesn't carry are folded on the worker, never in the closure** —
    `ui::hero_folds`' `fold_tracks`, `dominant_genre`, `year_span` and `fold_most_played` run
    beside the fetch that produced the `Vec` and their `Copy` results ride into
    `upgrade_in_event_loop`. **Most Played sums itself**: its query is
    `is_favorite = TRUE AND play_count > 0`, a strict subset of the Songs tab, so reusing the
    Songs duration overstates it.
  - **Favorites is assembled from three fetches**, so each folds its *own* answer on its *own*
    worker and stores it (`FavoritesUiState::songs_fold`/`::most_played_totals`) before calling
    `hero::republish_chips`. Folding at publish time meant the band came up with two chips at
    cold start, having walked every favourite on the UI thread to get there. **Moving the fold
    onto the worker moves the teardown with it** — `release_section_state` resets both folds
    beside the caches they summarise, a derived value outliving its source being the one thing
    the band can state that is *wrong* rather than merely absent.
  - **A spread of one is stated as nothing at all** (`push_fold` gates both chips on `> 1`),
    and **a set of none is stated as no band at all** — an empty hero publishes zero chips and
    leaves the copy to whatever the body already paints, so no page says "nothing here" twice.
    Each Favorites tab gates on its *unfiltered* count, a filter matching nothing being the
    empty states' business.
  - **A band states facts about the set the page is about, never about the current filter.**
    Forced rather than chosen: an album's chips cannot follow its track filter without lying
    about the album. The counts that *do* track a filter already exist and gate the grids'
    empty states.

- **`HeroChipStrip`** (`components/hero-chip-strip.slint`) is where a hero mounts
  `MetaChipStrip`: it fixes `HeroBackdrop.chip-fill`/`chrome` and forwards `measured` into
  `HeroChips.recompute`, making the wrong mount unspellable rather than merely tested against.
  Now Playing passes its `np-accent-{pill,bright}` pair by hand. The `Theme.*` defaults on
  `MetaChip` are correct nowhere and never taken; they exist so the file imports neither
  `Player` nor `HeroBackdrop`, which is what lets one component sit on a blurred cover *and* a
  blurred banner.
  - **`fade` multiplies both brushes** (default `1.0`, so the five fixed heroes never name it)
    and only `LibraryTabBand` sets it. It has to fade this way rather than with an `opacity`:
    the strip carries `MetaChipStrip`'s `changed watched-w` tracker, so a band wanting a
    fade-out must keep it *mounted* across the collapse, and `Opacity::need_layer` bails only
    at exactly `1.0`.
  - **`arrive-t` is the wrapper's own float**, eased on `dur-med` off `rows.length > 0` — chips
    are the one hero fact that can land *after* the banner, and stepping them in beside a
    settled band reads as a glitch. `dur-med` because that is what the blur crossfade and the
    palette mirrors either side already take. Both brushes carry both factors; multiplying only
    one fades the pill while its label steps. Pinned by
    `ui::library_tab_band_tests::the_chip_row_fades_in_when_it_lands`.
  - **`MetaChipStrip` owns the row layout and both paid-for pitfalls** (the `watched-w` mirror,
    `changed` rejecting a path expression; the 1 ms mount seed, `changed` not firing on a first
    evaluation) and reports through `measured(length)`. **Who chunks, and how far overflow may
    wrap before being dropped, is the host's**, via `ui::chips::chunk_chips_to_rows`'s
    `max_rows` — `None` for the Now Playing column, `Some(HERO_MAX_ROWS)` for a hero band.
  - **`HERO_MAX_ROWS` is 2, and the 2 is measured rather than picked.** Every hero ends its
    meta block with a stretch spacer before the action pill, and a second 26 px row plus its
    4 px gap fits inside that slack on all six, so a narrow window wraps into space that
    already existed. A third row overruns the tile and pushes the pill out of the banner. If a
    wrapped second row ever clips on Album or Playlist — the two carrying a subtitle line — the
    fix is a per-hero max into `ui::hero_chips::write_rows`, **not** a taller band; the band
    clips, so the failure is bounded.
  - **The gap is `pad-xs` between rows and `pad-sm` between chips**, and only the horizontal
    one is mirrored in Rust (`ui::chips::SPACING`): a wrapped row is the same strip continuing
    rather than a second one, so the tighter vertical gap reads correctly *and* buys the fit.
  - **The rows hang off a plain `VerticalLayout` pinned at `min-width: 0px`**, not off the
    root — the `if`-conditional-child pitfall in `slint-pitfalls.md`, which cost this component
    one shipped bug per axis. Without the wrapper the root reports `preferred: 0` and paints
    the second row outside its allotment, so `max_rows` bounded a wrap that never happened;
    with the wrapper and without the `min-width`, the widest published row becomes a floor no
    narrowing can negotiate. The second is the one to keep in mind, fixing the first being what
    introduces it.
  - **`Theme.hero-title-size` is the one number all six titles read**, the same argument
    applied to type: they had drifted to two sizes with nothing recording which was intended,
    and a literal is invisible in review because each view reads correctly alone.
  - `ui::hero_chips::tests` pins the layering and the geometry over **`HERO_VIEWS`** (two
    entries for six banners — each band stands for the pages under it), with **`MOSAIC_HOSTS`**
    and **`BAND_HOSTS`** pinning that a page still *mounts* its band and has grown no title,
    chip strip or artwork size of its own. The detail bodies are the half worth pinning: one
    regrowing a header passes every other check, because the shared band it stopped using is
    still correct.

- **The two mosaic heroes' `last_mosaic_paths` guard means "this mosaic is what's painted", so
  it moves only *past* the check that decides whether anything is** — inside
  `hero::apply_hero_blur` and `hero::clear_hero_blur`, never at the fetch that kicked them.
  Both bail outright when the section went inactive mid-compose, so a guard written beforehand
  records a paint that never happened and every later refresh for the same top-4 early-returns,
  leaving the banner on the accent-seeded floor for the rest of the session — only a section
  leave's `forget_mosaic` clears it. Recording past the check costs a duplicate compose per
  refresh that starts inside the compose window; the losers re-check under the same lock and
  drop out. **The pair is one source, not two**: `src/ui/mosaic_hero.rs`'s
  `impl_mosaic_hero!($Global, $Ui)` generates it into each view's `hero.rs` (a macro rather
  than a generic fn for the `impl_detail_view_helpers` reason — two distinct Slint-generated
  globals with no trait between them). This is the `last_grid_signature` discipline — write
  where the paint happens — applied to the one guard that wasn't following it.

- **Now-Playing accent tiers are derived on the `Player` global, not at the call site.** One
  solved brush (`np-accent-bright`) plus three named translucent tiers off it — `np-accent-pill`
  (.16), `np-accent-disc-hover` (.30), `np-accent-dim` (.60) — following `theme.slint`'s
  `floating-chrome-bg` precedent. Reach for the tier, never a fresh `.with-alpha(…)`.
  `np-accent-pill` is the resting weight for **every** tinted surface in the view, so the top
  bar sits at the same tint as the chips under it. **Only alphas used twice or more earn a
  name**; a genuine one-off stays inline, naming it would move the number without sharing it.

### Lists and playback

- **`play-row` replaces the queue with the view; there is no single-track play path, and no
  Play-All pill either.** Every row activation resolves the view's *displayed* ids and hands
  them to `library::playback::player_play_tracks(ids, start)`. The eight **Play All** pills that
  made that same call pinned to `Some(0)` — exactly what activating the first row does — are
  gone along with `Browse.has-playable-files`; don't reintroduce one.
  - The pill that remains is **Shuffle**, which earns its place by flipping shuffle on, and
    routes through `ui::callbacks::spawn_play_then_shuffle`: play from a **random** slot, then
    `queue_set_shuffle(state, true)`. Random because the shuffle anchors the current track at
    the front, so a head start opens every press on the same song; `queue_set_shuffle` rather
    than a read-then-toggle pair because the pill means "on", and a toggle racing the
    transport's button would turn it off.
  - The start slot comes from `ui::callbacks::play_row_start(&ids, id, idx)`, which trusts the
    index Slint passes when it lines up and otherwise falls back to a lookup by id — Browse's
    disk-only rows sit in the displayed list but not in `current_in_library_ids`, so its two
    index spaces differ. `player_play_tracks` then re-resolves that slot by **track id** against
    what the DB actually returned (`resolve_start_slot`), the fetch dropping ids that no longer
    exist, and re-shuffles anchored to the picked track when `shuffle_enabled`.
  - A new list view wires `play-row` to its own `*_track_ids()` helper. Search is the sole
    exception, reading `Search.tracks` through `model_track_ids`, its visible set being sorted
    and `COMPACT_TRACK_LIMIT`-truncated at render. The two Most Played grid tabs have no row
    index and resolve by id against their own cache, **filter-aware** through
    `row_match::most_played_matches` — the same predicate the model build uses, so the cards
    and the queue they load can't disagree about what's on screen.
  - Appending without wiping is the *context menu's* job (`queue_play_next_many` /
    `queue_add_tracks`).
- **Detail-page TrackList inset.** The band is full-bleed, so a detail *body* is free to inset
  on its root like any grid page — **Album and Genre do; Artist and Playlist can't**: Artist
  Detail's `below-hero` is the region `CompositeScrollbars` measures and the `CompositeScroll`
  hover sentinel covers, and Playlist Detail's empty state and drop banner deliberately fill
  `body`. Overlay scrollbars stay at `parent.width - self.width` either way. Bottom padding is
  never on the root — see the dead-strip entry in `slint-pitfalls.md`.

### Dialogs, pickers and toasts

- **Selectable-picker dialogs share one toolkit** (`components/dialog/selectable-picker.slint`:
  `SelectIndicator` + `SelectableRow` + `SelectAllHeader` + `PickerListCard`). Both the Export
  picker (`kind == "export-playlists"`) and the Add-to-Playlist picker
  (`kind == "add-to-playlist"`, multi-select, footer **Add** commits via
  `Playlists.add-tracks-to-selected`) are thin wiring over it; toolkit components are
  data-agnostic (`cover`/`subtitle`/`selected`/`disabled` `in` props + a `toggled()` callback).
  Both commit through the `Dialog.accepted` dispatcher gated on a `*-selected-count > 0`.
  Add-to-Playlist keeps fully-contained playlists disabled, and `set-all-add-picks` and the
  selection count count only enabled rows. Selection toggles and commit live in `files.rs`
  (commit needs `Rc<NotificationsUi>`); the picker opener stays in `dialog.rs`. The
  blur-avoidance rule for `PickerListCard` is in `slint-pitfalls.md`.
- **Reusable notifications stack** (`components/dialog/notification-stack.slint` +
  `src/ui/shell/notifications.rs`). Reads `Notifications.rows` and mirrors `Dialog`'s
  `kind`-routing — a new action is one `if (kind == "…")` branch plus one `notifications.show(…)`
  call. Cap 5. Per-card props use `data:` not `row:` (Slint reserves `row` as the iter var), and
  translated strings reach Rust via `pure callback`s on `Settings` wrapping `@tr(…)` literals.
- **Backend-thread toasts via `services::toast`.** `NotificationsUi` is `Rc` (UI-thread only),
  so backend failures on tokio workers surface through a neutral
  `OnceLock<UnboundedSender<ToastRequest>>` + `notify(ToastKind, detail)` — a no-op when
  uninstalled, holding no `ui::*` types, which preserves the `tasks`-no-`ui` rule.
  `boot::ui_setup::install_toast_bridge` drains the `mpsc` (**not** a `watch` — errors must not
  coalesce) into `NotificationsUi::show`, resolving the localized **title** by `ToastKind` and
  leaving the dynamic **detail** untranslated. Producers are `execute_actions`' decode failure
  (the vanished-file skip stays silent) and the `spawn_logged_toast!` macro on user-initiated
  scan/import failures. Routine failures (favorite/rating/nav) keep `spawn_logged!` — don't
  toast-spam.

### Filtering

- **Every filter box answers "does this row match" through `src/ui/row_match.rs`, and none of
  them spells `to_lowercase().contains(…)`.** The module owns `search_fields` (the six fields a
  track row is searchable by, ordered like the `tracks_fts` column list it mirrors), the
  `push_folded`/`fold_needle` pair that folds case **and accents**, and the predicates over them
  (`track_matches`, `most_played_matches`, and `Needle`'s `contains`/`equals`/`starts_with` for
  the single-name surfaces — the last two exist for the Search view's Top Result ranking, which
  asks *is this the name* rather than *does it contain the name*). Sixteen surfaces route
  through it. On all three tabbed pages the needle is **one shadow shared across the tabs**, and
  a tab pick clears it on both sides.
- **A needle is folded exactly once, by whoever owns it — and `Needle` is the type that makes
  that unspellable otherwise.** `fold_needle` is the only constructor and the predicates take
  nothing else. Carrying the needle rather than a `&str` is also what lets its *shape* be
  answered once per walk instead of once per row: `ascii` decides the allocation-free byte path
  and `digits` gates the year rule, and neither can change between rows.
- **The fold is looser than the FTS side, and years are looser in the other direction.**
  `is_combining_mark` is `General_Category=Mark`, so it also drops Indic spacing marks and kana
  voicing marks where SQLite's table is Latin-scoped — which only ever *widens* a substring
  filter, and under-folding is the failure that shows. `Needle::matches_year` is a **substring**
  where the FTS side's uniform `*` suffix makes years a prefix search, so `98` narrows a filter
  box to the 1980s and 1998. That is the field falling in with its neighbours rather than with
  the index, and the one place the parity claim doesn't hold. (`library-data.md` owns the FTS
  side.)
- **The two cached lists are the matchers that don't call `track_matches`.** My Library's Songs
  tab holds the whole library and Favorites' holds every favourite, so `RowSearchKey` folds
  `search_fields` once per fetch into a packed `\0`-joined `Box<str>` a keystroke can `contains`
  without allocating (through `Needle::as_str`), keeping `year` beside it as an integer so both
  matchers run the *same* `Needle::matches_year`. It lives in `ui::track_list_cache`, pinned by
  `…::tests::the_packed_key_and_track_matches_agree_field_for_field`. **The entity grids match
  their raw fields, not their `*_lc` sort keys** — folding can't be baked into a lowercased
  string without also changing the sort those keys drive.
- **Nine of the sixteen are fed by a single box, and the hand-off is a Rust dispatch rather than
  a binding.** My Library has one `SearchBar`, in the band, whose meaning follows the mounted
  tab and any open detail; `src/ui/my_library/filter.rs::dispatch` routes a settled keystroke to
  whichever surface is up. It has to be a *write*: an element can't declare a binding on another
  global's property, so `Tracks.filter: MyLibrary.filter` is unspellable, and the only form that
  would work puts `MyLibrary` in five globals' import lists.
  - **The two contracts stay different underneath**: the five grid/list globals fire
    `apply-filter(text)` and Rust ignores the argument, reading `<Global>.filter` back inside a
    memoized rebuild, while the four detail globals fire `filter-changed(text)` and Rust uses
    it. All nine arms therefore `set_filter` before invoking — which also keeps
    `playlist-detail.slint`'s `reorder-enabled` honest, that being the one live *Slint* reader
    of a detail's `filter`.
  - **Opening or closing a detail changes what the box means with nobody typing**:
    `MyLibrary.detail-scope-changed()` routes to `filter::sync_box`, taking the newly-mounted
    surface's own filter. That is a *reseat*, deliberately not the tab pick's clear-both-sides —
    clearing on the way out would drop the user's grid filter on every back.
  - **A tab move fires it too**, and that mirror is the one easiest to leave out: `dispatch`
    clears only the *entering* tab's needle, so the two arrivals that aren't picks (a cross-tab
    drill, a Mouse-4/5 walk, both through `persist-tab-idx`) land on a tab still filtered under
    a box that says nothing about it. Hence **five** `changed` mirrors in the sheet, not four.
  - **The mirrors can't cover a re-open**, which is the third caller and the one with no edge to
    fire on: `open_*` clears its detail's own filter on every fresh open, and a section re-enter
    re-runs it writing the *same* id back. Each of the four therefore invokes
    `detail-scope-changed()` itself, as the **last** statement of its `upgrade_in_event_loop`
    closure — `sync_box` picks the surface off the live id and tab, so anything earlier answers
    for the grid the detail is still sitting over. Pinned by
    `my_library_tests::{a_drill_a_back_or_a_tab_move_reseats_the_shared_box,
    a_fresh_open_reseats_the_shared_box_after_it_writes_the_id}`.

## Covers

- **No row struct carries a decoded cover; every one of them asks for it.** `TrackListRow` has
  no `image` field, and `TrackListRowItem` resolves its thumbnail per *instantiated* row through
  `RowCovers.request(artwork_path)`, wired once in `boot/ui_setup.rs` to the shared row-tier
  `CoverThumbs`. New TrackList consumers need zero cover plumbing. `CoverThumbs::prewarm`
  dedupes input and caps work at LRU capacity — pass paths in **display order** so the kept
  prefix paints first.
- **`QueueRow` follows the same contract through two globals rather than `RowCovers`**, each
  wanting a different tier: `Queue.request-cover` reaches the queue sheet's *private*
  `CoverThumbs` (so closing the sheet drops every buffer without yanking covers the track lists
  still need), `NowPlaying.request-cover` the shared row tier. That is what makes a queue the
  size of the library affordable.
- **`covers-generation` is the gate for a surface that fills without a fetch to hide behind.** A
  `pure` callback's result is cached until a dependency is dirtied, and every ordinary lazy-cover
  surface prewarms and **awaits before** it sets rows, so its first evaluation is already a cache
  hit. Three surfaces can't, for three distinct reasons — the queue sheet's **synchronous open**
  (rows must land inside `on_open_changed` for the slide-up to have text on frame one),
  `EntityCardGrid`'s **tab pick** (`TabBar` writes `selected-index` before it emits `selected`,
  so the entering `if` is already true when Rust hears about it), and `BrowseCardGrid`'s **mode
  toggle** (the pill re-presents a cached listing with no fetch in the path).
  - The argument does two jobs: reading it makes the binding depend on the counter, and its
    *value* is the "is this tier warm" flag. **At 0 the Rust side answers cache-only**
    (`get_cached_opt`), so rows mounted on that first frame paint placeholders instead of each
    dragging a decode onto the UI thread; the surface then warms off-thread and bumps, which
    re-runs the bindings *and* switches later rows to `get_or_load_opt`. Teardown rewinds to 0
    beside the tier clear, so 0 keeps meaning "cold" rather than "first open of the session".
    Wire the decoding lookup unconditionally and the counter is dead weight.
  - **The bump is gated on the prewarm's own verdict *and* a re-check of the active surface on
    the UI thread**, because they fail separately: a pick made while the decodes ran has already
    rewound the counter and owns a different tier, while a section leave landing mid-decode makes
    the prewarm hand its buffers straight back. **A prewarm that may release what it warmed owes
    its caller that `bool`**, and the caller owes it the check.
  - **Announce on the warm, not on the write** (`ui::tab_bar::should_announce_warm`, fed a
    `warmed_tab` that is `Some` only on `Ok(true)` — a `JoinError` is the same "we don't know").
    Whether the tier is warm and whether the rows moved are independent facts; a re-enter
    reliably lands on the signature skip, the mount-time `columns-changed` having written final
    rows while the prewarm was still decoding.
  - Browse rebuilds its model **without hopping the event loop** —
    `slint::invoke_from_event_loop` posts even when called from the UI thread, and a redraw
    winning that race paints an empty grid. Its released tier also obliges a `mark_dirty()` on
    the section leave and the same wire-time seed, having no enter-time fetch of its own.
  - **The Rust half of the lookup is `ui::grid_prewarm::grid_cover(thumbs, path, generation)`** —
    the tier and counter differ per page, the branch doesn't. Reach for it at a fourth surface:
    a copy that grew a decoding `else` arm reads correctly and quietly retires the mechanism.
    The hero's `CoverMosaic` keeps the one-argument form on purpose, its tier being warmed by a
    fetch.
- **Grid-cover cache cap via `ui::grid_prewarm::cover_cap_for_window(app, fallback)`.** One band
  for every grid — they all draw the same card at the same size, so there is nothing per-entity
  to tune. It derives a `[32, 96]` cap from the current monitor's *logical* resolution against a
  ~260×320 px card footprint, with one partial row of scroll headroom. Each cache is constructed
  with its own `DEFAULT_GRID_COVER_CAP` and resized from `install_views` once the winit window is
  live; the fallback is passed in so a module keeps ownership of its own default and a monitor
  reporting `None` lands there. The pure half is `cover_cap(w, h, fallback)`.
- **Cover-prewarm path dedup via `ui::grid_prewarm::unique_artwork_paths(paths, cap)`** — first-
  seen-ordered, deduped, non-empty `Vec<PathBuf>`. **Every prewarm site in the tree goes through
  it**, the per-entity wrapper owning only the projection. Don't lean on `prewarm`'s own internal
  dedup as a reason to skip it: the two sites that did each grew a divergent copy, one passing
  `PathBuf::from("")` straight to the decoder. **`cap` bounds kept *paths*, not input items**, so
  a grid passes `GRID_PREWARM_AHEAD` rather than `.take(N)`-ing its entity iterator and a
  full-list prewarm passes `cover_thumbs.capacity()`. Capping the input is what stops a detail
  over a 20 000-track genre allocating a `PathBuf` per unique cover to keep 512 — and
  `prewarm`'s own `.take(cap)` sits **after** its already-cached filter, so over a partially warm
  tier an uncapped call decodes a full capacity of new paths from anywhere in the list, evicting
  the warm visible prefix to do it.
- **Albums sub-section borrows `AlbumsUi.grid_covers`.** Artist Detail's Albums strip routes
  `request-album-cover` to `albums_ui.grid_cover(path)`, and the Artists wiring releases on
  **both** Artists section-leave and `on_close_detail`.

## Releasing what the UI pins

- **Detail-close releases global Image properties.** `release_detail_hero_images!` resets `cover`
  + `blur-img-a/b` to `Image::default()`, clears `has-blur`, and re-solves the two shared globals
  (`hero_backdrop::reset`, `hero_chips::clear`), alongside `clear_detail` +
  `release_detail_artwork`. Without it, `SharedPixelBuffer` Arcs pin (~650 KiB CPU + ~1.5 MiB GPU
  on Mesa). It runs on each view's **section leave**, but since the My Library fold that fires on
  a tab switch too — so each half is gated, per the hero-teardown rules above.
- **A close doesn't run it, and that is the contract the morph forced.** Every fact the band
  paints is a ternary over the detail id at `my-library-view.slint`, so clearing on the frame the
  id does leaves the band spending its whole 400 ms collapse painting a fallback glyph over a
  reset gradient. The fix is two parts, neither working alone:
  - The sheet **latches which arm the banner paints** (`hero-album`/`-artist`/`-genre`/
    `-playlist`, seeded by their bindings and written only while some detail is open — so a
    cross-tab drill still moves them and a close does not). The *body* router and the pill rows
    read the live `*-open`. **All four are written together, by one `latch-hero()` the four
    `changed *-open` handlers share, and that is not tidiness**: the hero facts are ternaries
    tested in declaration order, so a handler moving only its own arm leaves a stale arm
    outranking whatever opened after it for the rest of the page mount. Pinned by
    `my_library_tests::the_hero_reads_a_latched_arm_where_the_body_reads_the_live_one`, which
    counts the writes inside `latch-hero` as well as checking each handler routes through it.
  - The teardown moves out of the four close handlers onto **`MyLibrary.hero-collapsed`**, fired
    by a `dur-spatial` `Timer` in the band — the `Dialog.closed()` shape. The timer is armed and
    cancelled by the one edge that drives the morph, so a re-drill inside the window can't land
    the previous hero's teardown on the new one. `release_collapsed_hero` asks **all three**
    image-bearing globals through the narrower **`release_hero_slots!`** — all three because the
    band can't say which closed and doesn't need to — but hands each back only **on its own id**.
    The backstop is the page's own teardown rather than the per-tab leave: a nav away mid-morph
    kills the timer with the sheet.
- **The idle half of the band needs the same latch with the guard the other way up**, and that is
  `count-line`. Bound live it re-reads the arriving tab the frame `tab-idx` moves, so the
  departing sentence vanishes rather than fading and the section-enter's `fetch_grid` pops the
  new count onto it mid-fade. `live-count` holds the ternary, `count-line` is what the band is
  handed, and `latch-count()` is the unguarded writer the two `changed` handlers **and `init`**
  share. What differs from `latch-hero` is the guard: **the hero half holds across a *close*
  (`if (detail-open)`), the idle half across an *open* (`if (!detail-open)`)**. Pinned by
  `my_library_tests::the_count_line_holds_the_sentence_it_is_collapsing_out_of`; the mutation is
  dropping the `init` call, which leaves every other assertion green.
- **Dialog-close releases global Image properties + scalar state — via exactly one handler.**
  `Dialog.closed()` fires once close-anim `t` returns to ~0, and there is **one** `on_closed`
  registration in the tree (`playlists/callbacks/dialog.rs`). It does both halves:
  `Dialog.invoke_closed_teardown()` (the Slint side — `kind`/`target-id`/`input-text*`/`mosaic-*`/
  `pending-track-ids`/the two picker row models/`title`/`message`/`cancel-label`/`destructive`;
  restore `confirm-label` to `"OK"`, don't clear to `""`), then `current_artwork` **and**
  `TagEditor.cover` reset to `Image::default()` — the two `image`-typed global properties, which
  have no Slint default literal — plus `heap_trim::trim`. **The teardown is a Slint
  `public function`, not a callback body, and that is load-bearing**: a callback has a single
  handler slot, so a default `closed => { … }` body is installed at `InnerDialog::new()` and then
  silently replaced by the Rust `on_closed`. `CompositeScroll.reset()` takes this shape for the
  same reason. Do **not** clear in `accepted`/`cancelled` (unmounts the body mid-fade), and do
  **not** re-register `closed` from inside the handler (`Callback::call` `take()`s the handler and
  asserts). A new dialog kind pinning an image extends the single `on_closed`.

## Popups, native dialogs, input

- **PopupWindow auto-dismiss on OS focus loss.** `components/focus-loss-watcher.slint`'s
  `FocusLossWatcher` is a one-shot component firing `close()` when the local `Theme.window-focused`
  mirror transitions false. Mounted inside `if popup-is-open` so only the open popup has a live
  watcher; singletons gate on `PopupHighlight.id == "<discriminator>"`, and the per-row context
  menu on `PopupHighlight.row-ctx-id == row-data.id` (set on right-click, cleared in
  `winit_filter.rs`'s Release path). Slint 1.16 has no `closed` callback, but `pop.close()` is a
  safe no-op when hidden. **`changed` must watch the local mirror** — Slint rejects path
  expressions on globals.
- **Native dialogs (rfd) — always through `ui::file_dialog::parented(&weak, title)`.** UI thread
  via `slint::spawn_local(Compat::new(…))`; the helper owns the `weak.upgrade()` +
  `.set_parent(&ui.window().window_handle())` half and the caller chains its own
  `add_filter`/`set_file_name`/`pick_*`/`save_file`. Requires
  `slint = { features = ["raw-window-handle-06"] }`. **Never build the dialog inline**, and the
  reason is that the bug is invisible here: without the parent it z-orders behind Melodia on
  Windows and macOS, while Linux's XDG portal parents OS-side regardless. Held by
  `ui::file_dialog::tests::every_native_dialog_is_built_by_the_shared_helper`, which **walks
  `src/` rather than naming the five call sites** and carries a caller floor so a caller that
  stops opening a dialog trips it too.
- **Keyboard shortcuts** — the root `FocusScope` is `ShortcutScope` (`layout/shortcut-scope.slint`),
  which owns every binding plus the volume-commit debounce and takes the whole main layout as its
  children. It grabs focus on `init` and re-grabs via `shortcut-scope.grab-focus()` on every
  content-view switch: Slint doesn't hand focus back to an ancestor scope when the focused item is
  destroyed, so a view unmounting with focus inside it leaves shortcuts dead until the next click.
  - **Ten `changed` mirrors regrab it.** Nine are one per branch selector of an `if` chain that
    mounts a view — `Nav.selected-index`, `MyLibrary.tab-idx`, `Nav.now-playing-open`, the four
    `*Detail.*-id`s, `mini-switch.render-active`, and `Dialog.kind` for the overlay's own chain.
    Same reason `CompositeScroll.reset()` needs its five, so a new always-mounted mirror that
    unmounts a *composite* view owes both (this set is the wider of the two). The tenth,
    `Queue.open`, is not about destruction: the sheet is permanent and mounts nothing, but
    *nothing else moves focus onto it*, so without the grab a filter `SearchBar` keeps the
    keyboard behind the sheet's backdrop and eats the `a`.
  - Gates non-Esc on `!Dialog.open`. Typed keys in TextInputs never reach root. Bindings: Space
    play/pause; ←/→ seek ∓5 s (Shift ∓30 s, Ctrl prev/next); ↑/↓ vol ±5 (Ctrl ±1); 0–9 seek
    0–90 %; M mute; L favorite; N/P next/prev; S shuffle; R repeat; Q queue sheet (all three
    queue bindings mini-gated — the sheet doesn't exist in miniplayer mode, and arming
    `Queue.open` with no mounted mirror to fire `open-changed` remounts it rows-less); F Now
    Playing; F11 maximize; Esc Dialog cancel → queue sheet (clear selection, then close) → NP
    close; Ctrl+A queue select-all; Ctrl+B sidebar; Ctrl+, settings; Ctrl+N new playlist; OS
    media keys via souvlaki.
  - **`ShortcutScope` is the only `FocusScope` in the tree, and the queue sheet is why that's
    worth stating.** A `FocusScope` takes focus on **mouse press**, never on mount, so the
    sheet's own nested scope left Esc and Ctrl+A dead on a sheet opened with `Q`. Both sit at the
    root now, gated on `queue-sheet-up` (`Queue.open && !MiniPlayer.active`, the flag alone
    surviving the miniplayer swap), which also fixes the priority — Esc reaches the *dialog*
    first. A second scope would reintroduce all three problems; put new key handling here.
- **Animated view transitions** — main-content branches mount via `ViewTransition`: enter-only
  fade + 32 px axis slide over `Theme.dur-spatial`. No exit anim, Slint `if` destroying the
  outgoing branch instantly. Direction on `Nav.pending-enter-from: NavEnterFrom`
  (`{ below, right, left }`) — sidebar `below`, drill-in `right`, back `left` — written
  **synchronously on the UI thread just before** flipping the `if`, routed via
  `src/ui/nav_transition.rs`. `ViewTransition` flips `shown` via a single-shot 1 ms Timer, and the
  panel's `clip: true` masks overshoot.
  - **An unwritten edge is not a default — it is whatever the last navigation left in the
    global**, and since `mark_drill_back` fires on every detail close, the value sitting there
    between navigations is reliably `left`. So *every* Slint-side mount writes its own, `below`
    by construction, the two non-lateral directions being Rust's.
    `ui::nav_transition_tests::every_slint_side_mount_writes_its_own_enter_edge` **walks** the
    tree for both write shapes, the `file_dialog` reason. The miniplayer's mark is the one that
    isn't about a closer: the swap destroys the whole full UI, so the content branch remounts on
    the way *back*.
  - **Two inputs turn parts of it off, and they answer different questions**: `enabled: false` is
    "does this view animate at all" (the page's own enter already owns the entrance),
    `slide: false` is "is anything *else* already translating it" — fade, don't move. Both
    default on, so every mount that owns its motion says nothing.
  - **A page with sub-views nests a second one, and it has to be disarmed at mount.** Favorites
    and Recently Played wrap each tab body in its own `ViewTransition` so a tab pick slides
    sideways, but the page's own enter is still playing on the frame its first tab body mounts and
    a horizontal slide composed with the sidebar's fade-up reads as a diagonal. Hence `enabled`:
    off, its `settled` guard is true from the first evaluation. The host arms it in the tab bar's
    `selected` handler, the same place it sets the direction and for the same ordering reason.
    Starting `false` is what makes the page re-disarm for free, being destroyed and rebuilt on
    every entry; arming from a mount `Timer` would race `ViewTransition`'s own. The **direction**
    comes off `bar.previous-index`, so the host keeps no tab state and needs no mount seed.
  - **My Library nests the same thing and takes no direction at all, because it is the one page
    with a *third* animation — its own band's height.** The band is the non-stretching sibling
    above the body router, so a morph moves `body.y` by the whole distance between the two floors
    on every frame. All nine branches carry the same three lines — `enter-from: NavEnterFrom.below`,
    `enabled: root.body-anim-armed`, `slide: !band.morphing` — and none reads
    `Nav.pending-enter-from`.
    - **`slide: !band.morphing` is not a nicety, and same-axis is not enough.** The band publishes
      `morphing` as `hero-t != (detail-open ? 1.0 : 0.0)` — deliberately not `hero-t > 0`, which
      is `false` on a drill-in's first frame, the one frame the answer has to be `true`.
      Comparing against the *target* also makes it independent of where `changed detail-open`
      falls relative to the repeater, and it doesn't drift: `set_animated_value`'s `Done` arm
      returns `to_value` verbatim, so the equality really does clear. Left sliding, the body's own
      32 px is made of a curve that disagrees with the morph's, so the sum runs the wrong way
      before it turns. Fading through a container transform is M3's own answer for content inside
      one.
    - **`body-anim-armed` seeds off `band.tab-anim-armed`** and is written `true` from
      `changed detail-open` **and** from the `watched-tab-idx` mirror the shared filter box
      already needs. The seed alone answers "a tab has been picked in this mount"; the two
      arrivals that aren't picks each need one of the handlers, a cross-tab drill writing the new
      id and calling `go_to_tab` in *one* tick so `detail-open` never transitions. Neither handler
      can fire on the page's own entry, `changed` not running on a first evaluation — which is the
      point, since what the gate holds off is that entrance.
    - Pinned by `my_library_tests::{every_body_branch_enters_on_the_bands_own_axis,
      the_fade_only_mode_suppresses_both_offsets}` and
      `library_tab_band_tests::the_band_publishes_whether_its_height_is_still_moving`. The first
      **walks** the branches rather than listing them; the second pins both axes, gating one being
      the half-fix that still goes diagonal. **Favorites and Recently Played keep their left/right
      tab slide and want none of this** — `MosaicTabHero` is a fixed height.

## Settings and nav wiring

- **Section-visibility hooks go through `SectionActiveGate`**
  (`components/section-active-gate.slint`), mounted in `app-window.slint` in nav-index order —
  **once per section, plus one more per tab for a section built out of tabs**. It owns the
  predicate (the section's index is selected *and* neither the full-screen Now Playing view nor
  the miniplayer is covering it) and fires `active-changed(bool)` into that section's Rust hook,
  which releases the section's cover caches on the way out and re-warms or un-dirties them on the
  way back in. Non-visual, `FilterThrottle`-style, so it costs no layout. **It has to be mounted
  on the always-alive `AppWindow` root, not inside the view**: the leaving transition is the edge
  that matters and a view cannot observe its own, Slint destroying it with no unmount callback. A
  new section adds one mount, not a tenth copy of the predicate; pass
  `mini-active: mini-switch.active` (the switch's live derivation, which leads the `MiniPlayer`
  global by a beat).
  - **A tab switch is a section switch, and the gate is where that becomes true rather than each
    view.** `tab-index`/`current-tab` are an optional sub-predicate, both defaulting `-1` so the
    predicate short-circuits and the four tabless mounts are untouched. My Library mounts five at
    `index: 3`, so a tab leave fires the departing view's existing `section-active-changed(false)`
    and **every lifecycle hook works unchanged**: cover release, model clear, `mark_dirty()` →
    re-fetch. Two things follow — the page needs **no `covers-generation` machinery**, the
    entering tab's own fetch prewarming its first screenful before it writes rows, and background
    `library_changed` bumps stop rebuilding four hidden grids. A `0` default on either new
    property silently deactivates all nine sections, which
    `ui::tab_bar::tests::the_section_gate_ignores_its_tab_predicate_when_a_section_has_none` pins.
  - **What the five genuinely cannot answer is the page's own leave** — only the *mounted* tab's
    fires — and **the answer is not a sixth mount**, which is the obvious shape and compiles. A
    gate fires on transitions of *its own* predicate, that predicate is already false while Now
    Playing covers the band, and `sidebar.slint` clears `now-playing-open` and writes the new
    index in **one** handler, so the leave that matters most goes false → false and delivers
    nothing. `MyLibrary.page-active-changed` therefore rides `app-window.slint`'s
    `changed watched-nav-idx`, beside `CompositeScroll.reset()` and the focus regrab. Watching the
    index also makes "covered" and "left" different events for free.
  - **A mirror fires on every nav change where a gate fired only on its own predicate, so the hook
    owes a latch** — the half a later edit is likeliest to read as redundant. A `changed` handler
    cannot ask which index it moved *from*, so an unlatched teardown runs on Search → Browse and
    hands back what `seed_detail_from_settings` wrote at boot *because* the page was hidden. The
    latch is a `Cell<bool>` on the closure seeded from `the_band_is_up`, sound for the `seed_tab`
    reason below. It has to live in Rust rather than as a `prev-nav-idx` mirror:
    `Nav.selected-index` is declared `: 3` and the persisted index lands after `AppWindow::new()`,
    so a Slint seed is either a constant firing one spurious teardown per boot or a binding that
    re-evaluates to the *new* index inside the handler. Pinned by
    `ui::hero_backdrop::tests::the_page_leave_is_gated_on_the_nav_index_rather_than_on_the_gate`,
    whose load-bearing assertions are the one refusing a `page-active-changed(active)` anywhere in
    the sheet and the one asking the handler to still latch.
  - **The gate fires on transitions only, so each section's synchronous `section_active` shadow
    has to be seeded correctly on its own — a boot-ordering constraint, not a `wire_*` detail.**
    Every `wire_*` seeds by reading `Nav.selected-index`, so `boot::ui_setup::install_views` writes
    the **persisted nav index before `wire_all`**, and `ui::my_library::seed_tab` beside it for the
    same reason one step down: the five My Library seeds read `tab_is_mounted`, so a tab seeded
    after `wire_all` leaves all five answering for the global's declared `0` and costs a
    full-library Tracks query per launch. Pinned by
    `boot::ui_setup::tests::the_persisted_my_library_tab_is_seeded_before_any_view_is_wired`.
  - **What decides whether a wrongly-seeded gate ever corrects itself is the tracker's baseline,
    not a dropped callback.** `ChangeTracker::init` evaluates inside `AppWindow::new()` and adopts
    the result **silently** — it assigns straight into the stored value and never calls the notify
    half, and `init_delayed` appears nowhere in the generated tree. So the boot reading becomes the
    baseline every later evaluation is compared against, and a gate whose baseline already equals
    the value it settles on has no edge left to deliver. With `mini-switch.active` true on that
    pass (window width 0 until the geometry restore) every gate baselines `false`, and the
    asymmetry is the opposite of the intuitive one: **the restored section self-corrects** once the
    window is sized, while a section seeded `true` against a `false` baseline **stays wrongly
    active all session** — which is not a cosmetic stale tier, `install_library_changed_refresher`
    then taking its ungated arm and re-fetching the whole library per song during plain listening.
- **Nav state persistence keyed by view-id.** All in `views.json` (`ViewStateData`):
  `last_nav_index` mirrors `Nav.selected-index`; `last_detail_ids: HashMap<String, i64>` holds the
  open detail per tab keyed by `view_id`. Setter `library::settings::set_last_detail_id` writes via
  `mutate_view_state`. Sidebar nav does **not** reset detail ids; only the back button does. Adding
  a detail = a `view_id::*` const + open/close `set_last_detail_id` + a seed fn.
  - **A persisted view flag is an int/string/map, not a bool.** `ViewStateData` sat at clippy's
    `struct_excessive_bools` cap, and each tab index sidesteps it by being an index rather than a
    booleans-per-section set (`i32`, clamped on read by `ui::tab_bar::clamp_tab` against the page's
    Slint-declared `tab-count`). **`browse_view_mode` is where the rule bites hardest**, being the
    one genuinely binary flag on the struct: as an `i32` clamped against `Browse.view-mode-count`
    it costs nothing today and buys a third presentation for free, where a `bool` would put the
    struct back on the cap *and* make the widening a migration. Dropping a field is safe on shipped
    installs — serde ignores unknown keys.
- **Audio installers seed from `ui::settings_bind::read_or_default(state, "…")`.**
  `install_{equalizer,replaygain,visualizer}` each opened with the same `match read_settings(…)`,
  spelling their defaults out again in the error arm — a second copy of the `Default` impl and a
  second place for it to drift. Its sibling `toggle_binding` owns the apply-then-persist shape.
  **Not every installer wants it**: `playback_settings`/`file_watching`/`updater_settings` use
  `if let Ok(s)` deliberately, leaving the **Slint-declared** defaults in place on a read failure
  rather than re-deriving them in Rust.

### `TabBar`

**`components/tab-bar.slint`, mounted once — by `components/hero/tab-search-header.slint`'s
`TabSearchHeader`**, the row carrying the bar and the filter box together. **Reach for the *row*,
not the bar**: a page wanting tabs beside a filter wants all five of the fixes in it. Data-agnostic
— parallel `labels`/`icons` arrays, `avail-width`, `selected-index`, `selected(int)`, the same
shape as `ChipGroup`/`Dropdown`/`ColorDotGrid`. It takes the row's width as `avail-w` (the host
owning its own mirror) plus an optional `lead-w` + `@children` leading slot, and publishes
`row-floor`, `row-h`, `search-w`, `tab-enter-from`/`tab-anim-armed` and the six `tip-*` anchors.

- **`tip-x` and `tip-y` both carry the bar's own `absolute-position` offset inside the row**, and
  that is the half a host cannot supply, knowing only its own `header ↔ root` delta. Dropped from
  `tip-x` the pill lands a whole `lead-w` left of the tab it names — on the one mount that has a
  leading slot, so four of the six places you would look agree with the bug. Pinned by
  `ui::tab_search_header_tests`, which also holds the three hosts to mounting the row.
- **It publishes `previous-index` because a cell writes `selected-index` before it emits
  `selected`** — it has to, the `<=>` on that property being what carries the pick out. So a
  host's handler runs with `selected-index` already reading the tab just picked, and the outgoing
  one is recoverable nowhere else. The same ordering is why the Rust side needs the non-hopping
  `apply_*_now` twins. A host wanting the *old* index reaches for `previous-index`, never a
  mirror seeded at mount.
- **Material 3 tabs: a sliding underline, no fill on the selected tab** — accent label, accent
  icon at `filled: true`, and a 3 px accent pill in a gap in the 1 px divider. Hover keeps a state
  layer at `radius-md` rather than M3's square full-bleed rectangle, matching the settings rows.
  The cell fills its own bounds and the **row** gives the clearance, one `padding-bottom: pad-sm`
  reserving the divider/pill band, so a rounded fill floats above the line instead of resting on
  it. That is the deliberate split from the ChipGroups *inside* settings rows, which do fill: the
  bar navigates, the chips configure, and one page showing both needs them to read as different
  things.
- **Every brush it paints with is a defaulted `in property`** — `label-color`, `active-color`,
  `hover-fill`, `divider-color` — and that is what lets it sit on a hero. Settings takes the
  defaults; the two hero bands hand over `HeroBackdrop` tiers for **all four**. **Neither band
  hands a tier over raw — both go through four locally eased `dur-med` mirrors**, because the bar
  itself may not ease a brush and the solve lands late either way. `MosaicTabHero`'s **own title
  then has to take `hero-label` as well**, the title painting that same tier, so easing only the
  bar leaves the two halves of one text tier arriving `dur-med` apart. `LibraryTabBand` needs no
  equivalent, its title living inside `if hero-shown`.
  - **`active-color` is the one worth spelling out**, being a single input for the selected label,
    its FILL=1 icon *and* the underline (M3 gives all three one colour). `Theme.accent` carries
    **no contrast floor** against the pinned band — Latte's mauve lands near 1.7:1, under even the
    3:1 non-text bar — where `HeroBackdrop.chrome` is solved to clear 3:1 whatever the cover. The
    trade before "fixing" it back: `chrome` is the 3:1 tier and `on-backdrop` the 4.5:1 one, so the
    selected label is a tier below the idle ones, and the honest repair is a second input
    separating label from indicator, not a token swap at the call site. Pinned by
    `ui::tab_bar::tests::every_painted_brush_is_an_input` and
    `ui::mosaic_tab_hero_tests::the_hero_tab_bar_takes_every_brush_from_the_backdrop`.
- **The cells are equal width, sized to the widest tab, and that is what makes the underline
  arithmetic** (`ind-x: tab-w * anim-index`). A `for` loop exposes no per-tab element to read a
  position off, so a content-sized row would have to snapshot each tab's geometry on selection and
  go stale on every resize. It costs a little width and buys a sliding indicator with no
  bookkeeping — and it is why **the selected label can't be heavier**: the cell was measured
  against one weight.
- **What eases is two sources; every geometry derives from them unanimated.**
  `anim-index: root.selected-index` (`dur-med`, `ease-in-out`) owns *a tab was picked*;
  `compact-t: root.compact ? 1.0 : 0.0` (350 ms, M3 emphasized-decelerate) owns *the bar changed
  shape*. That duration is inlined rather than taking `dur-spatial`: the flip fires under the
  user's hand on a resize drag, where a 400 ms tail reads as the bar lagging the window.
  **They have to be two** — a single `animate` on `ind-x` or the pill's `width` conflates the two
  events, because both read `tab-w`, and the compact flip then reads as the pill flying in from
  one side on the way down and the other on the way back up. The divider is two segments cut
  either side of the pill, reading `ind-x` directly so neither lags it by a frame.
- **Floats are all a cell may ease.** Its `icon-color` used to and couldn't: a host crossing its
  palette re-dirties that binding every frame, so the glyph sat still and then caught up in one
  rush. The hover fill failed the same way on the arm live when you hover a tab and click it. Both
  are floats now (`hover-t`, `sel-t`) with the brushes tracking their sources unanimated; full
  argument in `slint-pitfalls.md`.
- **`compact-t` is seeded by its binding and owned by `changed compact`, which writes it.**
  Load-bearing at both ends: writing it swaps the animated binding for an animation of its own
  (`set_animated_value` → `set_binding`), which stops a resize drag restarting the curve on every
  event, and keeping the binding as the seed leaves mount alone. A constant seed plus a mount
  `Timer` would morph on every entry into Settings at a compact width. **The ease is gated on an
  `eased` flag a 100 ms mount `Timer` arms**, the *first* resolution not being a change but the
  bar learning how much room it has; 100 ms rather than the tree's usual 1 ms because this has to
  *outlast* the host's first `changed width`, not race it.
- **`compact` is measured, not thresholded.** A hidden `measure := VerticalLayout { visible: false; … }`
  mounts one **real** `SettingsTab` per label — the same component the bar draws, so measured and
  drawn shapes cannot drift — and `measure.preferred-width` is the widest expanded cell. The labels
  are translated, so the width that matters is the running locale's, which retires the
  hand-derived `page-w < 780px` literal and its "re-check when adding a locale" comment.
  - **`compact` stays an instant bool on purpose** — it is the *decision*, `compact-t` is that
    decision rendered over time. That split alone does **not** make easing the flip safe: Slint
    restarts an animated *binding* on dependency **dirtiness**, never comparing values, so it is
    irrelevant that the bool only ever sees a toggle. The gate is that `compact-t` is **written**.
  - **The decision had to move into the bar**: a component may read `preferred-width` off its own
    descendant but not off a child component's internals, so the host can only hand over
    `avail-width` and read the verdict back. It hands over a **mirrored** width, and it is the
    **content panel** less its own padding, not the window.
  - `visible: false` is the right hiding mechanism and worth knowing why: it lowers to an injected
    `Clip` parent with `clip: !visible`, and that pass runs **after** `default_geometry`, so the
    injected element gets no geometry bindings and sits at 0×0. Nothing paints, the hidden
    `TouchArea`s are unreachable, and the layout info the ruler exists for is still computed — the
    "`visible: false` doesn't remove from layout" pitfall read the useful way round. The generated
    layout info never reads the assigned width, which keeps
    `measure.width ← bar.width ← measure.preferred-width` from being a cycle.
- **The threshold is latched, not bare.** A slow resize drag delivers a stream of one-pixel widths
  and a hand jitters over the last of them, so a bare comparison flips on every one that straddles
  the line — invisible instant, hunting once eased. `compact` compares against
  `expand-w + (latched ? hysteresis : 0px)` with a 24 px band, and `changed compact => { latched = compact; }`
  is the only state, the threshold itself re-derived from `expand-w` every evaluation so a locale
  switch still moves it. It converges in one step both ways and needs no seed `Timer`.
- **The bar sizes itself with `min-width`/`preferred-width`/`max-width`, never a bound `width` — an
  eased width on a component root moves the whole window's minimum.** Slint reports a root's bound
  `width` as *both* `min` and `max`, so binding it puts the bar's layout floor on the animated
  `tab-w`; that floor climbs the header row into the page and out to the window, so dragging the
  edge inward chases a wall that is still moving. Worse, the bar was then routinely the constraint
  stopping the window reaching the very width its own threshold was waiting for. `min` is the
  constant `labels.length * compact-tab-w`, `preferred` tracks the animated cells, `max` keeps the
  upper half. **Any component root whose `width` becomes animated owes this same split** — check
  `layout_info`'s Horizontal arm in the generated `app-window.rs` if unsure; `min` reading back the
  root's `width` property is the tell. **It also owes a clip**, the split buying the window's
  freedom by letting the element be drawn narrower than it asked for: on the shrink leg `compact`
  flips instantly while `tab-w` takes 350 ms to follow, and the bound-width cells are
  incompressible. Rectangular is the point — no border, no radius, so it lowers to a scissor
  rather than an offscreen layer.
- **The label collapses onto its icon rather than unmounting.** `SettingsTab` takes `compact-t` as
  a float and derives `open: 1.0 - compact-t`: padding and spacing scale with `open`, so an
  `alignment: center` layout closes to just the icon with no positioning arithmetic. The label
  lives in an always-mounted slot whose `width: label-w * open` — an `if !compact:` is what made
  the flip a pop. Three details are load-bearing: the slot's `clip` is **rectangular and
  borderless** (a rounded one renders the text to an offscreen layer and upscales it on HiDPI),
  the label's fade is `label-color.with-alpha(…)` rather than `opacity` on the element (a layer,
  times five cells), and the alpha is a *bias* off the same source (`clamp(1 - compact-t * 2, 0, 1)`),
  gone by the time the slot is half closed so the clip never shows a half-eaten word. A second
  `animate` on the alpha would phase-lag it. `label-w` reads `lbl.preferred-width` straight off the
  Text the cell draws, which never reads back the width it is handed and so is a derivation rather
  than a cycle. The easing curve keeps both y control points inside `[0, 1]` so no derived width
  goes negative; a back-out curve would need a `clamp`.
- **Sharing the header row means the two widths are budgeted, not independent.** `avail-width` is
  `content-w` less the two 12 px gaps less whatever the input is taking, so the tabs give up their
  labels earlier than they did centred on their own row. `SearchBar`'s slot never stretches past
  `input-width` (`preferred` and `max` are the same number, so focus-scaling happens inside the
  slot), which makes it the page's job to size it. **The filter yields before the tabs do** — tabs
  are navigation, search only refines what a tab already shows. Both ends of that clamp read a
  published floor (the tabs' `compact-w`, the input's `min-w`) rather than a restated literal,
  which is what keeps each from drifting and makes the row's minimum a sum the page can check.
- **The header row is five things**, each a paid-for fix: a `page-w` mirror written from
  `changed width`, a 1 ms mount `Timer` re-running it, a seed at the row's own floor, the
  `search-w`/`avail-width` budget, and a top-layer tooltip frame declared after the scroll chrome.
  Which is why a *third* hand-written copy was the wrong answer: the two mosaic pages share
  `MosaicTabHero`, My Library has `LibraryTabBand`, and **Settings keeps its own** — it shares only
  the arithmetic, having no hero, no mosaic, no title, and a bar on the `Theme.*` defaults.
  - **The seed has to be the header row's own floor rather than a plausible page width.** The
    failure is asymmetric: seeded wide, the bar believes it can afford five full-width tabs and
    draws them into a panel that can't seat them; seeded at the floor it draws the icons it would
    have drawn anyway and widens once. A **miniplayer → full swap remounts the page at a panel
    narrower than any plausible guess**, which is the reliable way to see the wide one. The floor
    is `2 * pad-lg + bar.compact-w + 2 * pad-md + search-w-max`, pinned as derived by
    `ui::settings::settings_page::tests::the_page_width_seed_is_the_rows_floor`.
  - **A seed obliges the mount `Timer`**: `changed` doesn't fire when the first layout settles
    directly on the final value, and a window opened at its final size is exactly that — so
    without it a roomy window draws icon-only tabs until something resizes it.
  - **The compact tooltip is not on the tab**: the bar publishes `hovered-idx` (-1 for none) and
    `hovered-label`, and the host hangs one `Tooltip { side: TooltipSide.below }` off a frame
    declared **after** both `OverlayScrollbar`s. Equal-width cells let the host derive the rect
    from the index, so a tab that moves under a parked pointer (Ctrl+B, F11) keeps its anchor, and
    since `tab-w` eases the frame morphs with the cell for free. The gate stays on the instant
    `compact` — `Tooltip`'s 500 ms reveal outlasts the morph, so it can't surface while the label
    is still readable. **Only the tab that owns the hover may retract it, keyed on its index**, a
    data-agnostic bar being handed two tabs with one label otherwise letting a leave blank a live
    one.
- **The Rust half is `src/ui/tab_bar.rs`**, and it is one function: `clamp_tab(tab, tab_count)`.
  All four hosts persist their active tab and all clamp on *read* against the Slint-declared
  `tab-count` — the bar can only produce a valid index, but a file left by a build with more tabs
  would select a branch that mounts nothing. The component's two source-level invariants (the
  written-not-bound `compact-t`, the root's `clip`) are pinned there too, no host owning the file.

### `LibraryTabBand`

**The band that *morphs*** (`components/hero/library-tab-band.slint`). Idle it is a flat
`Theme.base` pane carrying the tab bar, the shared filter box and the mounted tab's count; with a
detail open it grows into that entity's hero at `MosaicTabHero`'s `hero-height` exactly — the same
formula, so the two bands agree on what a hero is. One animated float `hero-t` drives the height,
the backdrop reveal, the back slot, the palette and the count line's exit. **The two bands are
siblings and not one parameterised component**, differing in the mosaic-square-versus-artwork-tile
and in a fixed height versus a morph, which Slint has no way to abstract over — so the header-row
fixes are *ported verbatim* and `ui::library_tab_band_tests` exists to hold a copy to a contract a
copy is exactly how you lose. **Two things can't move into either band and stay at each host**: the
per-tab `ActionPill` rows, which arrive as `@children` placed after the meta block's trailing
spacer (also the slack `HERO_MAX_ROWS` is measured against), and the tooltip frame.

- **The height is a `min-height`/`preferred-height`/`max-height` split with `clip: true` on the
  root, never a bound `height`** — the animated-root-dimension pitfall, one axis over from the tab
  bar's. The clip is what the split obliges: it buys the freedom to be drawn shorter than asked,
  and the hero content would otherwise paint out of the compact band into the body.
- **`hero-t` is *written* from `changed detail-open` and only seeded by its binding**, so a page
  entered with a detail already open lands at hero height instead of growing into it, and a detail
  id moving underneath can't re-base the curve.
- **One duration, two curves.** The entry takes the standard container-transform curve
  (`cubic-bezier(0.4, 0, 0.2, 1)`) and the collapse keeps `cubic-bezier(0.2, 0, 0, 1)`; everything
  hanging off `hero-t` inherits the split. **The duration deliberately does not follow** — the
  collapse `Timer` and the detail body's `ViewTransition` are both `dur-spatial`, and a second
  number here is one that can drift from them. `Theme` cannot carry an easing, so both curves are
  spelled at two sites and `library_tab_band_tests::the_palette_crossing_is_anchored_to_the_morph`
  is the only thing holding the copies together. **That split is also why the state transition is
  `in`/`out` rather than `in-out`**: the colour is the same gesture as the geometry, so it has to
  run the geometry's *shape*, and one block can only carry one curve.
- **What may hang off an `if` is decided by change trackers, not by taste.** Everything
  tracker-free rides one `hero-shown: detail-open || hero-t > 0`. The chip strip **cannot** join
  them — it carries `MetaChipStrip`'s `changed watched-w` over a layout property a morph re-dirties
  every frame, so dropping its branch panics; it stays mounted and fades by brush alpha instead.
- **Nothing carrying content answers `opacity` any more.** An `Opacity` layer is sized to child
  *geometry* and a text run's ink leaves its line box, so the title block cropped every Arabic mark
  for the whole 400 ms. The artwork tile takes `ArtworkImage`'s own `fade` float, landing on the
  fill through **`transparentize`, which multiplies alpha — never `with-alpha`, which sets it**.
  The back disc is the case where folding into the brush is actively wrong, `IconButton`'s brushes
  feeding its two `animate` blocks: the cure was to make the faded element satisfy `need_layer`'s
  second bail, moving the glyph out of `disc` to be its sibling so `disc` is childless and its
  `opacity` free. The glyph's `x`/`y` are load-bearing rather than stylistic — a centring layout
  folds its info, and the animated `press-scale` behind it, into `IconButton`'s own `layout_info`.
  Full argument in `slint-pitfalls.md`.
- **The count line is anchored to the *compact* floor and the pill slot to the animated one**: a
  detail's pills belong on whichever floor is current, while the idle count belongs where it
  already sits. Its travel is an *offset* off that fixed anchor and **it does not leave the way it
  comes back** — out is a 12 px drop, back is a slide in from the left. `detail-open` picks the
  axis, being the only thing in scope that flips on the *edge* where `hero-t` knows only how far
  along the morph is. Down because the back slot eases open from zero on the same clock, and 12 px
  because the compact floor leaves sixteen under the line's box, so the drop fits inside the band
  it started in. The alpha is one expression in both directions (`count-t`, a *bias* off `hero-t`
  so the line doesn't read through the artwork tile fading in behind it) and the drop is that same
  float **linear**, which leaves the sentence in step with the pill row it shares a floor line
  with. Still no second clock and no `states` entry: a transition animates on its first evaluation
  and would flash the count on a page mounted straight into a detail. Pinned by
  `library_tab_band_tests::the_count_line_drops_out_of_a_fixed_anchor_and_returns_from_the_left`,
  whose load-bearing assertion is the one on `count-dy`.
- **The back disc scales on both dimensions with `hero-t`**, so every frame is a uniform scale of
  the settled row and there is nothing for the slot's clip to cut; its alpha is a doubled bias for
  the same reason the count's is biased — it is the only piece of the hero whose *size* also rides
  the morph, so a plain alpha would make its presence go as `hero-t` squared and read as a pop.

## The three tabbed pages

Favorites (nav 2), Recently Played (nav 8) and My Library (nav 3) share one contract. **Read this
block first; each page's section below lists only its deltas.** The nav-index map itself is in the
root `CLAUDE.md`.

- **`<Page>.tab-count` is the sole definition of how many tabs exist.** Rust clamps the persisted
  index through `ui::tab_bar::clamp_tab`, never its own const, and a per-page test `include_str!`s
  the `.slint` to pin the number against the `tab-*` constants, the body branches and both inline
  `@tr` arrays.
- **Build only the mounted tab's rows.** Sub-views are mutually exclusive `if`s, so anything
  prepared for an unmounted tab reaches nothing; the write side then drops a prepared result whose
  tab moved. Content hashes still come off the **source** entities, so one walk answers the
  signature for the tab it didn't build.
- **Counts hold `ui::tab_bar::UNFETCHED_COUNT` (`-1`) until fetched**, and the section leave puts
  them back there beside the model clears. A count outliving its model suppresses an empty state
  over an emptied model; resetting to `0` instead asserts "nothing here" for the length of the
  re-fetch. `-1` matches neither `== 0` nor `> 0`, so every existing gate expression keeps working —
  the one reader splitting on both is `MosaicHeroTile`, which clamps `max(…, 0)`. The five library
  counts are interpolated into gettext plurals, so each is read through a `>= 0` ternary (a ternary
  rather than an `if`, so the `Text` keeps its slot); a sixth inherits both obligations. **Tracks is
  the exception, and the exception is the rule read precisely** — what obliges the rewind is the
  leave dropping the rows, and its leave doesn't, so **rewind if and only if you clear, and if you
  clear, mark dirty**. Pinned by `ui::tab_bar::tests`.
- **A tab *pick* rewinds too, not just a leave.** A pick runs a synchronous apply against whatever
  cache is there, which the leave or a skipped tick can have emptied, so the apply would write `0`
  over the sentinel and assert an empty library for the length of the fetch already on its way.
  Three sites do it: `my_library::filter::clear_mounted` (excluding Songs, whose model survives the
  leave, and the four details, which write no grid count) and both curated pages' `on_tab_changed`.
- **Counts are written *above* the signature guard.** A pick stamps a signature against the cache
  it just walked, so a fetch returning identical content lands on the guard and a count written
  past it never arrives — stranding the Shuffle pill and sort row as well as the empty state. Safe
  because when the guard fires the model already holds exactly the rows the count describes, and
  `Property::set` is value-compared, so a redundant write stores nothing and dirties no dependent.
- **A gated fetch owes three things**, each a bug on the way in: the fetching branch **consumes**
  its dirty flag (seeded `true`, else a boot onto that tab pays for its own fetch twice);
  `release_section_state` **re-arms** it beside the cache wipe rather than leaving it to the
  leave's `mark_dirty` two files away; and the fetch re-arms on either way of storing nothing
  (failed query, or a leave landing mid-flight), since the pick consumes the flag *before*
  spawning.
- **Section guards sit *after* the slow part** — before it, the leave hasn't happened yet, so the
  two fetchers that `.await` a cover prewarm ask twice. Every store `release_section_state` wipes
  goes under the section gate; the `section_active()` bails are what order the two.
- **A tab pick can't await a prewarm the way a fetch does.** `TabBar` writes `selected-index`
  before it emits `selected`, so the entering `if` is already true when Rust hears about it. Rows
  therefore go in through the **non-hopping** `apply_*_now` twin (`apply_filtered_grids_now`,
  `apply_filtered_tracks_now`, `apply_filtered_grid_now`) — `slint::invoke_from_event_loop` posts
  even when called from the UI thread, and a redraw winning that race paints a bare panel or a
  `TrackList` of headers over an emptied model.
- **Covers ride the `covers-generation` gate**, and **the bump comes off `should_announce_warm`,
  never off the write** — see the Covers section.
- **Signature skips** are keyed on the tab *and* the column count beside the mounted tab's
  contents (the `last_mosaic_paths` pattern, reset on leave). Hashing both tabs together, or
  dropping either of the first two, silently skips exactly the apply that had to run.
- **A leave owes `mark_dirty()` for exactly what it hands back** — a tier, a model, a count.
  Tracks' leave releases nothing, so it owes none.
- Shared helpers: `ui::tab_bar::{clamp_tab, grid_signature, should_announce_warm, UNFETCHED_COUNT}`,
  `ui::grid_rows::{chunk_entity_rows, write_grid}`, `ui::track_list_cache`,
  `ui::mosaic_hero::impl_mosaic_hero!`.

### `ui/favorites/` (nav 2)

Three tabs under the shared `MosaicTabHero` band: Songs (sortable `TrackList`), Most Played and
Favorite Artists (virtualized, uncapped `EntityCardGrid`s — the grids virtualize, so neither query
truncates). One scroller per tab, so plain `OverlayScrollbar`s rather than `CompositeScrollbars`.

- `FavoritesUi::swap_tab_covers` releases the departed grid's tier and prewarms the entering one's
  first screenful (`GRID_PREWARM_AHEAD`, not the tier capacity — the grids are uncapped, so warming
  everything evicts its own work).
- **Two dirty flags, not three**: `refresh_grids` is one fetch feeding both grid tabs, and
  `refresh_hero` stays **ungated** (it answers the count, running time and mosaic, which the band
  states on all three tabs; `get_favorite_stats` carries its own `artwork_paths` and reads neither
  grid cache). `Favorites.track-count` comes from that ungated fetch, which is why Songs owes no
  count rewind. `kick_full_refresh` is a `tokio::join!` awaited inside `slint::spawn_local`, so
  every synchronous stretch between its `.await`s runs on the event loop.
- **Two sorts under separate `view_sort` keys** — `"favorites"` (Songs' `TrackList`) and
  `"favorite_artists"` (the Name / Favorites pill row) — **both resolved in memory**. The fetch
  **lost its sort parameters entirely**: `get_favorite_tracks`/`get_tracks` take only the state and
  `queries::track::track_list_order_by` is gone, replaced by one fixed `TRACK_LIST_ORDER` const
  that buys determinism (`ui::track_sort`'s tie-breaker is `sort_key` under a *stable* sort).
  `refresh_tracks` computes the permutation before the section guards let it store
  (`store_in_order`), the cover prewarm walking in display order ahead of the store; it re-reads
  the sort **twice**, before and after, because a header click landing mid-store would otherwise
  leave the header naming one order and the list showing another.
- **The artist sort applies to the cached `Vec<FavoriteArtist>`, not the filtered copy
  `build_filtered_grids` builds** — `first_screenful_paths` reads that cache to pick prewarm
  targets, so sorting downstream warms the covers of whichever artists SQL returned first while the
  grid paints a different prefix. `set_artist_sort` moves shadow and rows in one call. Filtering
  preserves order, so one sort serves both; `grid_signature` needs no sort input, `artists_content`
  being a *sequential* hash.
- Which tab is mounted comes off a `FavoritesTab` shadow on `FavoritesUi` (the fetchers run off the
  UI thread); `ui::favorites::seed_tab` seeds both it and the Slint property as the last statement
  of `favorites::install`. Both grid tiers are display-tuned by `ui::favorites::tune_cache_for_display`.
- Tests: `ui::favorites::tests::{tab_count_matches_the_tabs_slint_declares,
  each_gated_fetch_is_armed_beside_the_cache_it_fills, a_grid_pick_rewinds_the_count_it_could_not_answer,
  the_grid_counts_are_written_before_the_signature_can_skip_them,
  every_sort_pill_asks_for_a_field_the_comparator_knows,
  the_songs_model_is_written_only_while_its_tab_is_mounted}` and
  `grids::tests::only_the_mounted_tabs_rows_are_built` (mutation: building the wrong tab, which the
  write side would otherwise absorb in silence).
- Trees. Rust: `favorites/mod.rs` (handle + teardown), `tabs.rs`, `covers.rs` (four tiers),
  `rows.rs`, `songs.rs`, `grids/{fetch,apply,warm,sort}.rs`. Slint: the band is
  `components/hero/mosaic-tab-hero.slint`, the bodies
  `views/favorites/{songs,most-played,artists}-tab.slint` (the `views/settings/pages/`
  precedent), each keeping its own `OverlayScrollbar`s since Slint can't read an id declared inside
  an `if`.

### `ui/recently_played/` (nav 8)

Two tabs under the same band: Songs (the 200 most-recently-played rows — `get_recently_played`,
`last_played DESC`, index `idx_tracks_last_played`, migration `20260705000000`) and Most Played (a
virtualized `EntityCardGrid` over the uncapped, library-wide `get_most_played`). Near-mirror of
Favorites (`tabs.rs`, `covers.rs`, `rows.rs`, `songs.rs`, `grid/{fetch,apply,warm}`); deltas only:

- **`get_most_played` lost its `LIMIT` and its `.clamp(1, 100)`** when the ten-card carousel became
  a grid, the same trade `get_most_played_favorites` made.
- **Membership and order of Songs are fixed** — search re-walks the cached `tracks_all` in memory,
  never re-querying and never re-ordering.
- **The band states the recency set, not the tab's** — the mosaic is the four most-recently-played
  covers, a play-count ranking under a "Recently Played" banner naming the wrong page. This is also
  why its `last_mosaic_paths` *clear* half matters most: `refresh_tracks` only calls in when the
  paths differ from what the guard holds.
- **Neither tab has a sort.** Songs is the one `TrackList` mounted `sortable: false` — the flag runs
  `TrackList` → `TrackListHeader` → `HeaderCell` and defaults `true`, so the other eight mounts opt
  in by omission, and the gate is `enabled: root.sortable` on the cell's `TouchArea`, which forces
  `has-hover` off and retires click, hover fill and cursor together. The order it replaced was a
  trap: `"recency"` is a synthetic field no header cell owns, so one click was unreachable to undo
  and it persisted. The general answer to that trap now exists (`next_sort_with_natural`) and is
  deliberately *not* what this page took — a cycle is for an order the user can leave and return
  to, where here the order **is** the page. So there is no sort state at all: no
  `sort-field`/`sort-dir`/`request-sort`, no `ViewSort` in `RecentlyPlayedUiState`, no
  `view_sort["recently_played"]` (`shutdown.rs` prunes the key older builds wrote; a per-tab sort
  would need a *different* key, the `favorite_artists` precedent). Column widths and visibility
  still persist — the flag retires the *order*, not the header. **The middle link is the one worth
  pinning**: drop the `TrackList` → `TrackListHeader` forward and the mount still reads
  `sortable: false` while the page sorts again, so a pin covering only the two ends can't fail on
  the likeliest edit (`ui::recently_played::tests::{the_recently_played_list_is_not_sortable,
  the_sortable_flag_reaches_every_header_cell}`).
- 2nd subscriber to `stats_changed_tx`, on both counts (`last_played` for Songs, `play_count` for
  Most Played, same flush). **One `RecentlyPlayedUi::grid_dirty` flag for both tabs** where My
  Library uses a per-tab `SectionActiveGate`.
- **Its filter walk is the one apply path that may not run on the UI thread** — a settled keystroke
  folded the needle against every played track and hashed each survivor's six strings, on the event
  loop, every 130 ms. `apply_filtered_grid_settled` builds on a worker; what deferring costs is
  ordering, since `write_filtered_grid`'s signature check reads a *stale* set as a change rather
  than as staleness and so cannot be what stops the loser. Hence `RecentlyPlayedUi::filter_generation`
  (the `BrowseUi::fetch_token` shape), bumped by every `set_filter` and checked **twice**: on the
  worker to drop a walk not worth posting, and on the UI thread where a newer keystroke can land
  mid-post. The Songs walk stays on the UI thread, bounded by the 200-row set; Favorites' equivalent
  stays synchronous, its Most Played query being `is_favorite AND play_count > 0` rather than
  library-wide.
- **Sidebar placement**: routing `index: 8` but sits directly under Favorites in `sidebar.slint` —
  visual order follows source order, not index value.

### `ui/my_library/` (nav 3)

Five tabs (Songs, Albums, Artists, Genres, Playlists) under the shared `LibraryTabBand`; drilling
into an album/artist/genre/playlist keeps the user on the page and **morphs the band into that
entity's hero** rather than routing anywhere. **Nothing about the five views' data layer changed**
by the fold — same globals, models, `*Ui` handles, `fetch_grid`/`open_*`/`refresh_detail`, cover
tiers, `view_id` keys, `view_sort`/`view_columns`/`last_detail_ids` entries; `shutdown.rs` prunes
nothing. Five things are its own:

1. **A tab switch is a section switch**, so the page needs none of the curated pages' cover
   machinery — no `covers-generation`, no `swap_tab_covers`, no `prewarm_tab_covers`, no warm
   announcement — the entering tab's own fetch prewarming its first screenful before writing rows.
   Accepted cost: a tab pick is a full re-query, where keeping the departing tab's models alive
   would cost five tabs' row `Vec`s resident at once. **The one hook that does *not* work
   untouched is the hero teardown**, and the page's own `MyLibrary.page-active-changed` seam is
   what it exists for; see the hero-teardown rules and the `SectionActiveGate` bullet above.
2. **One search box for nine surfaces, dispatched in Rust** — see the Filtering section. Five
   `changed` mirrors in the sheet, not four.
3. **A drill's origin is a *section*, and a drill inside the page has none.**
   `AlbumDetail`/`ArtistDetail`/`GenreDetail` each carry an `origin-nav-index` stamped
   synchronously by the cross-view hand-off; `cross_tab_nav::origin_stamp` writes `-1` when the
   drill *started* on this page, and the close handlers then restore nothing. **The band's back
   arrow means "close this detail"**, and the tab bar names the detail's own tab for the whole
   visit — so Artists → artist → album → back lands on the **Albums grid**. Restoring the origin
   tab made the arrow contradict the bar beside it, and disagreed with itself besides, a Mouse-5
   *forward* re-open going through plain `open_album` and never re-stamping. Mouse-4/5 keeps true
   history semantics. `cross_tab_nav::Origin` stays a `{ nav, tab }` pair, but only the `tab`
   half's *second* job survives: `still_current`, the guard that stops a mid-fetch tab move from
   yanking the user, which the nav index alone can't answer with five views on one index.
   `PlaylistDetail` has no origin and needs none. **Each same-tab grid open zeroes a stale one**
   (`ui::my_library::tests::every_grid_open_zeroes_a_stale_origin`).
4. **`nav_history` replay has three arms, not two.** `NavEntry` gains `tab`, and
   same-section-different-tab is neither existing branch. **When a detail is coming, none of that
   navigation is written up front** — the close, the direction mark, `persist_tab` and the section
   flip are bundled into a `PendingNav` handed to `open_*_with`'s `on_applied` hook, so they land
   in the same UI-thread tick as the id. Written synchronously, the body router mounts the
   destination tab's *grid* for the whole DB fetch plus artwork decode the id waits on.
   **`open_playlist_with` exists because of this**, Playlists having been the one of the four with
   no hook. The obligation that comes with deferring is that *every* path skipping the hook lands
   the navigation itself (a missing view handle, each open's `Err`) or the press does nothing at
   all; the newer-replay bail is the one that correctly lands nothing.
   - **The bundle may not carry a precomputed verdict about the section.** `apply` runs the *close*
     first, and a detail reached by a cross-section drill carries an `origin-nav-index` its
     `close-detail` restores — so "did this walk cross a section", answered where the walk started,
     is a different question from "is the index where the target names" by the time the flip reads
     it. `PendingNav::apply` therefore reads `Nav.selected-index` live, which also still skips the
     redundant persist an ordinary same-section move would pay.
   - **Recording is the other half** — without `NavEntry.tab` two tabs of one page are the same
     snapshot and `record`'s dedup swallows the second — and **`on_tab_changed` is the one site
     that pushes one**, a pick being the only tab move that is the user's own navigation.
     `MyLibrary.persist-tab-idx(int)` sits beside `tab-changed` for exactly the moves that aren't a
     pick and stays silent, the drill recording its own destination a moment later. Pinned by
     `my_library_tests::{a_history_walk_lands_the_tab_beside_the_detail_id,
     the_replay_flips_the_section_against_the_index_the_close_left_behind}`; the first's mutation is
     hoisting `persist_tab` back above the spawn.
5. **The retired indices left three maps behind**, each failing differently and each fixed:
   `app-window.slint`'s `view-title()` (an untranslated "Melodia" heading),
   `rss_sampler::format_view` (index 3 now reports `MyLibrary(<tab>)` and gained the
   `PlaylistDetail` arm it never had), and the persisted-index range check, where a released user's
   `views.json` holding 4–7 would land on `PlaceholderView` — `ui::my_library::fold_retired_nav_index`,
   a pure fn so the compatibility path is testable without a window.

Trees. Rust: `ui/my_library/{mod,tabs,filter}.rs` (the tab enum, the stateless `seed_tab`,
`tab_is_mounted`, the dispatcher, `close_open_detail`) plus `ui/my_library/callbacks.rs` (the page's
five handlers). **There is no `MyLibraryUi` handle** — the five views keep their own
`section_active` shadows, which the per-tab gate now makes mean "the page is up *and* my tab is
mounted", and every other reader of the tab already holds an `&AppWindow`.
`ViewStateData.my_library_tab: i32`. Slint: `views/my-library-view.slint` is the mount sheet (the
band, one `GridGeometry` for four tabs, the nine-branch body router, two tooltip frames), with
`views/my-library/{songs,albums,artists,genres,playlists}-tab.slint`,
`views/my-library/{album,artist,genre,playlist}-detail.slint` and `views/my-library/tab-pills.slint`
(all nine `@children` pill rows in one file).

## The Settings page

- **The page is tabbed** — 5 tabs (Library, Playback, Interface, Services, About) over the same 12
  section cards. `views/settings-view.slint` is page chrome only (title, `SearchBar`, `TabBar`, the
  scroll body, its overlay scrollbar); `views/settings/settings-tabs.slint` is the router and
  `views/settings/pages/*.slint` the five tab pages, each owning its section list *and* an
  aggregate `has-matches`. **Search escapes the tabs**: a non-empty query mounts all five pages at
  once, which is how the cross-tab flat list comes back with no extra filtering logic, and why both
  modes mount the same five pages — the card-to-tab mapping is stated once. The no-results verdict
  is therefore 5 terms, not one per card.
- **The search box takes two properties.** `SettingsPage.search-input` is what the `SearchBar`
  two-way binds, `search-query` what the sections read, joined by a `FilterThrottle` on the same
  130 ms as every other filterable view. Mounting all five pages is what makes a keystroke
  expensive here with no model in sight — `row-visible` is up to three `matches` round-trips into
  Rust across dozens of call sites, plus `ChipGroup`'s hidden ruler re-laying out behind them. Both
  properties are cleared together on nav-away and on a tab pick. Rust-side, `on_matches` memoizes
  the fold against the raw needle, this being the one `row_match` caller that can't hold a folded
  shadow — it is invoked per *field*, not per pass.
- **A new tab** is one page file + a `tab-*` index + two symmetric router lines + one entry in each
  of the bar's two inline arrays + a `tab-name:` on every section that page mounts.
  **`SettingsPage.tab-count` is the sole definition of how many there are**, and
  `ui::settings::settings_page::tests::tab_count_matches_the_tabs_slint_declares` `include_str!`s
  all three files to pin the number against the `tab-*` constants, the router branches, the
  search-branch mounts and both arrays. The bar's `labels` must stay an inline `[@tr("…"), …]`
  literal in `tab-*` order.
- **The tab's own name is part of each card's search term**, threaded down as an
  `in property <string> tab-name` — search escapes the tabs, so a tab name is what a user types,
  and "Interface"/"Services" appear in no card's text otherwise. It reaches `row-visible` as a
  **term of its own**, not as a prefix each section splices onto its title: the join then lives in
  the global instead of twelve copies, and no card can match a substring spanning the seam. A mount
  that forgets `tab-name:` still matches its own title, so the page looks right and only the
  tab-name query comes up empty — `…::every_mounted_section_carries_its_tab_name` pins it, and
  asserts one page file per declared tab besides.
- **Anything that has to fit a width reads `SettingsPage`, and never measures.**
  `settings-view.slint` publishes two facts imperatively — `page-w` (the panel width it already
  mirrors; a live `root.width` read feeding a child's size re-enters layout) and `body-cols` — and
  the global derives `body-w`/`card-w`/`row-content-w` from them. Every card spans a full body
  column, so those are exact rather than approximate, and a control four levels down can size
  itself with no width mirror, no mount `Timer` and no `parent` reach-around. **`row-content-w` is
  the one to want**: the width a `SettingRowStacked` leaves its content block, which is what
  `ChipGroup` and `ColorDotGrid` default their `avail-width` to. Override the property at a call
  site that isn't a full-width card row; don't recompute the number.
- **`card-cap` (800 px) is one number both layouts obey**: a card grows with the panel to the cap,
  stops, and takes margins from there, and a second column appears exactly when two *capped* cards
  fit beside it — so a card's width only ever grows and the flip resizes nothing. A threshold
  spelled independently of the cap is the failure to avoid: set below it, the column divides before
  the cap is reached, and what you see is a card that grows without limit and then halves — the cap
  still in the source and simply unreachable. Pinned as derived by
  `…::the_two_column_flip_reserves_two_full_cards`. Latched with a 24 px band (the `TabBar`
  reason), and **never while searching**: search mounts all five pages and hides the non-matching
  cards, and a hidden card still claims its grid cell.
- **A page hands the `GridLayout` one cell per *column*, not per card** — each cell an
  `alignment: start` `VerticalLayout` of cards — and the columns place themselves through
  `SettingsPage.grid-row(i)`/`grid-col(i)`. That indirection is the whole reason the body reads as
  masonry: cards as cells are row-aligned, and a grid row is as tall as its taller cell, so a short
  card left a hole down to the next row. A page's columns are **contiguous halves** of its card
  list, so stacked they read in source order. Every card is still **mounted exactly once** — a page
  reads `has-matches` back off its section instances, and an element inside an `if` branch can't be
  read from outside it, so a one-branch-per-column-count arrangement would break the no-results
  placeholder. `…::every_column_takes_its_own_cell` pins that the indices run `0..n-1` once each
  and that no page declares more than two.
- `SectionCard` pins `preferred-width` *and* `max-width` to `SettingsPage.card-w`, leaving
  `min-width` alone — the preferred is what makes the two grid columns equal (Slint seeds a column
  at its cells' preferred width and only then hands out surplus by stretch), the max is what holds
  a card at the cap in a column wider than one card.
- **Nothing has a width floor any more**: the content column's old `min-width: 640px` is gone along
  with the horizontal `OverlayScrollbar` that existed to pan it, both row components' labels take
  `wrap: word-wrap` (a no-wrap `Text` reports its full width as its layout *minimum*, which no
  narrowing can negotiate), and `ChipGroup`/`ColorDotGrid` wrap. Don't reintroduce the bar — a
  settings page that needs one is a page with a row that has stopped being able to wrap.
- **The wrapping strips wrap through Rust, because Slint can't build a nested array.**
  `SettingsPage.chunk-indices(count, per-row) -> [[int]]`
  (`src/ui/settings/settings_page.rs::chunk_indices`) splits `0..count` into index groups and the
  strip iterates two real arrays. That shape sidesteps both traps the predecessor was built around:
  Slint rejects an inline `for … : if …`, and a `Rectangle` wrapper around a filtered-out item
  still claims its parent layout's spacing — which is why `ChipGroup` used to be a hand-unrolled
  4+4 split capped at eight chips. `wrap-per-row(avail, item-w, gap)` and
  `wrap-height(count, per-row, cell-h, gap)` sit beside it, both having been derived identically at
  the two call sites.
- **How many fit is measured, not estimated.** `ChipGroup` mounts a hidden `visible: false` ruler
  of one real `Chip` per option and takes `measure.preferred-width`, a `VerticalLayout` reporting
  the max of its children's widths. Same idiom and same reason as `TabBar`'s ruler: the measured
  and drawn shapes can't drift, and it follows the running locale for free. Sizing every row off
  the widest is deliberately conservative — the chips keep their natural widths, so a row is never
  full but can never overflow, and no per-item measurement has to escape the repeater.
  **`min-width: 0px` on the root is load-bearing**, stopping the ruler leaking a floor back out
  into the card. `ColorDotGrid` needs no ruler (32 px dots — the count is arithmetic) and flips its
  tooltip to `below` on every row after the first.
