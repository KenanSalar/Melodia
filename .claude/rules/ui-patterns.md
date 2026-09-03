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
  `PopupWindow`, so it captures no input and the host keeps its hover. Takes
  `host-width`/`host-height` explicitly, a component root having no `parent`. **A variant is only
  half of a side**: the `x`/`y` ternaries fall through to the *centred* arm, so a new `side`
  without its own arm puts the pill on the host and looks deliberate.

- **Two mount shapes.** In-tree is the default. **Top-layer** is for hosts whose pill lands where
  Slint paints later (bands, header strips): `components/tooltip-frame.slint`'s `TooltipFrame`,
  declared *after* the occluder, tracking the host via `absolute-position` deltas the host spells
  itself. `sidebar-tip` is the deliberate exception and should stay one — its `x` rides the live
  rail width, it owns a `held` latch, and it is the only frame passing `gap`.

- **A band publishes an anchor and draws no pill.** Pinned by walks, not lists —
  `no_page_or_shell_mounts_a_bare_tooltip` over `views/` **and `layout/`**, and
  `no_shared_band_draws_its_own_tooltip` over `components/hero/`.

- **An anchor crossing a component boundary has three answers**, a frame reading only ids in its
  own file: `out` properties (the hero bands), a global (the sidebar rail, whose boundary the
  frame's file doesn't contain), and a zero-width `clip: true` collapse rather than an `if`
  (`tab-pills.slint` — an id inside an `if` is unreadable from outside, and `Clip` swallows every
  event outside its empty rect, so the row is as unreachable as an unmounted branch while its ids
  stay readable).

- **The rail's tooltip needs a hold the tab bar doesn't**, its rows sitting 4 px apart so a
  travelling pointer is over nothing for a frame or two: a `held` bool cleared by a 150 ms
  `Timer`, with `hovered: row-hovered || held`. **`held` extends and can't arm** (written from
  `changed`, which skips the first evaluation), hence `row-hovered` beside it — and
  `changed watched-mini-render` clears the stale index, the miniplayer swap destroying the rail
  with no unmount hook. A new unmount path that strands the rail owes the same clear.

- **The volume readouts anchor to a *point***, so each mounts the pill in a zero-size `Rectangle`
  with `0px` host dimensions and drives it off **`force-shown` rather than `hovered`** — a value
  readout is up on the frame the drag starts, where the reveal delay is for a label you linger
  for.

### Pills, chips, sort rows

- **`PillButton`** (`components/action-pill.slint`) — `danger: true` tints icon **and** label
  only; the filled destructive treatment is `SectionButton`'s, and the two are not one pattern.
  Compose inside `ActionPill` with `PillLabel`/`PillDivider`; `IconButton` for round controls
  *outside* chips.

- **`SelectionPills`** — the "{n} selected" + `close` group. **Mount it behind the host's own
  `if`, never with a count of zero**: the slots are unconditional inside, so a component hiding
  its own children still claims a `pad-xs` of the pill's spacing. `divider-trails` follows
  position, not taste. Playlist Detail's row stays hand-rolled, its destructive pill sitting
  *between* the count and the close.

- **`MetaChip`/`MetaChipStrip` are decorative** — no `TouchArea`, no selected state. The
  *interactive* pill is `chip-group.slint`'s `Chip`. Deliberately not one component: one states a
  fact, the other configures something.

- **`PopupSurface`** — every `PopupWindow` body wraps it. `pill: true` for vertical-pill.

- **A surface that floats over the app edges with `Theme.surface2`, never `Theme.border`.** All
  four Catppuccin variants define `border` as the same hex as `surface0`, so a `surface0` card
  bordered with it has no rim at all. `Theme.border` is still right for an **input field** on
  `surface1`. The test is the fill underneath, not the element's kind: lift a surface off its
  background, or recess a field into one.

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
  is the only caller; nothing paints the third state, no header cell matching, which is the honest
  reading of "unsorted". Reach for it only where the order is one the user can leave and come back
  to; where the order *is* the page, `sortable: false` is the answer instead.

- **`SortPillRow`** — the `TabBar` shape (parallel `labels`/`fields` + `request-sort`), at all four
  pill rows; every other sortable surface goes through a `TrackList` column header. Pills carry
  `reserve-sort-slot` + `sort-direction` for the trailing `arrow_drop_*` slot — never concatenate
  `↑`/`↓` into the label. It exists for what it makes *unspellable*: the per-pill slot, ternary and
  `active` comparison could each go missing at one site and look right until the sort moved. What
  the *mount* still owes is the two arrays lining up — a label with no matching field sorts by the
  empty string — so both pins check lengths through `test_support::sort_pill_row_arrays`. `labels`
  stays an inline `[@tr("…"), …]` literal.

### Grids, strips, cards

- **Every grid is `EntityCard` in a virtualized chunked-row `ListView`**: Rust chunks the flat
  list into grid-row structs of N, **`GridColumnsSync`** computes N from the width and fires
  `columns-changed` so a resize rebuilds the model without touching the database. Detail views own
  no header — the four under My Library are bodies, the banner being the page's own band.

- **A strip and a grid are different components on purpose.** `HorizontalCardStrip` walks a plain
  `for` — affordable for a capped carousel, not for an uncapped page, where every card is built and
  every cover requested. `EntityCardGrid` is the virtualized counterpart over the same row struct,
  and each of its three instances **dropped its query's cap on arrival**, the reason for a cap
  being the strip's plain `for`.

- **`GridGeometry`** turns `avail-width` + `min-card-w` + `gap` + `card-text-h` into
  `cols`/`card-w`/`card-h`/`row-h` — **feed it the *body*'s width**, which the layout fixes
  independently of the cards, so a card sizing itself from it is a derivation and not a cycle. My
  Library derives it once for four mutually exclusive tabs.

- **`GridEmptyState`** is the centred glyph + heading + copy block, and **not only for grids** —
  Browse's three states and Playlist Detail's empty list mount it too, which is what the optional
  `action-*` properties are for. Properties rather than an `@children` slot: an empty `@children`
  still claims a `pad-md` gap and shifts the vertical centring.

- **Chunk through `ui::grid_rows`**, the row chunking *being* the virtualization boundary.
  **`chunk_rows` borrows** (the entity grids project cards out of a `GridData` they keep),
  **`chunk_built_rows` moves** (the grid tabs and Browse build a flat `Vec` and drop it). Reaching
  for the borrowing form with `Clone::clone` as the projection is the trap the split closes — a
  second full pass cloning every card into the chunk about to replace it. `columns` is floored at
  one in both: a grid mid-layout can report zero, and one card per row is a visible wrong where a
  zero-width `chunks` is a panic. Grid tabs take `chunk_entity_rows` + `write_grid` (generic over
  the row type, so Browse uses it too). Don't re-roll either.

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
  reads as — `slint-pitfalls.md` argues why. A control that stays up while the card is idle is not
  a hover affordance: it goes *beside* the card on `IconButton.fade`, as the station star does,
  since the slot carries one fade for everything in it.

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
  (`boot::ui_setup::apply_backdrop_style` raises the flag ahead of `install_views` for that
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
  Playing, two globals kept separate because a band stays mounted behind an open Now Playing.
  Reading either global from inside a stack ties it to one tier and puts the other site's inline
  copy back; `hero_blur_backdrop_tests` pins both directions. **Two mount sites, one
  `aurora-shown` property, and only the live arm mounted at each** — each stack sits behind its own
  branch of that property, the two conditions each other's negation.
  **The branch is safe because the condition is a process constant**: `Theme.aurora-backdrop` is an
  `in` property written once by `boot::ui_setup::apply_backdrop_style`, ahead of `install_views` and
  `app.show()`, so no frame sees it move and neither leaf carries a `changed` tracker. The pair was
  mounted unconditionally with the loser painted transparent for as long as an art-less track could
  move the arm with the view on screen — that term is gone, and **a transparent loser was never the
  free option it reads as**: `Brush::is_transparent()` (`i-slint-core/graphics/brush.rs`) answers
  `false` for every gradient whatever its stop alphas, and femtovg's `draw_rectangle` tessellates
  the path before `brush_to_paint` looks at it, so a stack faded to nothing still filled the whole
  surface every frame — four of them on the blur arm.
  What the gate does **not** retire is `shown`: the mounted stack still drains to its idle colours
  through it (below), which is why each stack's `shown: false` arm stays a *gradient of the same
  shape* — `Brush::interpolate` blends stop-for-stop and only between matching types, so
  "simplifying" that arm to a solid `transparent` kills the drain.
  `hero_blur_backdrop_tests::every_backdrop_site_mounts_only_the_live_arm` anchors each gate at the
  head of its mount line, `if root.aurora-shown:` being a substring of the negation.

- **The six bands reach that pair through `components/hero/hero-backdrop-stack.slint`, not
  directly.** `HeroBackdropStack` is the pair plus the `aurora-shown` choice, bound to the
  `HeroBackdrop` tier — the twenty-five lines `MosaicTabHero` and `LibraryTabBand` had each. Now
  Playing stays a direct mount because it paints `Player.np-*`, which is the whole reason the two
  leaves take their colours as inputs rather than naming a global. The wrapper's own `shown` is
  the **host's** question — ungated for the two mosaic bands, which never stop painting a hero,
  and `detail-open` for `LibraryTabBand`, whose globals outlive the tab that filled them. That
  makes the gate a two-file deal and each half its own pin: the band passes the term
  (`library_tab_band_tests::no_hero_tier_outlives_the_banner_it_was_solved_for`) and the wrapper
  forwards it to each child
  (`hero_blur_backdrop_tests::the_wrapper_forwards_its_hosts_gate_to_the_mounted_stack`) — drop
  either and both files still read correctly while My Library paints a detail's backdrop flat.

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
  `hero_blur_backdrop_tests::no_backdrop_site_gates_the_aurora_on_anything_but_the_setting` reads
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
  painted whichever other detail was open, which `seed_detail_from_settings` makes routine.
  `ArtworkImage` takes `has-cover` defaulting `true`. Neither the blur quartet nor the washes need
  an equivalent: `has-blur: false` is exactly what a procedural backdrop is for, and a tint is
  always a colour — an entry with nothing of its own substitutes a seed rather than going absent.

- **A hero may publish into either shared global only while it is the one on screen.**
  `install_views` seeds **all four** detail views unconditionally, so a cold start fetches up to
  four details whichever section it restores and the last to finish would win. The gate is passed
  to `apply_detail_artwork` (guarding the `HeroBackdrop` write **and only that** — the cover and
  blur slots either side are the view's own, and writing those while hidden is what leaves the
  page ready to paint), `apply_genre_hero` and `hero_chips::publish`.
  - **On the four details it is a live `tab_is_mounted` read *after* `on_applied`, and both halves
    are load-bearing.** A section shadow updates only on the next frame, so a cross-section drill —
    which moves the tab from inside the closure that publishes — answers for the tab being left;
    and the shadow is the wrong *question* besides, going false when Now Playing covers the band.
    Hoisting the binding above `on_applied` compiles, reads correctly, and puts the bug back.
  - **Dropping a publish is only free if something later replaces it.** `SectionState::new` starts
    `dirty: false` so the boot pre-fetch wins the first enter, but a pre-fetch that ran off-screen
    filled nothing shared — so the four detail sections seed the flag themselves at wire time
    (`if !section_active() { mark_dirty() }`), reading `tab_is_mounted` rather than the nav index,
    five sections sharing index 3.

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
    untouched — `release_section_state` still drops the LRU, grid data and row models every leave.

- **`HeroChips` publishers take their facts as arguments or off their own section handle, and read
  back no Slint property** — lifting a count off the global the caller just wrote makes write order
  part of the contract. **Facts a stats row doesn't carry are folded on the worker, never in the
  closure** (`ui::hero_folds`), their `Copy` results riding into `upgrade_in_event_loop`; **Most
  Played sums itself**, its query being a strict subset of the Songs tab. **Moving a fold onto the
  worker moves its teardown with it** — `release_section_state` resets the folds beside the caches
  they summarise, a derived value outliving its source being the one thing the band can state that
  is *wrong* rather than merely absent.

- **A spread of one is stated as nothing**, and **a set of none as no band at all** — an empty
  hero publishes zero chips and leaves the copy to what the body already paints, so no page says
  "nothing here" twice. Each tab gates on its *unfiltered* count, a filter matching nothing being
  the empty states' business. **A band states facts about the set the page is about, never the
  current filter**: forced rather than chosen, an album's chips being unable to follow its track
  filter without lying about the album.

- **`HeroChipStrip`** is where a hero mounts `MetaChipStrip`: it fixes the `HeroBackdrop` brushes
  and forwards `measured` into `HeroChips.recompute`, making the wrong mount unspellable rather
  than merely tested against. `MetaChip`'s `Theme.*` defaults are correct nowhere and never taken —
  they exist so the file imports neither `Player` nor `HeroBackdrop`, which is what lets one
  component sit on a blurred cover *and* a blurred banner.
  - **`fade` multiplies both brushes** and only `LibraryTabBand` sets it — the strip carries a
    `changed` tracker, so a band wanting a fade-out must keep it *mounted* across the collapse, and
    `Opacity::need_layer` bails only at exactly `1.0`.
  - **Who chunks is the host's**, via `chunk_chips_to_rows`'s `max_rows` — `None` for the Now
    Playing column, `Some(HERO_MAX_ROWS)` for a band. **`HERO_MAX_ROWS` is 2, measured rather than
    picked**: a second row plus its gap fits the slack every hero's trailing spacer already leaves,
    where a third overruns the tile and pushes the pill out of the banner. If a wrapped row ever
    clips, the fix is a per-hero max into `write_rows`, **not** a taller band — the band clips, so
    the failure is bounded.
  - **The rows hang off a plain `VerticalLayout` pinned at `min-width: 0px`**, not off the root —
    the conditional-child pitfall, one shipped bug per axis. Without the wrapper the root reports
    `preferred: 0` and paints the second row outside its allotment; with it and without the
    `min-width`, the widest published row becomes a floor no narrowing can negotiate. The second is
    the one to remember, fixing the first being what introduces it.
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
  two Most Played grid tabs have no row index and resolve by id, **filter-aware** through
  `most_played_matches`, the same predicate the model build uses, so the cards and the queue can't
  disagree about what's on screen.

- **Detail-page inset.** The band is full-bleed, so a detail *body* may inset on its root like any
  grid page — **Album and Genre do; Artist and Playlist can't**, Artist's `below-hero` being the
  region `CompositeScrollbars` measures and Playlist's empty state and drop banner filling `body`.
  Overlay scrollbars stay at `parent.width - self.width` either way; bottom padding is never on the
  root (the dead-strip pitfall). **The horizontal bar's clearance is a lane inside the list
  instead** — `reserve-scrollbar-lane` on `TrackList` / `DraggableTrackList`, which pads the column
  *inside* `outer-scroll` by `Theme.scrollbar-slot`. **All eight bar-pair hosts set it, composite
  ones included** — that bar takes the list's `x`/`width` and the view's bottom, which is the list's
  own edge the moment it hits the `below-sv.visible-height` cap. A composite host owes the lane a
  second time as a term in that cap's content-fit arm, which hand-sums rows + header + spacing.
  Search's songs section is the one opt-out, its bar being a layout sibling with a slot already.

### Dialogs, pickers, toasts

- **Selectable-picker dialogs share one toolkit** (`components/dialog/selectable-picker.slint`).
  Both the Export and Add-to-Playlist pickers are thin wiring over it, toolkit components staying
  data-agnostic. Both commit through the `Dialog.accepted` dispatcher gated on a selected count;
  Add-to-Playlist disables fully-contained playlists and counts only enabled rows. Toggles and
  commit live in `files.rs` (commit needs `Rc<NotificationsUi>`), the opener in `dialog.rs`.

- **Notifications stack** mirrors `Dialog`'s `kind`-routing — a new action is one branch plus one
  `show_localized(…)` call. Cap 5. Per-card props use `data:` not `row:` (Slint reserves `row` as
  the iter var), and translated strings reach Rust via `pure callback`s wrapping `@tr(…)` literals.

- **Backend-thread toasts via `utils::toast`.** `NotificationsUi` is `Rc`, so failures on tokio
  workers surface through a neutral `OnceLock<UnboundedSender<…>>` — no-op when uninstalled,
  holding no `ui::*` types, which preserves the `tasks`-no-`ui` rule. `install_toast_bridge` drains
  the `mpsc` (**not** a `watch` — errors must not coalesce), resolving the localized **title** by
  `ToastKind` and leaving the dynamic **detail** untranslated. Routine failures (favorite, rating,
  nav) keep `spawn_logged!` — don't toast-spam.

### Filtering

- **Every filter box answers "does this row match" through `crates/melodia-views/src/ui/row_match.rs`, and none of them
  spells `to_lowercase().contains(…)`.** It owns `search_fields` (ordered like the `tracks_fts`
  column list it mirrors), the `fold_needle` pair folding case **and accents**, and the predicates
  over them. Sixteen surfaces route through it; on all three tabbed pages the needle is **one
  shadow shared across the tabs**, cleared on both sides by a tab pick.

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
  search there (`library-data.md` owns that side). **The entity grids match raw fields, not their
  `*_lc` sort keys** — folding can't be baked into a lowercased string without changing the sort
  those keys drive.

- **Nine of the sixteen are fed by one box, dispatched in Rust rather than bound.** My Library's
  band owns the only `SearchBar`; `my_library/filter.rs::dispatch` routes a settled keystroke to
  whichever surface is up. It must be a *write*: an element can't declare a binding on another
  global's property, and the only form that would work puts `MyLibrary` in five globals' imports.
  - **The two contracts stay different underneath** — the five grid/list globals fire
    `apply-filter(text)` and Rust reads the property back inside a memoized rebuild; the four
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

- **No *uncapped* cover lookup decodes on the thread that asks.** `get_or_schedule_opt` answers
  from the tier and hands a miss to the decode pool, so the caller — a Slint model getter, so the
  event loop — gets the placeholder on that frame. What makes the placeholder temporary is a
  generation the same binding reads: the tier fires one notifier per landed *batch* and
  `ui::cover_generation::notify_on_decode` bumps the counter on the UI thread. Prewarm still covers
  the common case; this is for the misses it structurally can't reach. A prewarm is capped at the
  tier's own `CACHE_CAP` and walks **in display order**, so a library with more unique covers than
  that keeps its visible prefix and nothing else — and every row scrolled to past it used to decode
  inline, one at a time, on the event loop.
  - **Per batch, not per cover.** A screenful of misses coalesces into one pass over the mounted
    bindings, and a miss arriving mid-drain joins it rather than spawning a second.
  - **A cached failure is a hit**, so a broken cover re-queues nothing and can't spin against its
    own notifier.
  - **The notifier is a feedback loop, and two things stop it running away.** The queue is capped
    at the tier's own capacity, newest-wins, so a scroll can't queue more than the cache holds and
    then evict the visible prefix with the tail of its own batch. And a path the burst already
    handed to the pool is refused — a grid drawing more cards than its tier holds would otherwise
    re-queue whatever the last batch evicted, forever, at frame rate. A miss the burst hasn't seen
    clears that state, which is what tells a moved visible set apart from a thrashing one.
  - **A reset invalidates whatever is mid-decode.** `clear` and a genuine `set_thumb_size` bump a
    tier epoch, and a batch that captured the old one drops its buffers rather than landing behind
    the section leave that released them, or at a size nobody asked for. **The bump belongs under
    the *cache* lock**, that being the one a finished batch takes to insert: emptying the tier and
    invalidating the batch have to be one step, or a batch reading the epoch between them lands in
    the tier the reset just emptied.
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
  the queue sheet's *private* `CoverThumbs` (so closing it drops every buffer without yanking
  covers the track lists still need) and the shared row tier. That is what makes a queue the size
  of the library affordable.

- **`covers-generation` is the gate for a surface that fills with no fetch to hide behind.** A
  `pure` callback's result is cached until a dependency is dirtied, and every ordinary lazy-cover
  surface prewarms and **awaits before** setting rows, so its first evaluation is a cache hit.
  Three can't: the queue sheet's **synchronous open**, `EntityCardGrid`'s **tab pick** (`TabBar`
  writes `selected-index` before it emits `selected`), and `BrowseCardGrid`'s **mode toggle**.
  - The argument does two jobs — reading it makes the binding depend on the counter, and its value
    is the "is this tier warm" flag. **At 0 the Rust side answers cache-only**, so rows mounted on
    that frame don't even queue work for a tier a leave is about to clear; past 0 they take the
    scheduling lookup. Teardown rewinds to 0 beside the tier clear, so 0 keeps meaning "cold".
    Answer unconditionally and the flag half is dead weight.
  - **The bump has two callers and they mean different things.** `mark_covers_warm` is the
    prewarm's, and moves the counter off 0; **`repaint_covers` is the decode notifier's, and never
    does** — a batch landing after a tab-leave cleared the tier would otherwise read as warm and
    cost the next mount the cache-only frame the gate exists for.
  - **The three My Library grids carry the property without the gate.** Albums, Artists and
    Playlists never answer cache-only, so their counter is only ever the repaint token; 0 is a
    starting value there, not a state. Same for `RowCovers.generation`. Don't read a
    `covers-generation` as cold-gated without checking the Rust side.
  - **The bump is gated on the prewarm's verdict *and* a re-check on the UI thread**, because they
    fail separately: a pick made while decodes ran already rewound the counter, while a section
    leave landing mid-decode makes the prewarm hand its buffers back. **A prewarm that may release
    what it warmed owes its caller that `bool`.**
  - **Announce on the warm, not on the write** (`should_announce_warm`, fed a `warmed_tab` that is
    `Some` only on `Ok(true)` — a `JoinError` is the same "we don't know"). Whether the tier is
    warm and whether the rows moved are independent; a re-enter reliably lands on the signature
    skip, the mount-time `columns-changed` having written final rows mid-decode.
  - Browse rebuilds **without hopping the event loop** — `invoke_from_event_loop` posts even from
    the UI thread, and a redraw winning that race paints an empty grid. Its released tier also
    obliges a `mark_dirty()` on leave and the same wire-time seed, having no enter-time fetch.
  - **The Rust half is `grid_prewarm::grid_cover(thumbs, path, generation)`** — the tier and
    counter differ per page, the branch doesn't. Reach for it at a fourth surface: a copy that grew
    a **decoding** `else` arm reads correctly and puts the UI-thread decode straight back, which is
    the one thing neither arm does any more. The hero's `CoverMosaic` keeps the one-argument form,
    its tier being warmed by a fetch.

- **Cache cap via `grid_prewarm::cover_cap_for_window(app, fallback)`** — one band for every grid,
  they all draw the same card. Derives its cap from the **window's** own logical size against
  `GridGeometry`'s pitches, deliberately not the monitor's: the monitor caps against a screen the
  window may occupy a corner of, and asking costs a `with_winit_window` round trip that answers
  `None` for the whole window-less boot. The fallback is passed in so a module keeps its own
  default and a zero extent lands there.
  - **The pitches have to be the grid's, not a footprint estimate**: the cap has to come out at or
    above the cards actually mounted, or the scheduling lookup leaves the overflow on placeholders
    until a scroll. Margin comes from measuring the window rather than the grid's own box;
    `grid_prewarm_tests` pins the two against each other.
  - **The ceiling is bytes, not entries.** A decoded buffer costs the square of the tier size, so
    one entry count is two different budgets — and it was wrong the expensive way round, a large
    logical desktop being by construction a 1× one, where the buffers are a fifth the size and the
    same bytes buy several times the entries.
  - **Read after `app.show()` and again on every resize**, through `WindowChrome.display-changed`
    off the winit `Resized` filter. Read once, a cap sized for the launch window is one a later
    maximize overruns. It sets the cap exactly rather than growing it: a smaller window really
    does draw fewer cards.

- **Decode size via `grid_prewarm::cover_size_for_window(app)`, and it is derived from the card,
  not stepped off the scale factor.** `GridGeometry` packs toward `min-card-w`, so a card is
  *smallest* on the panels mounting the most of them: the two constants this replaced sized the
  wide case generously and doubled it for `HiDPI`, which made the displays paying for the most
  buffers hold each one at roughly twice the pixels it drew. `widest_card × scale` is the whole
  question, clamped to `STORE_MAX_DIM`, which is what caps a single-column panel's sharpness.
  - **The bound, never the card itself.** `card-w` sweeps `min-card-w` up to
    `min-card-w + pitch/cols` inside *every* column band, so a tier tracking it crosses a step
    boundary twice a band — and `display-changed` re-derives on every winit `Resized` while a
    genuine `set_thumb_size` clears the whole tier, so each crossing wipes every grid's covers
    mid-drag. Sizing to the band's upper bound is what makes the answer flat across a drag rather
    than merely close, and the step is what collapses the column counts a desktop passes through
    into a handful of tiers.
  - **The window is not the body, and `BODY_CHROME_W` is the gap.** The sidebar between them is
    the user's to drag from `sidebar-collapsed-w` to `sidebar-max-w`, a range no window
    measurement sees, so the estimate assumes the *widest* one: over-subtracting costs a tier one
    step too large, under-subtracting puts every card on an upscale, and `FemtoVG` minifies
    bilinear with no mipmaps.
  - `GRID_COVER_FALLBACK` is what a tier is *built* at, before any geometry exists. A fallback,
    not a tier size, but sized for the run where the deferred retune never schedules and it stays
    the live tier.

- **The cap and the size are read together**, in the same `tune_cache_for_display`
  call — two halves of one budget, and both are answers about the
  display. The derived size replaced a `448` copied into five files, each justifying it in its own
  doc comment off the same claim that flex-filled cards "run well past 260 px". They don't:
  `GridGeometry` packs toward `min-card-w`, so a card is **largest in a narrow panel** and lands
  near 190 px on a wide one. A tier spelling its own size is the thing to reach for this instead
  of. Needs no winit round trip, the scale factor being Slint's own, and shares the cap's
  zero-extent bail. `cover_thumbs::row_cover_size` is the row tier's twin, wired at each of its two
  construction sites rather than through a tune hook, neither having one.

- **Prewarm path dedup via `grid_prewarm::unique_artwork_paths(paths, cap)`**, first-seen-ordered
  and non-empty. **Every prewarm site goes through it**, the per-entity wrapper owning only the
  projection; don't lean on `prewarm`'s internal dedup as a reason to skip it, the two sites that
  did each grew a divergent copy. **`cap` bounds kept *paths*, not input items** — capping the
  input is what stops a detail over a 20 000-track genre allocating a `PathBuf` per unique cover to
  keep 512, and `prewarm`'s own `.take(cap)` sits **after** its already-cached filter, so over a
  partly warm tier an uncapped call decodes a full capacity from anywhere in the list, evicting the
  visible prefix to do it.

- **Artist Detail's Albums strip borrows `AlbumsUi.grid_covers`**, and the Artists wiring releases
  on **both** Artists section-leave and `on_close_detail`.

## Releasing what the UI pins

- **Detail-close releases global Image properties.** `release_detail_hero_images!` resets `cover` +
  `blur-img-a/b`, clears `has-blur`, and re-solves the two shared globals, alongside `clear_detail`
  + `release_detail_artwork`. Without it `SharedPixelBuffer` Arcs pin the cover tile and **both**
  blur slots, on the heap and again as Mesa textures. It runs on each view's **section leave**,
  gated per the hero-teardown rules above.

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
  Slint default literal — plus `heap_trim::trim`. **The teardown is a `public function`, not a
  callback body**: a callback has a single handler slot, so a default `closed => { … }` body is
  installed at construction and then silently replaced by the Rust `on_closed`.
  `CompositeScroll.reset()` takes this shape for the same reason. Do **not** clear in
  `accepted`/`cancelled` (unmounts the body mid-fade), and do **not** re-register `closed` from
  inside the handler (`Callback::call` `take()`s it and asserts). A new dialog kind pinning an
  image extends the single `on_closed`.

## Popups, native dialogs, input

- **PopupWindow auto-dismiss on OS focus loss** — `FocusLossWatcher`, mounted inside
  `if popup-is-open` so only the open popup has a live watcher. Singletons gate on
  `PopupHighlight.id`, the per-row context menu on `row-ctx-id == row-data.id`. Slint 1.16 has no
  `closed` callback, but `pop.close()` is a safe no-op when hidden.

- **Native dialogs (rfd) — always through `ui::file_dialog::parented(&weak, title)`.** The helper
  owns the `weak.upgrade()` + `.set_parent(…)` half; the caller chains its own
  `add_filter`/`pick_*`/`save_file`. **Never build one inline**, and the reason is that the bug is
  invisible here: without the parent it z-orders behind Melodia on Windows and macOS, while Linux's
  XDG portal parents OS-side regardless. Held by a test that **walks `src/`** rather than naming
  the five sites, and carries a caller floor so a caller that stops opening a dialog trips it too.

- **Keyboard shortcuts** — the root `FocusScope` is `ShortcutScope`, which owns every binding and
  takes the whole main layout as its children. The bindings are in that file; what isn't visible
  there:
  - **Ten `changed` mirrors regrab focus**, Slint not handing focus back to an ancestor scope when
    the focused item is destroyed, so a view unmounting with focus inside it leaves shortcuts dead
    until the next click. Nine are one per branch selector of an `if` chain that mounts a view —
    the same reason `CompositeScroll.reset()` needs its five, so a new always-mounted mirror
    unmounting a *composite* view owes both (this set is the wider). The tenth, `Queue.open`, is
    not about destruction: the sheet is permanent and mounts nothing, but *nothing else moves focus
    onto it*, so without the grab a filter `SearchBar` keeps the keyboard and eats the `a`.
  - Non-Esc gates on `!Dialog.open`; typed keys in TextInputs never reach root. The three queue
    bindings are mini-gated — the sheet doesn't exist in miniplayer mode, and arming `Queue.open`
    with no mounted mirror to fire `open-changed` remounts it rows-less.
  - **`ShortcutScope` is the only `FocusScope` in the tree, and the queue sheet is why that's worth
    stating.** A `FocusScope` takes focus on **mouse press**, never on mount, so the sheet's own
    nested scope left Esc and Ctrl+A dead on a sheet opened with `Q`. Both sit at the root now,
    gated on `queue-sheet-up` (`Queue.open && !MiniPlayer.active`, the flag alone surviving the
    miniplayer swap), which also fixes the priority — Esc reaches the *dialog* first. A second
    scope reintroduces all three problems; put new key handling here.

- **Animated view transitions** — content branches mount via `ViewTransition`: enter-only fade +
  32 px axis slide, no exit anim, Slint `if` destroying the outgoing branch instantly. Direction on
  `Nav.pending-enter-from`, written **synchronously just before** flipping the `if` via
  `ui/nav_transition.rs`.
  - **An unwritten edge is not a default — it is whatever the last navigation left in the global**,
    and `mark_drill_back` fires on every detail close, so the value sitting there is reliably
    `left`. *Every* Slint-side mount writes its own, `below` by construction, the two non-lateral
    directions being Rust's; the pin **walks** the tree. The miniplayer's mark isn't about a closer
    — the swap destroys the whole full UI, so the content branch remounts on the way *back*.
  - **Two inputs turn parts of it off and answer different questions**: `enabled: false` is "does
    this view animate at all", `slide: false` is "is anything *else* already translating it" —
    fade, don't move. Both default on, so a mount that owns its motion says nothing.
  - **The launch mount has a third off-switch, and it is a global rather than an input.**
    `Nav.suppress-enter-animation` carries the "Skip Startup Animation" setting, raised by
    `boot::ui_setup` before `app.show()` and handed back by the first mount that settles — so the
    window opens on its content and every later navigation still slides. A **live read inside
    `settled`** rather than a property captured at `init`, and the capture would work — no branch
    exists until the first layout pass, which is inside `app.show()` and long after
    `hydrate_ui_from_settings` — but it spends a property and a handler to dodge the one thing the
    live read owes: **the hand-back sits one statement after `shown` flips**, a clear landing on a
    frame where `shown` is still false fading the settled page straight back out. That is also why
    the hand-back can't be Rust's. Gated on **`enabled`**, since a nested body mounted at boot runs
    the same `Timer` and would otherwise drop the flag for the page above it. Pinned by
    `ui::startup_motion_tests`.
  - **A page with sub-views nests a second one and must disarm it at mount**, the page's own enter
    still playing when the first tab body mounts and a horizontal slide composed with a fade-up
    reading as a diagonal. The host arms it in the tab bar's `selected` handler; starting `false`
    is what makes the page re-disarm for free, being rebuilt on every entry. Direction comes off
    `bar.previous-index`, so the host keeps no tab state and needs no mount seed.
  - **My Library gates its direction on `morphing` rather than dropping it**, being the one page
    with a *third* animation — its own band's height, which moves `body.y` by the whole distance
    between the two floors every frame. All nine branches carry `enabled: root.body-anim-armed`
    and `slide: !band.morphing`, and none reads `Nav.pending-enter-from`; the five **tab** bodies
    take `enter-from: band.tab-enter-from` (the bar's own left/right, forwarded off the band as on
    `MosaicTabHero`) and the four **details** keep `below`, a drill not being a move along the bar.
    So a tab pick with the band still slides like the sibling pages, and anything the band morphs
    through is a cross-fade with both offsets zeroed — which is what makes `slide` the load-bearing
    line rather than the fixed axis it replaced. **`morphing` is
    `hero-t != (detail-open ? 1.0 : 0.0)`** — deliberately not `hero-t > 0`, which is `false` on a
    drill-in's first frame, the one frame the answer must be `true`; same-axis is not enough to
    skip it, the body's 32 px being made of a curve that disagrees with the morph's, so the sum
    runs the wrong way before it turns. **`body-anim-armed`** is written from `changed detail-open`
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
    needs **no `covers-generation` machinery**, the entering tab's own fetch prewarming before it
    writes rows. A `0` default on either property silently deactivates every section.
  - **What the five cannot answer is the page's own leave** — only the *mounted* tab's fires — and
    **the answer is not a sixth mount**, which is the obvious shape and compiles: a gate fires on
    transitions of *its own* predicate, that predicate is already false while Now Playing covers
    the band, and `sidebar.slint` clears `now-playing-open` and writes the new index in **one**
    handler, so the leave that matters most goes false → false. `page-active-changed` rides
    `changed watched-nav-idx` instead, which also makes "covered" and "left" different events.
  - **A mirror fires on every nav change where a gate fired only on its own predicate, so the hook
    owes a latch** — the half a later edit reads as redundant. A `changed` handler cannot ask which
    index it moved *from*, so unlatched the teardown also runs on Search → Browse and discards what
    `seed_detail_from_settings` wrote at boot precisely *because* the page was hidden. The latch is
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
  ordering is why the Rust side needs the non-hopping `apply_*_now` twins. A host wanting the old
  index reaches for `previous-index`, never a mirror seeded at mount.

- **Every brush it paints with is a defaulted `in property`**, which is what lets it sit on a hero.
  Settings takes the defaults; the two bands hand over `HeroBackdrop` tiers for **all four** —
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
  which keeps each from drifting and makes the row's minimum a sum the page can check.
  **`tip-x`/`tip-y` carry the bar's own `absolute-position` offset inside the row**, the half a
  host cannot supply — dropped, the pill lands a whole `lead-w` left of the tab it names, on the
  one mount with a leading slot.

- **The Rust half is one function: `clamp_tab(tab, tab_count)`.** All four hosts clamp on *read*
  against the Slint-declared `tab-count` — the bar can only produce a valid index, but a file left
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
lose. **Two things stay at each host**: the per-tab `ActionPill` rows (`@children`, placed after the
trailing spacer, which is also the slack `HERO_MAX_ROWS` is measured against) and the tooltip frame.

- **The height is a `min`/`preferred`/`max-height` split with `clip: true`**, never a bound
  `height` — the animated-root-dimension pitfall one axis over. **`hero-t` is *written* from
  `changed detail-open` and only seeded by its binding**, so a page entered with a detail already
  open lands at hero height instead of growing into it.

- **What may hang off an `if` is decided by change trackers, not taste.** Everything tracker-free
  rides one `hero-shown: detail-open || hero-t > 0`. The chip strip **cannot** join them — it
  carries a `changed` tracker over a layout property the morph re-dirties every frame, so dropping
  its branch panics; it stays mounted and fades by brush alpha.

- **Nothing carrying content answers `opacity`** — the layer is sized to child *geometry* and a
  text run's ink leaves its line box. Fade through the brush instead, via **`transparentize`, which
  multiplies alpha — never `with-alpha`, which sets it**; where the element has brushes feeding its
  own `animate` blocks, make it satisfy `need_layer`'s second bail rather than folding the fade in.
  Full argument in `slint-pitfalls.md`.

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

- **Build only the mounted tab's rows.** Sub-views are mutually exclusive `if`s, so anything
  prepared for an unmounted tab reaches nothing; the write side then drops a prepared result whose
  tab moved. Content hashes still come off the **source** entities, so one walk answers the
  signature for the tab it didn't build.

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

- **A tab *pick* rewinds too.** A pick runs a synchronous apply against whatever cache is there,
  which a leave or skipped tick can have emptied, so it would write `0` over the sentinel and
  assert an empty library for the length of the fetch already on its way. Three sites do it,
  excluding Songs (whose model survives the leave) and the four details (which write no grid
  count).

- **Counts are written *above* the signature guard.** A pick stamps a signature against the cache
  it just walked, so a fetch returning identical content lands on the guard and a count written
  past it never arrives — stranding the Shuffle pill and sort row as well as the empty state. Safe
  because when the guard fires the model already holds exactly the rows the count describes, and
  `Property::set` is value-compared.

- **A gated fetch owes three things**, each a bug on the way in: the fetching branch **consumes**
  its dirty flag (seeded `true`, else a boot onto that tab fetches twice); `release_section_state`
  **re-arms** it beside the cache wipe rather than leaving it to the leave's `mark_dirty` two files
  away; and the fetch re-arms on either way of storing nothing (failed query, or a leave landing
  mid-flight), the pick having consumed the flag *before* spawning.

- **Section guards sit *after* the slow part** — before it the leave hasn't happened yet, so the
  two fetchers that `.await` a cover prewarm ask twice. Every store `release_section_state` wipes
  goes under the gate.

- **A tab pick can't await a prewarm the way a fetch does**, `TabBar` writing `selected-index`
  before it emits `selected`. Rows go in through the **non-hopping** `apply_*_now` twin —
  `invoke_from_event_loop` posts even from the UI thread, and a redraw winning that race paints a
  bare panel or a `TrackList` of headers over an emptied model.

- **Covers ride the `covers-generation` gate**, and the bump comes off `should_announce_warm`,
  never off the write — see Covers.

- **Signature skips** are keyed on the tab *and* the column count beside the mounted tab's
  contents. Hashing both tabs together, or dropping either of the first two, silently skips exactly
  the apply that had to run.

- **A leave owes `mark_dirty()` for exactly what it hands back** — a tier, a model, a count.
  Tracks' leave releases nothing, so it owes none.

- Shared helpers: `ui::tab_bar::{clamp_tab, grid_signature, should_announce_warm, UNFETCHED_COUNT}`,
  `ui::grid_rows::{chunk_entity_rows, write_grid}`, `ui::track_list_cache`,
  `ui::mosaic_hero::{compose_off_thread, MosaicGuard}`.

### Per-page deltas

Each page's own tree is the reference for how it fills; what follows is only the decisions a later
edit would otherwise reverse.

- **Favorites** (three tabs) — `refresh_hero` stays **ungated**, answering the count, running time
  and collage the band states on all three tabs, which is why Songs owes no count rewind. Both sorts
  resolve **in memory**, the fetch having lost its sort parameters entirely. **The artist sort
  applies to the cached `Vec`, not the filtered copy** — `first_screenful_paths` picks prewarm
  targets off that cache, so sorting downstream warms the covers of whichever artists SQL returned
  first while the grid paints a different prefix. **`swap_tab_covers` prewarms
  `GRID_PREWARM_AHEAD`, not the tier capacity**, the grids being uncapped, so warming everything
  evicts its own work.

- **Recently Played** (two tabs) — **neither has a sort, and that is a decision rather than an
  omission**: the order **is** the page, so the synthetic-field cycle above is deliberately *not*
  what it took, and there is no sort state at all (nothing on the global, no `ViewSort`, no
  `view_sort` key). `sortable: false` runs `TrackList` → `TrackListHeader` → `HeaderCell` and
  defaults `true`, so the other eight mounts opt in by omission — **the middle link is the one
  worth pinning**, since dropping the forward leaves the mount still reading `sortable: false`
  while the page sorts again. **The band states the recency set, not the mounted tab's**, a
  play-count ranking under this banner naming the wrong page. Its filter walk is **the one apply
  path that may not run on the UI thread**, and what deferring costs is ordering — the signature
  check reads a stale set as a change rather than as staleness, so `filter_generation` is checked
  twice, on the worker and again on the UI thread.

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

- **The page is tabbed** — 5 tabs over the same 15 section cards. `settings-view.slint` is page
  chrome only, `settings-tabs.slint` the router, `views/settings/pages/*.slint` the five pages,
  each owning its section list *and* an aggregate `has-matches`. **Search escapes the tabs**: a
  non-empty query mounts all five pages at once, which is how the cross-tab flat list comes back
  with no extra filtering logic, and why both modes mount the same five pages — the card-to-tab
  mapping is stated once.

- **The search box takes two properties** — `search-input` for the `SearchBar`, `search-query` for
  the sections, joined by a `FilterThrottle`. Mounting all five pages is what makes a keystroke
  expensive here with no model in sight. Both are cleared together on nav-away and on a tab pick.
  `on_matches` memoizes the fold against the raw needle, this being the one `row_match` caller that
  can't hold a folded shadow — it is invoked per *field*, not per pass.

- **A new tab** is one page file + a `tab-*` index + two symmetric router lines + one entry in each
  of the bar's two inline arrays + a `tab-name:` on every section that page mounts, with
  `tab-count` the sole definition of how many there are. The bar's `labels` must stay an inline
  `[@tr("…"), …]` literal in `tab-*` order.

- **The tab's own name is part of each card's search term** — search escapes the tabs, so a tab
  name is what a user types, and "Interface"/"Services" appear in no card's text otherwise. It
  reaches `row-visible` as a **term of its own**, not a prefix spliced onto each title: the join
  lives in the global instead of twelve copies, and no card can match a substring spanning the
  seam. A mount that forgets `tab-name:` still matches its own title, so the page looks right and
  only the tab-name query comes up empty.

- **A row's label and description are declared once, as properties the visibility term and the row
  both read.** The section's local `row-visible(label, desc)` is fed `label + " " + desc`, so a pair
  spelled at both sites lets a reworded description stop its own row matching — invisible until
  someone types a word from it. The other half is the one to check on a **cluster**: a toggle whose
  sub-rows hang off its own visibility owes every sub-row's pair as a term (`playback-section`'s
  crossfade `||` chain, `discord-section`'s concatenated haystack), or a sub-row is reachable only
  by scrolling to it. Untranslated brands and SPDX ids take a property too — the drift hazard is
  the double spelling, not the `@tr`. Nothing enforces any of this, and a fresh section is exactly
  where it comes back.

- **Anything that has to fit a width reads `SettingsPage`, and never measures.**
  `settings-view.slint` publishes `page-w` (the panel width it already mirrors — a live
  `root.width` read feeding a child's size re-enters layout) and `body-cols`; the global derives
  `body-w`/`card-w`/`row-content-w`. Every card spans a full body column, so those are exact, and a
  control four levels down sizes itself with no width mirror, no mount `Timer` and no `parent`
  reach-around. **`row-content-w` is the one to want** — the width a `SettingRowStacked` leaves its
  content block. Override at a call site that isn't a full-width card row; don't recompute the
  number.

- **`card-cap` (800 px) is one number both layouts obey**: a card grows to the cap, stops, and
  takes margins from there, and a second column appears exactly when two *capped* cards fit beside
  it — so a card's width only ever grows and the flip resizes nothing. A threshold spelled
  independently of the cap is the failure to avoid: set below it, the column divides before the cap
  is reached, and what you see is a card that grows without limit and then halves, the cap still in
  the source and simply unreachable. Latched with a 24 px band, and **never while searching**:
  search mounts all five pages and hides the non-matching cards, and a hidden card still claims its
  grid cell.

- **A page hands the `GridLayout` one cell per *column*, not per card**, placed through
  `grid-row(i)`/`grid-col(i)`. That indirection is the whole reason the body reads as masonry:
  cards as cells are row-aligned, and a grid row is as tall as its taller cell, so a short card left
  a hole down to the next row. Columns are **contiguous halves** of the card list, so stacked they
  read in source order. Every card is still **mounted exactly once** — a page reads `has-matches`
  back off its section instances, and an element inside an `if` can't be read from outside it, so a
  one-branch-per-column-count arrangement would break the no-results placeholder.

- **Nothing has a width floor any more**: the content column's `min-width` is gone with the
  horizontal scrollbar that existed to pan it, both row components' labels take `wrap: word-wrap`
  (a no-wrap `Text` reports its full width as its layout *minimum*), and both strips wrap. Don't
  reintroduce the bar — a settings page that needs one is a page with a row that has stopped being
  able to wrap.

- **The wrapping strips wrap through Rust, because Slint can't build a nested array.**
  `chunk-indices(count, per-row) -> [[int]]` splits `0..count` into index groups and the strip
  iterates two real arrays, sidestepping both traps the predecessor was built around: Slint rejects
  an inline `for … : if …`, and a `Rectangle` wrapper around a filtered-out item still claims its
  parent layout's spacing. `wrap-per-row` and `wrap-height` sit beside it. **How many fit is
  measured, not estimated** — `ChipGroup` mounts a hidden ruler of real `Chip`s, `TabBar`'s idiom,
  and **`min-width: 0px` on the root is load-bearing**, stopping that ruler leaking a floor into
  the card.
