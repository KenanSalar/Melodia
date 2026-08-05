# My Library — the five library views become one tabbed page

Working doc. Keep the phase markers current; delete this file when the feature ships.

| phase | status |
|---|---|
| 0 — Prep | ☑ done |
| 1 — `MyLibrary` global, nav plumbing, mount sheet | ☑ done |
| 2 — `LibraryTabBand` | ☑ done |
| 3 — The five tab bodies and the mount sheet | ☑ done |
| 4 — The four details under the band | ☑ done |
| 5 — Cleanup, i18n, docs | ☐ not started |

## Context

Favorites (nav 2) and Recently Played (nav 8) were converted to tabbed pages under a
shared pinned band (`components/hero/mosaic-tab-hero.slint`, commits `de276e1` /
`b84b21b`). Tracks, Albums, Artists, Genres and Playlists are still five sidebar
entries, five page headers, five search boxes and — with their detail views — nine
router branches. The five headers are near-verbatim copies of each other; four of them
carry hand-copied grid geometry and hand-copied empty states that `GridGeometry` /
`GridEmptyState` already replace.

This folds them into one **My Library** page: five tabs under one band, one search box
whose meaning follows the mounted tab, the count promoted to the band's second row, and
the per-tab pill rows on the right of it. Drilling into a detail keeps the user on the
same page — the band **morphs** into that entity's hero banner (blur backdrop, artwork
tile, title, chips) with the tabs still in it and the back button to their left.

Decided up front:

| question | answer |
|---|---|
| nav slot | index **3**, sidebar order *under* Recently Played; 4–7 retired |
| idle band | **flat surface pane** — no blur, no mosaic |
| filter on tab switch | **cleared**, both sides (the Favorites rule) |
| tabs | Songs · Albums · Artists · Genres · Playlists |
| Tracks tab | renamed **Songs**, icon `music_note` (Favorites' Songs tab); the sidebar's old `library_music` becomes My Library's icon |
| detail presentation | the band **morphs**; `DetailHeader` is retired |
| tab-switch data | tab leave **is** a section leave — teardown + refetch, no retained rows |

## The two band states

```
IDLE (no detail open)                          HERO (detail open)
┌──────────────────────────────────────┐       ┌──────────────────────────────────────┐
│ [Songs][Albums][Artists]…   [Search] │       │ (←) [Songs][Albums][Artists]…[Search]│
│                                      │       │ ┌────┐ Ravedeath, 1972               │
│  1,248 songs        ( Shuffle │ ⋯ )  │       │ │ art│ Tim Hecker                    │
└──────────────────────────────────────┘       │ └────┘ ⟨chips⟩      ( Shuffle │ ⋯ )  │
   flat Theme pane                             └──────────────────────────────────────┘
   2·pad-lg + 48 + pad-md + meta-row-h            2·pad-lg + 48 + pad-md + Theme.hero-artwork
   = 32 + 48 + 12 + 40 ≈ 132 px                   = 32 + 48 + 12 + 140 = 232 px
                                                  (= MosaicTabHero's `hero-height`, verbatim)
```

One animated float `hero-t: 0 → 1` drives everything: band height, the backdrop reveal,
the back-button slot's width, the tab bar's brushes, and the pill row's position.

## Architectural decisions (the load-bearing ones)

**1. Nothing about the five views' data layer changes.** `Tracks` / `Albums` /
`AlbumDetail` / `Artists` / … keep their globals, models, `*Ui` handles, `fetch_grid` /
`open_*` / `refresh_detail`, cover tiers, `view_id` keys, `view_sort` / `view_columns` /
`last_detail_ids` entries. This is a chrome-and-routing change. The ten Rust modules
(`src/ui/{tracks,albums,artists,genres,playlists}/` + their `callbacks/` twins, ~9 kLOC)
are touched only where they read a header property that moved.

That extends to what the page does *not* add to `MyLibrary`. **There is no `detail-open`
on the global** — a global's property is bound in the global's own file, so deriving it
from the four `*Detail.*-id`s would make `my-library.slint` the second globals file to
import siblings (`globals/dialog.slint` is the only one today, and CLAUDE.md keeps that
edge set deliberately shallow). It is a private `property <bool>` in
`views/my-library-view.slint`, which already imports all four detail globals, handed to
the band as an `in property`. Rust reads the four ids directly, exactly as
`nav_history::current_detail_id_for_section` already does. Derived, not written, and no
new global edge.

**2. A tab switch *is* a section leave + a section enter.** Today five
`SectionActiveGate` mounts (indices 3–7) drive `<Global>.section-active-changed(bool)`,
and each view's lifecycle releases its cover tiers, empties its models and
`mark_dirty()`s on the way out, re-fetching on the way back. Extend
`components/section-active-gate.slint` with an optional sub-predicate:

```slint
in property <int> tab-index: -1;      // -1 = section has no tabs
in property <int> current-tab: -1;
private property <bool> watched:
    Nav.selected-index == root.index && !Nav.now-playing-open && !root.mini-active
    && (root.tab-index < 0 || root.tab-index == root.current-tab);
```

and mount five of them at `index: 3` with `tab-index: MyLibrary.tab-<x>;
current-tab: MyLibrary.tab-idx`. **Every existing lifecycle hook then works unchanged** —
tab-leave releases covers exactly as sidebar-leave does today, and the entering tab's
`take_dirty()` → `fetch_grid` path already prewarms its own first screenful (into
`spawn_blocking`, capped by `grid_prewarm::cover_cap_for_window`) *before* `rebuild_grid`.
That is why this page needs **no** `covers-generation` machinery (Favorites needed it
because a tab pick had no fetch to hide behind; here it does), and no `swap_tab_covers`,
no `prewarm_tab_covers`, no per-tab warm announcement.

The rule from `ui-patterns.md` — "a new section adds one mount, not a tenth copy of the
predicate" — is preserved: the predicate stays in the gate.

Two consequences to hold onto. **It tightens visibility gating for free**: Tracks'
`install_library_changed_refresher` and all four `library_changed` subscribers already
bail on `!section_active()`, and tab-scoping means a background bump no longer rebuilds
four hidden grids while one tab is up. And **the cost of a tab pick is a full re-query +
prewarm** — identical to today's sidebar-switch cost, but tabs invite more switching than
a sidebar does. That is the accepted trade; the only escape hatch would be to keep the
departing tab's models alive so the leave needn't mark dirty, which is the Favorites shape
and costs five tabs' row `Vec`s resident at once. Not taken.

**3. One search box, dispatched in Rust — and the filter reaches each view by
*binding*, not by a write.** `MyLibrary.filter` is the only text; the band fires
`MyLibrary.filter-changed(string)` through the shared `FilterThrottle` (130 ms, the
list-view shape, not the Settings shape).

The dispatch cannot be a single call, because the two halves of the page answer different
contracts. The five grid/list globals fire `apply-filter(text)` and Rust **ignores the
argument** — `callbacks/albums/grid.rs` is `on_apply_filter(move |_text| … rebuild_grid(…))`
and `compute_indices` reads `<Global>.get_filter()` itself, memoized against
`GridIndexCache { filter, sort_field, sort_dir, indices }`. The four detail globals fire
`filter-changed(text)` and Rust **uses** the argument, folding it into the
`Mutex<Needle>` on `*DetailState::filter`.

**This decision originally said the sheet binds each view's filter one-way —
`Tracks.filter: MyLibrary.filter;` and the four grid twins — and that is not spellable.**
A `.slint` binding belongs to the element or global in whose scope it is written, so an
element cannot declare one on another global's property; there are zero such bindings
anywhere in the tree, which is what made the omission easy to miss on paper. The only
binding form that would work is one declared inside `globals/tracks.slint` itself, which
would make five globals import `MyLibrary` — the edge decision 1 exists to avoid, in the
other direction.

So the hand-off from the page's one box to the five views is a **write**, and
`src/ui/my_library/filter.rs` does it: each grid/list arm sets the target global's
`filter` and then invokes its existing `apply-filter`, since the rebuild reads that
property back rather than taking the argument. The four detail arms are unchanged — they
pass the needle by argument, which is what decision 3's two-contract split was already
describing. Nine `if` branches in one Rust function beats nine branches in a Slint
callback body, and Rust already holds both facts.

**What the correction retires is the constraint that rode along.** "Once `<Global>.filter`
carries a binding, Rust must never write it" was true of a shape that cannot exist; with
no binding to orphan, the write is simply the mechanism. The `<=>` on each per-view
`SearchBar` survives external writes (`link_two_way` does), so while those boxes are still
up they mirror the shared one rather than fighting it — which is what makes Phase 2's
duplicate-box state work at all. The four `*Detail.filter`s stay as they are: Rust clears
each to `""` on a fresh detail open (`albums/detail.rs`, `artists/detail.rs`,
`genres/detail.rs`, `playlists/detail.rs`) and `callbacks/artists/detail.rs` clears it
again on close, and nothing about that has to change.

The nine `blur-search-tick` properties go too, with one `MyLibrary.blur-search-tick` in
their place; their *writers* are rewired, not deleted (see Phase 4). **All nine writers are
Slint-side — there are zero Rust writers** — so that consolidation costs no Rust at all.

**4. The pill row is `@children` in an absolutely-positioned slot.** Slint allows one
`@children`, and the pills need two homes — right of the count in idle, bottom-left under
the chips in hero. So the band wraps `@children` in a `HorizontalLayout` positioned by
animated `x`/`y` interpolated between the two anchors; the morph slides the pills across.
A layout child, not a bare `Rectangle` — the pill rows are `if`-conditional and a
`Rectangle` reports 0×0 over conditional children (`slint-pitfalls.md`). It is a direct
child of the band's root, not of the header `VerticalLayout`, because a layout overrides a
child's `x`/`y`; the idle band therefore reserves `meta-row-h` for it rather than measuring
it.
*Fallback if the floating slot proves fragile:* pin the pills bottom-left in both modes
and accept a taller idle band.

**5. The tab bar's four brushes animate between token sets.** Idle sits on
`Theme.mantle`, hero on the solved backdrop, and `hero_backdrop::reset` publishes an
accent-seeded *dark* floor solve — so `HeroBackdrop.on-backdrop` is near-white and wrong
on a light theme's idle pane. The band declares four `property <brush>` ternaries on
`detail-open` with `animate { duration: Theme.dur-spatial; }` and hands those to `TabBar`,
whose brushes are already all inputs (`ui::tab_bar::tests::every_painted_brush_is_an_input`).

**6. The band's height uses the min/preferred/max split, not a bound `height`.** Slint
reports a component root's bound dimension as both `min` and `max`, so an animated
`height` would ease the *window's* own minimum height — the `TabBar` bug, one axis over.
`min-height: compact-h` / `preferred-height: <animated>` / `max-height: hero-h`, plus
`clip: true` on the root (the split lets the element be drawn shorter than it asked for,
and the hero content would otherwise paint past the compact band into the body).
`tab-bar.slint`'s width split is the precedent, `clip` and all.

*Cost:* an animated `preferred-height` re-runs the page's layout every frame for
`dur-spatial`. Bounded — the bodies are virtualized `ListView`s, so the relayout touches
visible rows only. Measure once at Phase 4, which is where the morph first runs; if it
janks, drop to `dur-med` or snap the height and animate only the contents. Do not slow the
body to hide it.

**7. `hero-t` is written, not bound.** Seed it with the binding (`detail-open ? 1 : 0`)
so mount lands in `NotAnimating`, and own it from `changed detail-open => { self.hero-t = … }`.
An animated *binding* restarts on dependency dirtiness, not on value change — which is why
`tab-bar.slint` writes `compact-t` from `changed compact` and has a test pinning that.

**8. Counts adopt the `-1` sentinel.** `total-count` on all five globals now gates a
count line in *shared* chrome that swaps per tab, so a value outliving the model it
numbers is visible during a switch. Default them to `ui::tab_bar::UNFETCHED_COUNT` and
rewind on section-leave beside the model clears — the `b84b21b` precedent, which
`ui-patterns.md` already flags as "worth doing when one is next touched". The count line
renders `""` below zero; `-1` matches neither `== 0` nor `> 0`, so every empty-state gate
keeps its expression, and no `max(…, 0)` clamp is needed (this band has no mosaic tile).

There is no bools problem to dodge here — `ViewStateData` carries exactly one bool
(`artist_albums_collapsed`), well under `clippy::struct_excessive_bools`' cap of 3 (which
`clippy.toml` doesn't override), since the two `favorites_*_collapsed` flags were already
removed. The rule stated at
`services/view_state.rs`'s doc still binds regardless: **a new persisted view flag is an
int / string / map, never a bool.** `my_library_tab` is an `i32` for that reason, not
because the cap is close.

**9. A drill's origin is now a *pair*, and `origin-nav-index` alone stops
discriminating.** Five views collapsing onto index 3 means an intra-page cross-tab jump
(Songs → *Go to Artist*) stamps `origin-nav-index = 3` and then navigates to 3 — so
`cross_tab_nav`'s `nav.get_selected_index() == origin` guard is trivially true, and the
back path's `nav.set_selected_index(origin)` is a no-op that restores no tab.

Add **`origin-tab: int`** (default `-1`) to `AlbumDetail` / `ArtistDetail` / `GenreDetail`,
written beside `origin-nav-index` in the same synchronous stretch. It has to live per
detail global, not once on `MyLibrary`: **two details can be open at once** — Songs → Go
to Artist → click an album leaves `ArtistDetail.artist-id >= 0` *and*
`AlbumDetail.album-id >= 0` — so a single slot is clobbered by the second drill and the
back chain returns to the wrong tab. `PlaylistDetail` needs neither; it has no
`origin-nav-index` today and nothing navigates to a playlist cross-view.

The guard becomes `origin_nav == get_selected_index() && (origin_nav != NAV_MY_LIBRARY ||
tab_idx == origin_tab)`, and the back path restores nav *and*, when `origin_nav == 3`, the
tab. `origin-nav-index` stays load-bearing regardless: `cross_tab_nav` wires eight globals
and half those origins (Browse, Favorites, Recently Played, Search) sit outside My Library.

## Phases

Each phase ends compiling and passing `cargo clippy --all-targets --locked -- -D warnings`
and `cargo test --locked`. Phase 0 has exactly one visible change, recorded below; **Phase 1
is where the page appears**, since retiring nav 4–7 is not something that can be done
invisibly — see its own section for what "Phases 1–2 leave the running app unchanged" was
trying to say and why it couldn't hold.

---

### Phase 0 — Prep ☑

Pure de-duplication, done first so the tab bodies inherit it instead of the diff carrying
both shapes.

**What the phase actually landed**, including the four things the plan above got wrong or
left open:

- **The per-view `lifecycle.rs` files are under `src/ui/callbacks/<view>/`, not
  `src/ui/<view>/`** — the latter holds the fetch/state half (`mod.rs`, `grid.rs`,
  `detail.rs`, …). Phases 3 and 4 name these files; read the path from here.
- **The sentinel forces a guard the curated pages never needed, and this is the one thing
  from Phase 0 that Phase 3 must carry.** All five `total-count`s are interpolated into a
  gettext plural, so a bare `-1` renders **"-1 albums"** — the curated counts only ever
  gated `== 0` / `> 0`, which the sentinel satisfies by missing both. Every count line is
  now `<Global>.total-count >= 0 ? @tr(…) : ""`, a ternary rather than an `if` so the
  `Text` keeps its slot in the `spacing: 0` title column and nothing jumps. **The band's
  `count-text` ternary inherits this**, and it is not optional there either.
- **Tracks took the full leave arm** (`mark_dirty()` + the rewind), so "a tab switch *is* a
  section leave" already holds for Songs and decision 2 needs no special case for it. A
  rewind alone would have been a bug: nothing else re-fetches that list, so the header
  would have stated `""` over rows still on screen, permanently. **This is the phase's one
  behaviour change** — re-entering Tracks now re-queries where it used to be instant,
  which is what the other four already did. Its *model* is still not cleared on leave; the
  stale rows are what the list paints during the deferred re-fetch, and whether Songs also
  empties is Phase 3's call.
- **`IconButton` needs nothing** — `idle-bg` (`components/icon-button.slint:18`) and
  `idle-fg` (`:25`) are already defaulted `in` properties. Phase 2 can mount the
  chip-coloured back button as written; don't re-check.
- **`body:` as a property name does not collide with a `body :=` element id.** All four
  grid views mount `GridEmptyState { body: @tr(…); }` *inside* their `body := Rectangle`
  and compile clean, and `GridGeometry { avail-width: body.width; }` forward-references it
  from above. Phase 3's mount sheet reuses both shapes.

Two things went beyond the written scope, both inside the blast radius:

- The four `lifecycle.rs` leave arms hand-rolled `downcast_ref::<VecModel<…>>()` +
  `set_vec(Vec::new())` where `ui::model_diff::clear_vec_model` already existed (and logs
  on a failed downcast, where the hand-rolls swallowed it). Fifteen blocks became fifteen
  lines, and `Model` / `VecModel` left all four files' imports.
- `melodia-ui/ui/components/material-icon.slint` is no longer imported by any of the four
  views — the empty state was its only consumer in each.

Docs are deliberately **not** updated yet: `.claude/rules/ui-patterns.md` (lines 31 and 45)
and root `CLAUDE.md` describe the pre-fold world and are Phase 5's, per the list there. The
two stale claims to fix then are "the four older grid views still carry hand-copied
versions" and "the four older entity grids deliberately keep the older shape".

- `views/{album,artist,genres,playlists}-view.slint`: replace the four hand-copied
  `min-card-w`/`computed-cols`/`card-w`/`card-h`/`row-h` blocks with
  `components/grid-geometry.slint`'s **`GridGeometry`**, and the four hand-copied empty
  states with **`GridEmptyState`** (`components/grid/grid-empty-state.slint` — under
  `grid/`, not beside `grid-geometry.slint`).
  **This is also a fix, not only a de-dup.** The four copies compute off `grid.width` — the
  child they are sizing — while `GridGeometry`'s contract and both mosaic pages pass the
  *container's* width (`favorites-view.slint` → `body.width`). They are numerically equal
  today because the grid is `width: 100%` of body, so the swap is safe; the point is that
  reading a child's width to size that child's own children is the shape `page-w` exists to
  avoid.
- `components/section-active-gate.slint`: add the `tab-index` / `current-tab` sub-predicate
  (above). All nine existing mounts keep the default `-1` and are unaffected.
- Adopt `UNFETCHED_COUNT` on `Tracks/Albums/Artists/Genres/Playlists.total-count`:
  declare `-1`, rewind in each leave arm beside the model clears. Extend
  `ui::tab_bar::tests`' `CURATED_PAGES`-style pin to cover the five.
  **Tracks is the odd one out twice** — its lifecycle is inline in `callbacks/tracks.rs`
  rather than a `lifecycle.rs`, and it does *no* teardown on leave today (no model clear,
  no cover release). So this adds a leave arm it has never had, and "tab leave releases
  covers exactly as sidebar leave does" means "releases nothing" for Songs. Don't expect
  symmetry there; the row tier it reads is shared and outlives every section.
- Confirm `IconButton` needs nothing: it already exposes `idle-bg` / `idle-fg`, so the
  chip-coloured back button is `idle-bg: HeroBackdrop.chip-fill; idle-fg: HeroBackdrop.chrome;`.

### Phase 1 — `MyLibrary` global, nav plumbing, mount sheet ☑

**What the phase actually landed**, and the five places the plan below was wrong or made a
choice it hadn't seen yet:

- **The page is visible from this phase on, and "Phases 1–2 leave the running app
  unchanged" could never have held.** Retiring 4–7 is what forces it: the moment five views
  share nav index 3, every seed, guard and map that identifies one by its index stops
  discriminating, and none of that can be staged. So the sheet mounts the **existing** view
  bodies rather than being an empty stub — the five view files and the four detail views are
  moved out of `app-window.slint`'s router into `views/my-library-view.slint` unchanged,
  under a temporary `TabBar`. Every page still works; each just carries its own header and
  search box until Phase 3 strips them, so the page reads as five stacked headers under a
  bare bar. The alternative — an empty placeholder — would have left the whole library
  unreachable for two phases and left this phase's real risk (the tab-scoped gates, the
  origin pair, the third replay arm) untested until Phase 3.
- **There is no `MyLibraryUi` handle, and the boot-ordering argument survives without
  one.** The plan gave it an `AtomicU8` tab shadow and split seeding into
  `seed_tab_property` / `seed_tab_shadow` to match Favorites. Nothing needs it: the five
  views each keep their own `section_active` shadow, which the per-tab gate now makes mean
  "My Library is up *and* my tab is mounted", and every other reader of the tab
  (`format_view`, `nav_history`, `cross_tab_nav`, the filter dispatcher) already holds an
  `&AppWindow`. So it is one stateless `ui::my_library::seed_tab(app, persisted)` in the
  `ui::settings_page::seed_tab` shape, called at step 5a — and the load-bearing half, that
  the seed precedes `wire_all`, is unchanged and pinned.
- **The five section seeds go through one predicate, not five spellings.**
  `ui::my_library::tab_is_mounted(ui, MyLibraryTab::X)` replaces the five
  `get_selected_index() == <literal>` reads, four of which were bare magic numbers. Pinned
  by `every_section_seed_reads_the_mounted_tab`; the mutation to check is dropping the tab
  half, which leaves four views wrongly active for a session.
- **A tab move that isn't a user's pick needs its own callback.** `MyLibrary.tab-changed`
  clears the filter, which is right for a tab bar click and wrong for a cross-tab drill or a
  Mouse-4/5 walk — so `persist-tab-idx(int)` sits beside it, the `Nav.persist-selected-index`
  shape one level down, and `cross_tab_nav` / `nav_history` reach for that.
- **`artists/cross_tab.rs`'s `open-album` was a hand-rolled copy of
  `cross_tab_nav::open_album_cross_tab` with its origin hardcoded to the Artists tab**, and
  it is now one call to the shared helper with `Origin::read`. That is where the origin pair
  and the mid-fetch guard get to exist once rather than twice — and re-inlining it is what
  `every_detail_records_the_tab_it_was_opened_from` catches. `Origin` is the small
  `{ nav, tab }` type that carries the pair; `Origin::section(n)` is the tabless form
  Favorites and Search hand over.
- **The `@tr("My Library")` msgid landed here, not in Phase 5.** It has to:
  `ui::locale::tests::every_translated_literal_has_a_msgid_in_every_catalogue` is part of
  the phase gate, so the six catalogues move with the string that needs them.

Two things the plan asked for that were dropped as inert, both re-addable in three lines:

- **No page-level `SectionActiveGate` and no `MyLibrary.section-active-changed`.** The five
  per-tab gates already fire the mounted tab's leave when the page leaves, and the other
  four are inactive by then, so a sixth gate would have nothing to say and no Rust handler
  to say it to.
- **No `detail-open` property on the sheet yet.** Decision 1's argument for where it lives
  stands; it arrives in Phase 2 with the band that reads it.

**What did *not* change**, and is worth stating because it is the whole premise: the five
views' data layer. Same globals, models, `*Ui` handles, cover tiers, `view_id` keys,
`view_sort` / `view_columns` / `last_detail_ids` entries. `shutdown.rs` prunes nothing.

- **`melodia-ui/ui/globals/my-library.slint`** (the 20th globals file; add its import +
  re-export to `app-window.slint`'s flat `export { }` block):
  `tab-songs/-albums/-artists/-genres/-playlists` (0–4), **`tab-count: 5`** (the sole
  definition), `in-out tab-idx`, `tab-changed(int)`, `in-out filter`, `blur-search-tick`,
  `filter-changed(string)`, `section-active-changed(bool)`, `back()`. **No `detail-open`**
  — decision 1. The file imports no sibling global.
- **`src/ui/my_library/{mod,tabs,filter}.rs`** — `MyLibraryTab` enum with
  `as_code`/`from_code`, `tab_from_index(&MyLibrary, i32)` (comparing against the
  global's own constants, never a Rust literal), an `AtomicU8` active-tab shadow, and the
  filter dispatcher. `MyLibraryUi` holds only the shadow — no caches, no `SectionState`
  (the five sections keep their own).
  Tab seeding **splits in two**, and the split is the whole point (see the boot bullet
  below): `seed_tab_property(&AppWindow, i32)` clamps through `ui::tab_bar::clamp_tab` and
  writes the Slint property; `seed_tab_shadow(&AppWindow, &MyLibraryUi)` reads it back into
  the handle.
- **`src/ui/callbacks/my_library/mod.rs`** — `wire_my_library`: `on_tab_changed`
  (shadow → clear `MyLibrary.filter`, which reaches all five views through the bindings of
  decision 3 → persist → the `CompositeScroll.reset()` + focus regrab below),
  `on_filter_changed` → dispatcher, `on_back` → the open detail's `close-detail()`.
  Local `const NAV_MY_LIBRARY: i32 = 3;`.
- **Persistence** — `ViewStateData.my_library_tab: i32` + `library::settings::set_my_library_tab`
  (`src/library/settings/view.rs`, the `set_favorites_tab` shape) + a `view_state_tests`
  case. All five `view_sort` keys stay live, so `shutdown.rs` prunes nothing.
- **Boot ordering — `MyLibrary.tab-idx` is seeded at step 5a, before `wire_all`.** This is
  the opposite of where Favorites seeds, and the difference is load-bearing. Each of the
  five `wire_*` seeds its `section_active` shadow from `Nav.selected-index == 3 &&
  MyLibrary.tab-idx == <its tab>`; run the seed afterwards and `tab-idx` is still the
  global's declared `0` at that moment, so **Songs seeds `true` and the other four seed
  `false` regardless of the persisted tab**. The gate's `changed` edge self-corrects a
  frame later, but boot has already fired the first-enter kick — every launch would run a
  full Tracks query even when the user's tab is Artists, and the real tab's fetch would
  land late. `seed_tab_property` needs only `app` and the persisted value, so it goes
  beside the `Nav.selected-index` hydration; `seed_tab_shadow`, which needs the handle,
  stays at 5c2h with the detail seeds. Extend `boot/tests/ui_setup_tests.rs`'s existing
  hydrate-before-`wire_all` pin to cover it.
- **Nav** — `layout/sidebar.slint`: the five items become one
  `SidebarItem { index: 3; label: @tr("My Library"); icon: "library_music"; }`. It is
  already in the right place — Tracks (index 3) sits directly under Recently Played today,
  so this is a relabel of that row plus four deletions. `globals/nav.slint`'s index comment
  updated (4–7 retired).
- **The retired indices leave three maps behind, all of which currently name 4–7.**
  - `app-window.slint`'s `view-title(idx)`: 3 becomes `@tr("My Library")`; 4–7 fall through
    to the untranslated `"Melodia"` return. The `PlaceholderView` branch's ten `!=` clauses
    shrink to six.
  - `src/tasks/rss_sampler.rs::format_view` hard-codes the same 0–9 map **and restates it
    in its doc comment**. Index 3 becomes `MyLibrary(<tab>)` consulting the per-tab detail
    id; arms 4–7 go. Left alone, `MELODIA_RSS_SAMPLE` reports "Tracks" for the whole page
    and the diagnostic stops distinguishing the thing it exists to distinguish.
  - `boot::ui_setup`'s `(0..=9)` read-side check must additionally fold a persisted
    `4..=7` down to `3`. The app is publicly released and `views.json` in the wild holds
    those values; without the fold they land on `PlaceholderView`. The write-side
    `clamp(0, 9)` in `library/settings/view.rs` can stay — nothing writes 4–7 after this.
- **Cross-tab nav** — `callbacks/cross_tab_nav.rs` (which wires **eight** globals: `Tracks`,
  `Browse`, `Favorites`, `PlaylistDetail`, `AlbumDetail`, `ArtistDetail`, `GenreDetail`,
  `RecentlyPlayed`, plus `Search.go-to-genre`) and `callbacks/artists/cross_tab.rs`:
  `NAV_ALBUMS/ARTISTS/GENRES` all become `NAV_MY_LIBRARY`, and each hand-off now also
  writes `MyLibrary.tab-idx` (+ persists it) inside the same `upgrade_in_event_loop`
  closure that flips `Nav.selected-index`. The guard becomes the pair of decision 9, and
  `origin-tab` is stamped beside `origin-nav-index` on the three detail globals that carry
  one.
- **`src/ui/nav_history.rs`** — `NavEntry` gains `tab: i32` (`-1` outside section 3);
  `record_current` reads `MyLibrary.tab-idx` for section 3;
  `current_detail_id_for_section` becomes `current_detail_id_for(ui, section, tab)`.
  `NAV_ALBUMS/…/PLAYLISTS` collapse to `NAV_MY_LIBRARY` + the four tab constants.
  **Replay needs a third arm, not a widened comparison.** Same-section-different-tab is
  neither of the two branches that exist: the same-section arm only opens/closes a detail,
  and the cross-section arm flips `Nav.selected-index`. The new arm writes `tab-idx`,
  persists it, calls `nav_transition::mark`, then opens/closes the *target tab's* detail.
  `src/ui/tests/nav_history_tests.rs` needs the new field.
- **Router** — one `if !Nav.now-playing-open && Nav.selected-index == 3: ViewTransition { MyLibraryView … }`
  branch replacing the nine, which move **into** `MyLibraryView` rather than being deleted:
  keyed on `MyLibrary.tab-idx` plus each detail's own id, since boot restores a detail id
  per view and more than one can be `>= 0` at a time. The five tab-scoped
  `SectionActiveGate`s go here too. Nothing waits for Phases 3/4 — the old sidebar items
  and router branches are gone as of this phase; what Phase 3 deletes is the five *view
  files*, once the tab bodies replace them.

### Phase 2 — `LibraryTabBand` ☑

**What the phase actually landed**, and the six places the plan below was wrong or made a
choice it hadn't seen yet:

- **The band is *mounted* from this phase, not from Phase 3, and that isn't a scope
  decision — it is what makes the phase gate mean anything.** `melodia-ui/build.rs`
  compiles only `ui/app-window.slint`'s import graph, so a band written and left unmounted
  is a 300-line file that is never parsed, never type-checked, and covered by nothing but
  `include_str!` text pins; "the phase ends compiling" would be vacuously true and Phase 3
  would eat every error at once. So the sheet's temporary `TabBar` is gone and
  `band := LibraryTabBand` is in its place.
- **It mounts with `detail-open: false` as a literal.** The hero half is written,
  compiled and pinned — it simply never evaluates true, because the four detail views
  still carry their own `DetailHeader` and deriving `detail-open` now would grow a blank
  232 px hero above a real one. Phase 4 swaps the literal for the private derivation of
  decision 1. Two things follow: **the morph cannot be exercised yet**, so "measure the
  morph once, here" moved to Phase 4's manual list, and the band's tab-bar brushes sit on
  their idle (`Theme.*`) arm for the whole phase.
- **Three of Phase 3's sheet items came forward, because the band cannot be mounted
  without them**: the `count-text` ternary, the `filter-placeholder` ternary and the
  filter wiring (`filter <=> MyLibrary.filter`, the `FilterThrottle`, the root blur
  `TouchArea`). Each reuses msgids the five view headers already registered, so no
  catalogue moved. The five per-view search boxes and count lines are still up, so the
  page duplicates both for one phase — the Phase 1 "five stacked headers" trade, one step
  smaller.
- **The shared box is wired to work rather than left inert, and the write is `set_filter`,
  not a binding — see the decision-3 correction above.** One known transitional oddity:
  `on_tab_changed` clears `MyLibrary.filter` but not the five view globals', so a tab you
  filtered through *its own* box and then left keeps that filter while the shared box
  reads empty. Not a regression (nothing cleared them before either), and it goes with
  the per-view boxes in Phase 3, where
  `a_tab_pick_clears_the_filter_on_both_sides` pins it.
- **The pill slot needed no interpolation, and decision 4's two anchors collapsed to
  one.** The idle meta row's floor and the hero band's floor are the *same* line —
  `root.height - pad-lg - pill-h` in both states — so the slot is a fixed
  `alignment: end` row that simply rides the animated height. What pays for it is a
  `padding-bottom: pill-h + pad-xs` on the hero text column, reserving exactly the band
  the pill row used to occupy as a child of that column, so `HERO_MAX_ROWS`' two-row slack
  is arithmetically unchanged. No `preferred-width` self-read, no `alignment` ternary, and
  the fallback in decision 4 is unneeded.
- **The meta block is two mutually exclusive columns, not one column of ternaries.** The
  two states want different `alignment`s — a lone count line centres in its 40 px row, a
  hero block stretches so its trailing spacer pushes the chips up — and `alignment` is the
  one thing a ternary can't reach. Splitting also lets each title read a single font size
  instead of a conditional pair, which is the shape `ui::hero_chips::tests` wants in
  Phase 5.

Three smaller deviations, all recorded rather than silent:

- **`HeroChipStrip` is gated on `detail-open`** rather than relying on "publishes nothing
  in idle". `HeroChips.rows` is one global six heroes share and it outlives whichever
  filled it; this page publishes none of its own, so it would have nothing to clear if a
  stale set arrived.
- **No `out property <length> band-height`.** Nothing reads it — the band is a
  `VerticalLayout` child and sizes itself. Add it when a reader appears.
- **`meta-row-h` and the back-button slot are derived from `pill-h`, not spelled.**
  `meta-row-h = pill-h + 2·pad-xs` (40 px) and the slot is `pill-h + pad-sm` wide, so the
  compact band's height and the back button's clearance both follow the one number that
  says how tall an `ActionPill` is.

**`melodia-ui/ui/components/hero/library-tab-band.slint`.** Sibling of `MosaicTabHero`,
not a fork of it: the two share `TabBar` / `SearchBar` / `HeroBlurBackdrop` /
`HeroChipStrip` / `Tooltip` anchors but differ in the mosaic-vs-artwork tile and the whole
morph. **Port these five verbatim from `MosaicTabHero`** — each is a paid-for fix and a
copy is where one goes missing:

1. the `page-w` mirror (`changed width => { self.page-w = self.width; }`) **and** its
   1 ms mount `Timer`;
2. the seed at the row's own floor
   (`2*pad-lg + bar.compact-w + 2*pad-md + search-w-max`);
3. the `search-w` clamp budgeted against `bar.compact-w` and `search.min-w`, never
   restated literals;
4. `tab-enter-from` off `bar.previous-index` + `tab-anim-armed` starting `false`;
5. the published `tip-x/-y/-w/-h/-label/-visible` anchors (the tooltip must be declared
   after the scroll body by the host).

Because it is a sibling and not a subclass, it **re-declares the tab + search half too** —
`tab-labels`, `tab-icons`, `in-out tab-idx`, `tab-selected(int)`, `in-out filter`,
`blur-tick`, `search-edited(string)`, and `filter-placeholder` (the mounts differ: Artist
Detail's is "Filter tracks & albums…", the others "Filter <things>…"). New surface on top
of that:

```slint
in property <bool> detail-open;
in property <string> count-text;          // idle line, "" while a count is unfetched
in property <string> title;               // hero line
in property <string> subtitle;            // album artist / playlist description
in property <bool>   title-badge;         // smart-playlist auto_awesome
in property <string> artwork-path; in property <image> cover;
in property <bool>   circular-artwork; in property <string> fallback-icon;
in property <brush>  tile-bg; in property <brush> tile-icon-color;   // Genre's gradient
in property <image>  blur-a; in property <image> blur-b;
in property <bool>   use-a; in property <bool> has-blur;
callback back-clicked();
out property <length> band-height;
```

Morph mechanics, in one place:

- `hero-t` — seeded `detail-open ? 1.0 : 0.0`, **written** from `changed detail-open`,
  `animate { duration: Theme.dur-spatial; easing: cubic-bezier(0.2, 0, 0, 1); }`.
- root: `min-height: compact-h` / `preferred-height: compact-h + (hero-h - compact-h) * hero-t`
  / `max-height: hero-h`, `clip: true`, `vertical-stretch: 0`.
  `hero-h` is `MosaicTabHero`'s formula unchanged (232 px); `compact-h` is
  `2*pad-lg + tab-row-h + pad-md + meta-row-h` (≈ 132 px).
- backdrop: `HeroBlurBackdrop` always mounted (it paints only its gradient floor when
  `has-blur` is false), with a full-bleed idle pane over it whose alpha is folded into
  the brush — `background: Theme.mantle.with-alpha(1.0 - root.hero-t)` — **not**
  `opacity` on an element, which costs a layer (the `tab-bar.slint` label-fade rule).
  **Genre Detail keeps `has-blur: false` on purpose**: its backdrop is a name-hashed
  *gradient* published by `hero_backdrop::apply_gradient` and reset explicitly in
  `genres/lifecycle.rs`, because `release_detail_hero_images!` never runs for a view with
  no images. Don't "simplify" the four-image quartet into a required input.
- back-button slot: an `if root.hero-t > 0` `HorizontalLayout` before the bar, width
  `40px * hero-t`, holding
  `IconButton { icon: "arrow_back"; idle-bg: HeroBackdrop.chip-fill; idle-fg: HeroBackdrop.chrome; }`
  — the same colour approach as the Favorites chips, which is exactly `MetaChip`'s pair.
- the four `TabBar` brushes: animated ternaries on `detail-open`
  (`Theme.text` ⇄ `HeroBackdrop.on-backdrop`, `Theme.accent` ⇄ `HeroBackdrop.chrome`,
  `Theme.surface0` ⇄ `on-backdrop.with-alpha(0.12)`, `Theme.surface1` ⇄ `tile-edge`).
- the meta row: artwork column under `if hero-t > 0` (`opacity: hero-t` is acceptable
  *here* — it settles at 1.0, so no layer at rest), then the text column whose first line
  is `detail-open ? title : count-text` at `Theme.page-title-size` idle /
  `Theme.hero-title-size` hero, then `HeroChipStrip { }` (publishes nothing in idle mode,
  so it collapses on its own).
- the floating pill slot (decision 4).

**There is deliberately no "My Library" title in idle mode** — the tabs name the page and
the sidebar item carries the word, the same trade the Settings page made. The count is the
prominent line instead.

**Measuring the morph belongs to Phase 4, not here** — with `detail-open` a literal
`false` there is nothing to measure. The concern stands: the animated `preferred-height`
re-runs the page's layout every frame for `dur-spatial`, and it should be invisible
because the bodies are virtualized. Fallbacks in decision 6.

**Tests** — ☑ `src/ui/tests/library_tab_band_tests.rs`, eleven pins. Six mirror
`mosaic_tab_hero_tests.rs` (the five ported fixes plus the published tooltip anchor); the
other five are the morph's: the height min/preferred/max split and the root `clip`, that
`hero-t` is *both* seeded by its binding and written from `changed detail-open`, that the
back button takes both brushes from `HeroBackdrop`, that the idle pane folds its alpha
into the brush rather than reaching for `opacity`, and that the band spells no `@tr`
literal of its own.

The tab-bar-brush pin is the one that had to be reworded rather than ported: here each
brush is a *pair*, so it asserts a `HeroBackdrop.` arm **and** a `Theme.` arm, since
dropping either half is a bug visible in only one of the two states.

### Phase 3 — The five tab bodies and the mount sheet ☑

**What the phase actually landed**, and the six places the plan below was wrong or made a
choice it hadn't seen yet:

- **The pill rows are one component, `views/my-library/tab-pills.slint`, and the Playlists
  row inside it is *collapsed* rather than `if`-gated.** The plan has the five rows handed
  to the band as five `if`-gated `@children`, the Favorites shape. Four of them can be;
  Playlists can't, and the reason is the constraint this page keeps running into from a new
  direction. Its four action pills carry `tooltip-overlay: true`, so the tooltip is a
  top-layer frame the host declares *after* the scroll body — anchored on the pill it would
  be drawn by the band, which the body paints over and which clips besides. That frame
  reaches the pills **by id**, and an id declared inside an `if` can't be read from outside
  it. Today those pills are unconditional in a page header, which is exactly why the frame
  works and why tab-gating them is what breaks it. So the ids sit at a component root the
  sheet can see, and the file publishes `tip-x/-y/-w/-h/-label/-visible` the way
  `MosaicTabHero` publishes the bar's — the same answer one boundary down. The row itself
  hides by collapsing to a zero-width `clip: true` cell (`min-width: 0px` spelled out, since
  an explicit min is what *replaces* the constraint the pill's own bound width would
  otherwise merge in; `preferred`/`max` carry the ternary): nothing paints, and `Clip`
  swallows every event outside its empty rect, so the pills are as unreachable as an
  unmounted branch. **One component holding all five rows** rather than four inline plus
  one, because the band's slot spaces its cells and a second always-mounted child would
  offset every other tab's row from the band's right edge. Re-`if`-gating the row is a
  compile error, not a silent regression — the `tip-*` bindings stop resolving — which is
  why the pin that matters is `the_playlist_action_tooltip_is_published_rather_than_drawn`,
  covering the *other* half: that the sheet still draws the frame.
- **A tab pick's direction goes through `Nav.pending-enter-from`, not straight from
  `band.tab-enter-from`.** The plan binds the tab branches to the band's published
  direction, which is right for Favorites and wrong here for a reason Favorites can't have:
  it has no details. A back out of a detail mounts a *grid* branch with
  `Nav.pending-enter-from` already set to `left` by `nav_transition::mark`; bound to the
  band, that branch would slide in from whichever way the last **tab pick** went. So the
  sheet's `tab-selected` handler writes the band's answer into `Nav.pending-enter-from` and
  all nine branches keep reading it — one channel for a tab pick, a drill, a back and a
  Mouse-4/5 step. The band settles `tab-enter-from` before it emits `tab-selected`, so the
  read is already the new one. `enabled: band.tab-anim-armed` still gates the five tab
  branches and still doesn't gate the four detail ones.
- **Phase 0's open question — whether Songs also empties its model on leave — is answered
  *no*, by moving the fix to the other end.** `on_tab_changed` now dispatches the empty
  needle through `filter::dispatch` after clearing the band's box, so the **entering** tab's
  own `filter` is cleared and its model rebuilt from the cache Rust already holds,
  synchronously, ahead of the section gate's re-fetch. That closes the window the model
  clear was for — a tab painting rows built from a needle nothing on screen shows — without
  buying a blank list across the app's slowest query on every return to Songs. It also
  spells the nine-way hand-off once instead of twice.
- **The four grids now sit flush, and that is what makes the sheet's one `GridGeometry`
  honest.** Each grid tab mounts its grid at `y: Theme.pad-md; width: 100%` — the
  `favorites/most-played-tab.slint` contract verbatim — so `avail-width: body.width` is the
  width the cards actually get. The cards' inset drops from `pad-lg + gap` to the grid's own
  `gap`, which also lines them up closer to the band's own padding. The alternative was the
  sheet restating the tab body's inset, where a mismatch silently over-counts columns and
  overflows the cards. **The Songs tab keeps its `pad-lg`** — same split Favorites has, and
  for the same reason: the geometry doesn't reach it.
- **`GridColumnsSync.seed` gates on the tab, not on the detail id.** It writes `columns` to
  all four grid globals (an unmounted grid's *fetch* reads the property, so the write is
  what stops it entering at a stale count) and invokes `columns-changed` only on the mounted
  tab. Gating that invoke on the detail id as well would have looked tighter and left a
  detail closing onto a grid chunked for a window size that is gone.
- **`ui::tab_bar::tests`' `LibraryPage` lost its `view` field**, which `include_str!`'d the
  five deleted files — a compile break, not a test failure. The five
  `total-count >= 0` guards are one `count-text` ternary now, so they are pinned against the
  sheet through a single `MY_LIBRARY_VIEW` const.

The five per-view `blur-search-tick` properties are dead as of this phase (`Tracks`,
`Albums`, `Artists`, `Genres`, `Playlists`) — their last writers went with the headers, and
the Songs tab's `request-blur-search` was re-pointed at `MyLibrary`'s. They are deleted in
Phase 5 with the four detail ones rather than piecemeal here.

- **`views/my-library/{songs,albums,artists,genres,playlists}-tab.slint`** — the five
  current view bodies with header, `SearchBar`, pill rows, `FilterThrottle`, backdrop
  `TouchArea` and grid geometry removed. The four grid tabs take
  `in card-w / card-h / row-h / gap` from the sheet (the `favorites/most-played-tab.slint`
  contract) and keep **their own** `OverlayScrollbar`s — Slint can't read an id declared
  inside an `if` from outside it.
- **`views/my-library-view.slint`** — the mount sheet, which **already exists**: Phase 2
  put `band := LibraryTabBand` where Phase 1's temporary `TabBar` was, and with it the
  `count-text` and `filter-placeholder` ternaries, the `FilterThrottle`, the root backdrop
  `TouchArea { clicked => { MyLibrary.blur-search-tick += 1; } }` and the `tab-tip` frame —
  all of them things the band cannot be mounted without. The `page-w` mirror and its mount
  `Timer` moved **into** the band and are gone from the sheet. Phase 3 swaps the five
  bodies for the tab files above; the branch chain stays. What it gains:
  - one `GridGeometry` off `body.width`, plus one `GridColumnsSync` whose `seed(c)` writes
    `columns` to **all four** grid globals and invokes `columns-changed` only on the
    mounted one. Writing all four is what stops an unmounted grid re-chunking on entry at
    a stale column count.
  - the per-tab pill rows, handed to the band as `@children`: Songs → selection chip +
    `ColumnTogglePopup`; Albums/Artists/Genres → their existing sort `ActionPill`s;
    Playlists → its four action buttons (New / Smart / Import / Export). The band's slot is
    already there and empty.
  - **the five per-view `SearchBar`s and count lines go**, which is what makes the shared
    box the only one — and with them the "shared box empty, per-view box still filtering"
    state Phase 2 left behind. `on_tab_changed` gains the clear of the entering tab's own
    filter, which is what `a_tab_pick_clears_the_filter_on_both_sides` pins.
  - `body := Rectangle { clip: true; … }` with one `ViewTransition` per branch, **and the
    two kinds take different `enter-from` sources**: the four tab branches read
    `band.tab-enter-from` under `enabled: band.tab-anim-armed` (the Favorites
    disarm-at-mount rule, so the page's own fade-up isn't compounded into a diagonal),
    while the four detail branches keep `Nav.pending-enter-from` — that is what
    `nav_transition::mark` writes, and it is what makes a cross-tab drill and a Mouse-4/5
    step slide the right way. **The band publishes both today and nothing reads them**;
    all nine branches are still on `Nav.pending-enter-from`.
  - a **second** tooltip frame beside the `tab-tip` the band already anchors: the Playlists
    tab's own `header-tip` — its four action pills mount `tooltip-overlay: true` and the
    current `playlists-view.slint` declares a five-property ternary chain for them.
- Delete `views/{tracks,album,artist,genres,playlists}-view.slint` once the five tab files
  replace them. The sidebar items and the router branches went in Phase 1.
- ☑ `app-window.slint`'s `watched-my-library-tab: MyLibrary.tab-idx` mirror — landed in
  Phase 1, since a tab pick could unmount Artist Detail from the moment the page existed.
  Its `changed` handler calls **both** `shortcut-scope.grab-focus()` and
  `CompositeScroll.reset()`; that is the **tenth** focus mirror and the **fifth** composite
  reset.

### Phase 4 — The four details under the band ☑

**What the phase actually landed**, and the five places the plan below was wrong or left
something open:

- **`MyLibrary.back()` was already wired; the gap was purely Slint.** `on_back` has
  dispatched to the mounted tab's `invoke_close_detail()` since Phase 1 — the sheet simply
  never handled `back-clicked`, and the button sat inside a `hero-t > 0` branch that
  `detail-open: false` kept dead. So the plan's "Rust: `MyLibrary.back()` routes to…"
  bullet cost one line in the sheet. **What it did buy was a dedup**: that handler was a
  verbatim copy of `nav_history::invoke_close_detail`'s five-arm match, and the two are now
  one `ui::my_library::close_open_detail(ui, tab)` — the band's arrow and a Mouse-4 step out
  of a detail are the same act, and `nav_history` keeps only its `section != NAV_MY_LIBRARY`
  guard on top.
- **Removing the four detail search boxes broke the filter in two ways the plan didn't
  name, and both had to be fixed here.** Phase 3 removed the five *grid* boxes; this phase
  removed the four *detail* ones, and each box was carrying a fact nothing else did.
  **`<Detail>.filter` stopped being written**, because `dispatch`'s detail arms pass the
  needle by *argument* and the property was kept current by the box's `<=>`. Its live
  reader is `playlist-detail.slint`'s `reorder-enabled`, which refuses a drag while the
  list is filtered — so a filtered playlist would have stayed reorderable, with the
  index → position mapping the drag depends on wrong. The four detail arms now `set_filter`
  before invoking, and **all nine arms read alike**, which is the simpler shape besides.
  And **the one box started lying whenever a detail opened or closed**: a drill-in finds
  the detail's filter already cleared by `open_*` while the box still holds the grid's
  needle, and a back out finds the grid's needle untouched (the rebuild is memoized on it)
  while the box reads empty. `MyLibrary.detail-scope-changed()` — fired from four
  `changed watched-*-id` mirrors on the sheet, since `changed` rejects a path expression on
  a global — routes to **`filter::sync_box`**, `dispatch` read backwards: it takes the
  mounted surface's own filter. **Not the tab pick's clear-both-sides rule**, which was
  considered and rejected: clearing on the way out drops the user's grid filter on every
  back, which the two-box arrangement never did.
- **`ui::hero_chips::tests` had to move forward from Phase 5, and not as a test failure.**
  `HERO_VIEWS` `include_str!`'d the four `*-detail-view.slint` files and `detail-header.slint`
  itself, so deleting them is a **compile break**. `HERO_VIEWS` is now `[…; 2]` — the two
  shared bands, `mosaic-tab-hero.slint` and `library-tab-band.slint`, each standing for the
  pages under it — and the new **`BAND_HOSTS`** is `MOSAIC_HOSTS`' twin over the sheet plus
  the four detail bodies, pinning that the sheet mounts the band and the bodies mount
  nothing. The bodies are the half worth pinning: a detail regrowing a header of its own
  passes every other check in the file.
- **Two of that file's pins had to be re-authored rather than ported, and the second is a
  real geometry change.** `no_hero_view_sizes_its_own_artwork_tile` moved off `DetailHeader`
  onto the band. And `the_two_subtitled_heroes_keep_that_line_inside_the_title_row` became
  **`the_subtitled_heroes_share_one_collapsing_line`**, because the shape it asserted is
  gone: `ui-patterns.md` records that Album's artist and Playlist's description ride
  *inside* the title row since the `SearchBar` beside them has already claimed that height,
  and in the band the search box is up in the tab row. There is nothing to ride in, so
  Phase 2 built the subtitle as a sibling row and it costs its full line box plus a
  `pad-xs` gap. **The meta column is `Theme.hero-artwork` less the pill band it reserves —
  140 − 36 = 104 px — and a subtitled hero at two chip rows lands right at that ceiling.**
  What the band buys back is that there is exactly *one* subtitle row for four heroes, so
  the pin now holds the count (one, collapsing on `""`), the size (`font-size-md`), the
  order (above the chip strip) and that only the sheet names an entity. If a wrapped second
  chip row clips on Album or Playlist, the fix is a per-hero max into
  `hero_chips::write_rows`, not a taller band — the band clips, so the failure is bounded.
- **The band's `fallback-icon: "music-note"` default is correct and was left alone.** The
  plan called it a typo'd ligature inherited from `DetailHeader`; it is in fact a
  **sentinel** — `artwork-image.slint:56` branches on that exact string to reach
  `assets/icons/music-note.svg` rather than a Material Symbols glyph. Changing it to
  `music_note` would have silently swapped the placeholder art.

Five things beyond the written scope, all inside the blast radius:

- **`ArtworkImage` gained `has-cover`, and without it the Genre hero paints another
  detail's artwork.** The tile gates on `cover.width` alone, and the sheet's `cover`
  ternary has to bind *some* global on the Genre arm — Slint has no empty-`image` literal
  and `GenreDetail` owns no cover, its tile being a name-hashed gradient. The old
  `DetailHeader` sidestepped this by having Genre Detail pass no `cover` at all, which a
  ternary cannot do. **More than one detail is open as a matter of routine** —
  `seed_detail_from_settings` restores one per view whichever tab boot resumes — so this is
  reachable on a cold start, and it reads as a decode landing in the wrong view. The band
  passes `has-cover: root.artwork-path != ""`; the input defaults `true`, so every other
  mount is untouched, and the five fallback branches now read `!root.shows-cover` off one
  derived predicate rather than each respelling `cover.width == 0 &&`. The blur quartet
  needs no equivalent: `HeroBlurBackdrop` already gates both slots on `has-blur`, which the
  sheet holds `false` on that arm.

- **The four grid pill rows are now gated *off* their detail as well as on their tab**, and
  the Playlists collapsed cell with them. The plan's `@children` bullet only added the four
  detail rows; without the other half a sort row survives over an open album and sorts a
  grid nobody can see. Both halves route on the **same four predicates the body router
  uses**, derived once on the sheet and forwarded into `tab-pills.slint` as `in` properties
  — respelled there, the pills and the body could drift by one clause and only one of the
  nine states would show it.
- **The detail pill rows keep their in-tree tooltips.** `Tooltip`'s default side is `above`
  and the band's slot rides its lower edge, so the pill lands inside the band, painted after
  everything the band owns and before the body. The Playlists row's four overlay pills are
  *inherited* from the old page header, not required by the band, and were left exactly as
  Phase 3 left them.
- **Only Album and Genre collapsed their inset onto the root**, where the plan said all
  four could. Artist keeps per-child because `below-hero` is the region
  `CompositeScrollbars` measures and the `CompositeScroll` hover sentinel covers, so it has
  to run full-bleed; Playlist keeps per-child because its empty state and drop banner
  deliberately fill `body`. Artist Detail also **lost its root backdrop `TouchArea`** —
  `hover-catch` now covers the whole body and already does that job.
- Eight comments naming a component that no longer exists were fixed
  (`hero-blur-backdrop`, `cover-mosaic`, `hero-backdrop`, `sidebar`, `theme`, plus
  `AlbumView` / `ArtistView` references in `albums/detail.rs`, `artists/detail.rs` and
  `callbacks/albums/detail.rs` that Phase 3 had already stranded).

**What did *not* change**, and is again the whole premise: the five views' data layer.
`release_detail_hero_images!`, `clear_detail`, `release_detail_artwork`, `restore_origin`,
`set_last_detail_id`, `nav_history::record_current` and every `section_active()` publish
gate on `hero_backdrop` / `hero_chips` are untouched. `PlaylistDetail` still carries no
origin pair and still needs none.

- **`views/my-library/{album,artist,genre,playlist}-detail.slint`** — the four detail
  views with `DetailHeader`, its `@children` column (title row, `SearchBar`,
  `HeroChipStrip`, spacer, pill row) and the `back-clicked` handler removed. What remains
  is body only: the `TrackList` (or Artist Detail's composite scroller, or Playlist
  Detail's `DraggableTrackList` + drop banner + empty state) and its scrollbars — two
  `OverlayScrollbar`s each, except Artist Detail, which keeps `CompositeScrollbars`.
  Their root can now pad uniformly on three sides like a grid page — the reason they
  couldn't (`DetailHeader` is full-bleed) has moved into the band.
- **The backdrop `TouchArea`s are rewired, not deleted.** Each of the four has one and
  Artist Detail has a **second** inside its `hover-catch`; all of them now write
  `MyLibrary.blur-search-tick`. (The Songs tab's `request-blur-search` forward was rewired
  in Phase 3, with the header it belonged to.) The nine per-view `blur-search-tick`
  *properties* are what go dead (Phase 5); their writers outlive them.
- **The sheet's `detail-open: false` literal becomes the private derivation of decision 1**
  — the phase's first edit, and the one that makes every other item here visible. Nothing
  may flip it before `DetailHeader` is gone from all four detail views, or the page wears
  two banners at once.
- The sheet resolves the band's hero facts per open detail (title / subtitle / artwork /
  blur quartet / badge / Genre's `tile-bg` gradient) as private `property` ternaries, and
  the four detail branches join the body router (nine branches total, split on
  `*Detail.*-id`, exactly as the current chain is).
- **Measure the morph here** — it is the first phase in which it runs. The animated
  `preferred-height` re-runs the page's layout every frame for `dur-spatial`; it should be
  invisible because the bodies are virtualized. Fallbacks in decision 6.
- The four detail pill rows join the `@children` `if` chain, keyed on
  `MyLibrary.tab-idx == tab-X && <X>Detail.<x>-id >= 0`.
- **`components/detail-header.slint` is retired** once nothing mounts it. Its
  `ArtworkImage` + `HeroBlurBackdrop` composition moves into the band.
- Rust: `MyLibrary.back()` routes to the open detail's existing `close-detail()`, so every
  teardown (`release_detail_hero_images!`, `clear_detail`, `release_detail_artwork`,
  `set_last_detail_id(None)`, `nav_history::record_current`) is unchanged, and it restores
  `origin-tab` alongside `origin-nav-index` per decision 9. The `hero_backdrop` /
  `hero_chips` publish gate is still each view's `section_active()`, which the tab-scoped
  `SectionActiveGate` now makes mean "My Library is up *and* this tab is mounted" — no Rust
  change, and it closes the boot case for free (the four `if !section_active() { mark_dirty() }`
  seeds keep working).
- **Watch the teardown-versus-morph seam in the manual pass.** `close-detail` →
  `release_detail_hero_images!` → `hero_backdrop::reset` + `hero_chips::clear` all land on
  the leave edge, and then the band spends `dur-spatial` shrinking over an already-reset
  backdrop; a tab switch away from a tab with a detail open does the same. The idle pane
  fading in over it should cover this. If it pops, delay the reset by `dur-spatial` rather
  than slowing the morph.

### Phase 5 — Cleanup, i18n, docs

- Delete the nine now-dead `blur-search-tick` properties (their writers were rewired in
  Phase 4). **No Rust moves with them** — every one of the nine writers is Slint-side.
- ☑ `My Library` msgid — landed in Phase 1 with the sidebar row, since
  `ui::locale::tests::every_translated_literal_has_a_msgid_in_every_catalogue` is part of
  every phase's gate. What may still be owed here is the count plurals, **if** the band's
  `count-text` reworded any of them; the five tab labels reuse the msgids the page titles
  already registered, so nothing is orphaned there.
- **No icon work.** `library_music`, `music_note` and `arrow_back` are all already in
  `scripts/icons.txt`, so neither `subset-icon-fonts.sh` nor `check-icons.py` needs a run.
  (Stated because the omission is what produces tofu.)
- ☑ `ui::hero_chips::tests` — landed in Phase 4 and had to: `HERO_VIEWS` `include_str!`'d
  the four detail views and `detail-header.slint`, so deleting them was a **compile break**
  rather than a test failure. `HERO_VIEWS` is `[…; 2]`, `BAND_HOSTS` is beside
  `MOSAIC_HOSTS`, and two pins were re-authored — see that phase's write-up for the
  subtitle one, which asserts a different shape rather than the same one moved.
- `ui::placeholder_tests`: `BUDGETING_HOSTS` is `[(&str, &str); 2]` (settings-view +
  mosaic-tab-hero) and becomes **3**, gaining the band — it budgets its header row and
  drives `input-width` the same way. This is an addition, not a re-verification.
- `ui::row_match` surface counts and the `link_two_way … viewport_y` count in
  `slint-pitfalls.md` (still 2 — Artist Detail and Browse) re-verified.
- Docs: root `CLAUDE.md` (a My Library bullet in the module map pointing at the Favorites
  bullet as the reference contract, the nav index table, the retired 4–7),
  `.claude/rules/ui-patterns.md` (the band beside `MosaicTabHero`; the tab-scoped
  `SectionActiveGate`; the "a tab switch is a section switch" rule; **the two claims
  Phase 0 already falsified** — line 31's "the four older grid views still carry
  hand-copied versions" and line 45's "the four older entity grids deliberately keep the
  older shape", the latter also owing the `>= 0` guard rule and what Tracks' leave arm
  cost, and both now naming files that no longer exist; **and the top-layer tooltip list**,
  whose three mounts are `my-library-view.slint`'s two — `tab-tip` and the `header-tip`
  that moved off `playlists-view.slint` — plus `settings-view.slint`'s and the sidebar
  rail's. `tab-pills.slint` is worth a line of its own there: it is the third answer to
  "publish the anchor, let the host draw it", and the first where the boundary crossed is
  an `if` rather than a component),
  `.claude/rules/slint-pitfalls.md` (the animated-root-**height** twin of the width entry),
  `README.md` feature blurb.
- Delete this file.

## Regression checklist

Things this codebase has already paid for once, which this change is positioned to break:

- ☑ **Boot ordering, and it is not the Favorites ordering.** `install_views` keeps
  hydrating `Nav.selected-index` before `wire_all` — *and* `ui::my_library::seed_tab` runs
  there too, beside it. There is no second half waiting for a handle, because there is no
  handle. Seeding the tab after the five `wire_*` calls (the Favorites shape) leaves all
  five `section_active` shadows answering for `tab-idx == 0`: Songs wrongly active, the
  persisted tab wrongly inactive, one wasted full-library query per launch. Pinned by
  `boot::ui_setup::tests::the_persisted_my_library_tab_is_seeded_before_any_view_is_wired`.
- ☑ **`ChangeTracker` baselining** — the tab-scoped gates baseline silently inside
  `AppWindow::new()`, so each of the five `wire_*` seeds its `section_active` shadow through
  `ui::my_library::tab_is_mounted`, not from the nav index alone. Getting this wrong leaves
  one section wrongly active all session and re-fetches the whole library per song. Pinned
  by `ui::my_library::tests::every_section_seed_reads_the_mounted_tab`.
- ☑ **The filter has two contracts, not one.** `apply-filter` ignores its argument and
  Rust reads `<Global>.filter`; `filter-changed` uses its argument and Rust folds it into
  a `Mutex<Needle>`. The grid/list arms therefore **write** `<Global>.filter` before
  invoking the rebuild — the one-way binding this checklist used to forbid writing over
  turned out not to be spellable in Slint at all (decision 3). The four `*Detail.filter`s
  keep taking the argument and stay Rust-written on detail open (and, on Artist Detail, on
  close).
- ☑ **A drill's origin is a pair.** `origin-nav-index` alone cannot tell one My Library tab
  from another, and one `origin-tab` cannot hold two simultaneously open details. Per
  detail global, written in the same synchronous stretch as `origin-nav-index` — and the
  reading half is one type, `cross_tab_nav::Origin`, so the mid-fetch guard is spelled once.
- ☑ **`nav_history` replay has three arms.** Cross-section, cross-tab, same-tab. The middle
  one is the one a `(section, tab)` comparison alone leaves unhandled. **Recording is the
  other half**: without `NavEntry.tab`, two tabs of one page are the same snapshot and
  `record`'s dedup swallows the second, so Mouse-4 cannot walk back across a tab switch
  (`nav_history_tests::a_tab_switch_is_not_a_duplicate_of_the_tab_it_left`).
- **`TabBar` write-before-emit** — `selected-index` is already the new tab when
  `tab-changed` reaches Rust. Anything needing the outgoing tab reads `previous-index`.
- ☑ **`CompositeScroll.reset()`** — five call sites now, not four.
- ☑ **Focus regrab** — ten mirrors now, not nine.
- ☑ **`ViewTransition` disarm** — `tab-anim-armed` starts `false` and is armed in the bar's
  `selected` handler, never from a mount `Timer`. It gates the five tab branches and not
  the four detail ones. **The *direction* is a separate question and all nine share one
  answer**, `Nav.pending-enter-from`, which the sheet writes on a tab pick — binding the
  tab branches to the band instead leaves a back out of a detail sliding whichever way the
  last tab pick went.
- ☑ **Bottom padding** — never a root `padding-bottom` over a scroller; it reads as a dead
  strip. The four grid tabs inset nothing (the grid runs flush and takes its own `gap`,
  the Favorites contract); Songs insets left/right/top.
- ☑ **The retired indices leave three maps behind** — `view-title()`,
  `rss_sampler::format_view` and the persisted-index range check. Each named 4–7 and each
  failed differently: an untranslated "Melodia" heading, a diagnostic that stops
  distinguishing the page, and a released user booting onto `PlaceholderView`. The third is
  `ui::my_library::fold_retired_nav_index`, a pure fn so the compatibility path is testable
  without a window. `format_view` also gained the `PlaylistDetail` arm it never had.
- ☑ **`views.json` compatibility** — the app is publicly released. Dropping
  `favorites_*_collapsed` is the precedent that removal is safe (serde ignores unknown
  keys); `my_library_tab` inherits `#[serde(default)]` from the struct-level attribute, and
  a persisted `last_nav_index` of 4–7 must clamp rather than land on nothing.
- **No new bools on `ViewStateData`** — not because it is near
  `clippy::struct_excessive_bools`' cap (it holds one bool of three) but because
  `view_state.rs` states the rule outright: a new persisted view
  flag is an int / string / map. `my_library_tab` is an `i32`; `origin-tab` lives on the
  detail globals, not the file.
- **`release.yml`** — no new workspace member here, so the `melodia-*` glob hazard doesn't
  apply; nothing to do.

## Verification

Per phase:

```bash
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Targeted test additions, all `include_str!` source pins in the house style:

- ☑ `ui::my_library::tests::tab_count_matches_the_tabs_slint_declares` — parses
  `MyLibrary.tab-count`, counts the `tab-*` constants scoped to that global's body, and
  checks the bar's two inline arrays plus the sheet routing a body per declared tab.
- ☑ `boot::ui_setup::tests::the_persisted_my_library_tab_is_seeded_before_any_view_is_wired`
  — the byte-offset shape of the existing nav pin. The mutation to check is moving the call
  down beside the `5c2h` seeds, which is where a tab seed looks like it belongs.
- ☑ `…::every_tab_carries_its_own_section_gate` — five `SectionActiveGate` mounts at
  `index: 3` with five distinct `tab-index:` values, each carrying `current-tab` (without
  it `tab-index` compares against the gate's own `-1` and the section never activates).
- ☑ `…::every_section_seed_reads_the_mounted_tab` — the five `section_active` seeds go
  through `tab_is_mounted`, not the nav index.
- ☑ `…::every_detail_records_the_tab_it_was_opened_from` — the three detail globals declare
  `origin-tab`, every site writing `origin-nav-index` writes it too, and
  `artists/cross_tab` stays routed through the shared hand-off rather than re-inlining it.
- ☑ `…::the_retired_indices_fold_onto_the_page_that_absorbed_them` and
  `…::the_sidebar_offers_one_row_for_the_whole_page`.
- ☑ `ui::nav_history::tests::a_tab_switch_is_not_a_duplicate_of_the_tab_it_left`.
- ☑ `…::a_tab_pick_clears_the_filter_on_both_sides` — the band's box *and* the entering
  tab's own filter, plus that no tab body kept a `SearchBar` or a `FilterThrottle` of its
  own. The mutation to check is dropping the dispatch: the box then reads empty over a tab
  still filtered by the needle it was left with.
- ☑ `…::every_sort_pill_asks_for_a_field_the_comparator_knows` — the Albums / Artists /
  Genres rows finally get the pin the Favorites Artists row already has, and one file
  holding all three is what made it cheap. The Favorites copy's "nothing pins this for the
  Albums / Artists / Genres rows" line is gone with it.
- ☑ `…::the_playlist_action_tooltip_is_published_rather_than_drawn` — the four pills
  suppress their in-tree tooltip, `tab-pills.slint` publishes all six anchors, and the
  sheet's `header-tip` reads them. Anchored on the pill instead, all four are simply never
  seen.
- ☑ `ui::library_tab_band_tests` — as listed in Phase 2.
- ☑ `ui::tab_bar::tests` extended, on a `LIBRARY_PAGES` array beside `CURATED_PAGES`:
  `every_library_count_starts_at_the_unfetched_sentinel` (declaration **and** the `>= 0`
  guard), `every_library_leave_rewinds_the_count_it_numbered`, and
  `the_section_gate_ignores_its_tab_predicate_when_a_section_has_none` (both new gate
  properties default `-1`, and the predicate keeps its negative short-circuit — a `0`
  default silently deactivates all nine sections).
- ☑ `…::the_morph_is_driven_by_the_sheets_own_derivation`,
  `…::the_back_arrow_routes_to_the_pages_own_close` and
  `…::every_hero_fact_the_band_declares_is_fed_by_the_sheet` — the three seam pins in
  `library_tab_band_tests.rs`, which now `include_str!`s the mount sheet beside the band. A
  band nobody drives passes all eight of its own pins; the last of the three is the one that
  catches a *new* fact added to the band and never bound, which sits at its default and
  fails nothing.
- ☑ `…::the_hero_tile_suppresses_a_cover_the_open_detail_does_not_own` — the band's
  `has-cover` gate. The mutation to check is dropping it, which only shows on a boot that
  restored a detail on some *other* tab.
- ☑ `…::the_pill_row_follows_the_body_router` — the four predicates are derived on the
  sheet, forwarded into the pills, and gate the grid rows *off* their detail as well as the
  detail rows *on* it.
- ☑ `…::a_detail_open_or_close_reseats_the_shared_box` — the four id mirrors, the callback,
  and `filter::sync_box` reaching all nine surfaces. The mutation to check is dropping the
  close half: the grid comes back correctly filtered and only the box lies.
- ☑ `ui::hero_chips::tests` — `HERO_VIEWS` at 2, plus the new `BAND_HOSTS`. Landed in
  Phase 4; see there for why it couldn't wait.
- `ui::placeholder_tests` — `BUDGETING_HOSTS` at 3.

Manual, after Phase 4:

1. Sidebar → My Library lands on the persisted tab; every tab's grid/list paints, its
   count is right, its pills are its own.
2. Type in the search box on each tab — filters that tab only; switching tabs clears it.
3. Resize narrow → the tab bar goes icon-only and the search box compresses to its floor
   before the tabs give up their labels; the compact tooltip appears below a tab, and the
   Playlists tab's own action-pill tooltips still work.
4. Open an album → the band grows into the hero, the back button appears left of the tabs,
   chips and pills land, the search box now filters the album's tracks. Back → it shrinks
   back to the count row, with no backdrop pop at the seam. **Measure the morph here** —
   the animated `preferred-height` re-runs the page's layout every frame for `dur-spatial`.
   Fallbacks in decision 6; don't slow the body to hide it.
4b. **A subtitled hero at a width that wraps the chips** — an album with an artist, or a
   playlist with a description, narrowed until the strip takes a second row. The meta
   column is `Theme.hero-artwork` less the pill band it reserves (140 − 36 = 104 px) and
   that case lands right at the ceiling, because the band's subtitle sits on a row of its
   own where `DetailHeader`'s rode inside the title row. If the second row clips, the fix
   is a per-hero max into `hero_chips::write_rows` — one row when a subtitle is present —
   not a taller band.
4c. Filter the Albums grid, drill into an album, come back: the box and the grid agree at
   every step. Same on a playlist, and confirm a *filtered* playlist refuses a drag.
5. Right-click a track → Go to Artist from the Songs tab: lands on the Artists tab with
   the detail open; from there open one of its albums; back twice returns Artists → Songs.
6. Mouse-4 / Mouse-5 walk the history across tabs and details.
7. Miniplayer → full-window swap while a detail is open: the band remounts at the right
   size and the tab bar isn't stuck icon-only.
8. Light theme (Latte) with a detail open: the tab bar's selected label and the back
   button read against the blur, and the idle pane's tab colours read against `mantle`.
9. Genre Detail specifically: gradient backdrop, no blur, tile colours intact through the
   morph in both directions — and check it **after a restart that resumed with an album or
   playlist detail open on another tab**, which is the case `has-cover` exists for.
10. `RUST_LOG=info MELODIA_RSS_SAMPLE=1` — the `view=` tag names the tab and the open
    detail, and idle RSS after walking all five tabs and back sits where it did before.
