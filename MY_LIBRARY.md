# My Library — the five library views become one tabbed page

Working doc. Keep the phase markers current; delete this file when the feature ships.

| phase | status |
|---|---|
| 0 — Prep | ☑ done |
| 1 — `MyLibrary` global, nav plumbing, empty page | ☐ not started |
| 2 — `LibraryTabBand` | ☐ not started |
| 3 — The five tab bodies and the mount sheet | ☐ not started |
| 4 — The four details under the band | ☐ not started |
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

So the sheet binds each view's filter one-way — `Tracks.filter: MyLibrary.filter;` and the
four grid twins — which keeps the property the input `rebuild_grid` / `compute_indices`
already read, and makes "clear the filter on a tab pick" one write instead of six.
`src/ui/my_library/filter.rs` then only *routes*: on (active tab, open detail id) it
invokes the mounted tab's existing rebuild, and for an open detail also calls that view's
existing `set_filter`. Nine `if` branches in one Rust function beats nine branches in a
Slint callback body, and Rust already holds both facts.

**The constraint that rides along: once `<Global>.filter` carries a binding, Rust must
never write it.** Nothing does today — the write side is the per-view `SearchBar`'s `<=>`,
which goes away with the headers. The nine `blur-search-tick` properties go too, with one
`MyLibrary.blur-search-tick` in their place; their *writers* are rewired, not deleted (see
Phase 4).

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
visible rows only. Measure once at Phase 2; if it janks, drop to `dur-med` or snap the
height and animate only the contents. Do not slow the body to hide it.

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
and `cargo test --locked`. Phases 1–2 leave the running app unchanged; Phase 0 has exactly
one visible change, recorded below.

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

### Phase 1 — `MyLibrary` global, nav plumbing, empty page

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
  branch replacing the nine; `MyLibraryView` is a stub in this phase. One
  `SectionActiveGate { index: 3; … MyLibrary.section-active-changed(active) }` plus the
  five tab-scoped ones. The old five sidebar items and nine branches are deleted last, in
  Phases 3/4.

### Phase 2 — `LibraryTabBand`

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

**Measure the morph once, here.** The animated `preferred-height` re-runs the page's layout
every frame for `dur-spatial`; it should be invisible because the bodies are virtualized,
but the number to check is this phase's, not Phase 4's. Fallbacks in decision 6.

**Tests** — `src/ui/tests/library_tab_band_tests.rs`, mirroring `mosaic_tab_hero_tests.rs`
(the five ported fixes) plus: the height min/preferred/max split and the root `clip`; that
`hero-t` is written from `changed detail-open` and not left bound; that the back button
takes both brushes from `HeroBackdrop`; that the band publishes its tooltip anchor rather
than drawing one.

### Phase 3 — The five tab bodies and the mount sheet

- **`views/my-library/{songs,albums,artists,genres,playlists}-tab.slint`** — the five
  current view bodies with header, `SearchBar`, pill rows, `FilterThrottle`, backdrop
  `TouchArea` and grid geometry removed. The four grid tabs take
  `in card-w / card-h / row-h / gap` from the sheet (the `favorites/most-played-tab.slint`
  contract) and keep **their own** `OverlayScrollbar`s — Slint can't read an id declared
  inside an `if` from outside it.
- **`views/my-library-view.slint`** — the mount sheet:
  - the private `detail-open` derivation (decision 1) and one `GridGeometry` off
    `body.width`, plus one `GridColumnsSync` whose `seed(c)` writes `columns` to **all
    four** grid globals and invokes `columns-changed` only on the mounted one. Writing all
    four is what stops an unmounted grid re-chunking on entry at a stale column count.
  - the five one-way filter bindings of decision 3 (`Tracks.filter: MyLibrary.filter;` …),
    `FilterThrottle { fire() => { MyLibrary.filter-changed(MyLibrary.filter); } }`, and the
    root backdrop `TouchArea { clicked => { MyLibrary.blur-search-tick += 1; } }`.
  - the `count-text` ternary — inline `@tr("{n} album" | "{n} albums" % Albums.total-count)`
    literals guarded on `>= 0`; a Rust-seeded `[string]` would render untranslated.
  - `band := LibraryTabBand { … }` with the per-tab pill rows as `@children`:
    Songs → selection chip + `ColumnTogglePopup`; Albums/Artists/Genres → their existing
    sort `ActionPill`s; Playlists → its four action buttons (New / Smart / Import / Export).
  - `body := Rectangle { clip: true; … }` with one `ViewTransition` per branch, **and the
    two kinds take different `enter-from` sources**: the four tab branches read
    `band.tab-enter-from` under `enabled: band.tab-anim-armed` (the Favorites
    disarm-at-mount rule, so the page's own fade-up isn't compounded into a diagonal),
    while the four detail branches keep `Nav.pending-enter-from` — that is what
    `nav_transition::mark` writes, and it is what makes a cross-tab drill and a Mouse-4/5
    step slide the right way.
  - **two** tooltip frames, declared after the body: the `tab-tip`, and the Playlists tab's
    own `header-tip` — its four action pills mount `tooltip-overlay: true` and the current
    `playlists-view.slint` declares a five-property ternary chain for them.
- Delete `views/{tracks,album,artist,genres,playlists}-view.slint`, the five sidebar
  items and the five main-view router branches.
- `app-window.slint`: a `watched-my-library-tab: MyLibrary.tab-idx` mirror whose `changed`
  handler calls **both** `shortcut-scope.grab-focus()` (a tab body unmounts with focus
  inside it — the filter box now lives in the always-mounted band, so this covers the
  `TrackList` case) and `CompositeScroll.reset()` (Artist Detail is composite and a tab
  pick unmounts it with no unmount hook). That is a **tenth** focus mirror and a **fifth**
  composite reset.

### Phase 4 — The four details under the band

- **`views/my-library/{album,artist,genre,playlist}-detail.slint`** — the four detail
  views with `DetailHeader`, its `@children` column (title row, `SearchBar`,
  `HeroChipStrip`, spacer, pill row) and the `back-clicked` handler removed. What remains
  is body only: the `TrackList` (or Artist Detail's composite scroller, or Playlist
  Detail's `DraggableTrackList` + drop banner + empty state) and its scrollbars — two
  `OverlayScrollbar`s each, except Artist Detail, which keeps `CompositeScrollbars`.
  Their root can now pad uniformly on three sides like a grid page — the reason they
  couldn't (`DetailHeader` is full-bleed) has moved into the band.
- **The backdrop `TouchArea`s are rewired, not deleted.** Each of the four has one, Artist
  Detail has a **second** inside its `hover-catch`, and the Songs tab's `TrackList`
  forwards `request-blur-search`. All of them now write `MyLibrary.blur-search-tick`. The
  nine per-view `blur-search-tick` *properties* are what go dead (Phase 5); their writers
  outlive them.
- The sheet resolves the band's hero facts per open detail (title / subtitle / artwork /
  blur quartet / badge / Genre's `tile-bg` gradient) as private `property` ternaries, and
  the four detail branches join the body router (nine branches total, split on
  `*Detail.*-id`, exactly as the current chain is).
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
  Phase 4).
- `melodia-ui/translations/*/LC_MESSAGES/melodia-ui.po` × 6: new msgids (`My Library`,
  the count plurals if their wording changed). The five tab labels reuse the msgids the
  page titles already registered, so nothing is orphaned there. The gate is
  `ui::locale::tests::every_translated_literal_has_a_msgid_in_every_catalogue`.
- **No icon work.** `library_music`, `music_note` and `arrow_back` are all already in
  `scripts/icons.txt`, so neither `subset-icon-fonts.sh` nor `check-icons.py` needs a run.
  (Stated because the omission is what produces tofu.)
- `ui::hero_chips::tests`: `HERO_VIEWS` is `[(&str, &str); 5]` today (the four detail views
  + `mosaic-tab-hero.slint`) and becomes `[…; 2]` — `mosaic-tab-hero.slint` +
  `library-tab-band.slint`. `MOSAIC_HOSTS` is untouched. Add a `BAND_HOSTS` array in its
  shape, covering `my-library-view.slint` **and the four detail bodies**, pinning that each
  still *mounts* the band (or nothing) and has grown no title, chip strip or artwork size of
  its own. Pinning the sheet alone would miss the likelier regression — a detail body
  quietly regrowing a header.
- `ui::placeholder_tests`: `BUDGETING_HOSTS` is `[(&str, &str); 2]` (settings-view +
  mosaic-tab-hero) and becomes **3**, gaining the band — it budgets its header row and
  drives `input-width` the same way. This is an addition, not a re-verification.
- `ui::row_match` surface counts and the `link_two_way … viewport_y` count in
  `slint-pitfalls.md` (still 2 — Artist Detail and Browse) re-verified.
- Docs: root `CLAUDE.md` (a My Library bullet in the module map pointing at the Favorites
  bullet as the reference contract, the nav index table, the retired 4–7),
  `.claude/rules/ui-patterns.md` (the band beside `MosaicTabHero`; the tab-scoped
  `SectionActiveGate`; the "a tab switch is a section switch" rule; **and the two claims
  Phase 0 already falsified** — line 31's "the four older grid views still carry
  hand-copied versions" and line 45's "the four older entity grids deliberately keep the
  older shape", the latter also owing the `>= 0` guard rule and what Tracks' leave arm
  cost),
  `.claude/rules/slint-pitfalls.md` (the animated-root-**height** twin of the width entry),
  `README.md` feature blurb.
- Delete this file.

## Regression checklist

Things this codebase has already paid for once, which this change is positioned to break:

- **Boot ordering, and it is not the Favorites ordering.** `install_views` keeps hydrating
  `Nav.selected-index` before `wire_all` — *and* `seed_tab_property` runs there too, beside
  it. Only `seed_tab_shadow` waits for the handle. Seeding the tab after the five `wire_*`
  calls (the Favorites shape) leaves all five `section_active` shadows answering for
  `tab-idx == 0`: Songs wrongly active, the persisted tab wrongly inactive, one wasted
  full-library query per launch.
- **`ChangeTracker` baselining** — the tab-scoped gates baseline silently inside
  `AppWindow::new()`, so each of the five `wire_*` must seed its `section_active` shadow
  from `Nav.selected-index == 3 && MyLibrary.tab-idx == <its tab>`, not from the nav index
  alone. Getting this wrong leaves one section wrongly active all session and re-fetches
  the whole library per song.
- **The filter has two contracts, not one.** `apply-filter` ignores its argument and Rust
  reads `<Global>.filter`; `filter-changed` uses its argument and Rust folds it into a
  `Mutex<Needle>`. Once the sheet binds `<Global>.filter`, **Rust must never write it** —
  a binding and a `set_filter` on the same property is a silent one-way loss.
- **A drill's origin is a pair.** `origin-nav-index` alone cannot tell one My Library tab
  from another, and one `origin-tab` cannot hold two simultaneously open details. Per
  detail global, written in the same synchronous stretch as `origin-nav-index`.
- **`nav_history` replay has three arms.** Cross-section, cross-tab, same-tab. The middle
  one does not exist today and is the one a `(section, tab)` comparison alone leaves
  unhandled.
- **`TabBar` write-before-emit** — `selected-index` is already the new tab when
  `tab-changed` reaches Rust. Anything needing the outgoing tab reads `previous-index`.
- **`CompositeScroll.reset()`** — five call sites now, not four.
- **Focus regrab** — ten mirrors now, not nine.
- **`ViewTransition` disarm** — `tab-anim-armed` starts `false` and is armed in the bar's
  `selected` handler, never from a mount `Timer`. Detail branches don't use it at all;
  they keep `Nav.pending-enter-from`.
- **Bottom padding** — the tab bodies inset left/right/top only; a root `padding-bottom`
  over a scroller reads as a dead strip.
- **The retired indices leave three maps behind** — `view-title()`, `rss_sampler::format_view`
  and the persisted-index range check. Each names 4–7 today and each fails differently:
  an untranslated "Melodia" heading, a diagnostic that stops distinguishing the page, and
  a released user booting onto `PlaceholderView`.
- **`views.json` compatibility** — the app is publicly released. Dropping
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

- `ui::my_library::tests::tab_count_matches_the_tabs_slint_declares` — parses
  `MyLibrary.tab-count`, counts the `tab-*` constants scoped to that global's body, the
  sheet's `if MyLibrary.tab-idx == … : ViewTransition` branches, and the `@tr(` entries in
  the inline `labels` array.
- `…::the_persisted_tab_is_seeded_before_any_view_is_wired` — the byte-offset shape of
  `boot/tests/ui_setup_tests.rs`'s existing nav pin, extended to `seed_tab_property`.
  The mutation to check is moving the call down beside `seed_tab_shadow`.
- `…::a_tab_pick_clears_the_filter_on_both_sides`.
- `…::every_tab_carries_its_own_section_gate` — five `SectionActiveGate` mounts at
  `index: 3` with five distinct `tab-index:` values.
- `…::every_detail_records_the_tab_it_was_opened_from` — the three detail globals declare
  `origin-tab`, and every site writing `origin-nav-index` writes it too.
- `…::every_sort_pill_asks_for_a_field_the_comparator_knows` — the Albums / Artists /
  Genres rows finally get the pin the Favorites Artists row already has.
- `ui::library_tab_band_tests` — as listed in Phase 2.
- ☑ `ui::tab_bar::tests` extended, on a `LIBRARY_PAGES` array beside `CURATED_PAGES`:
  `every_library_count_starts_at_the_unfetched_sentinel` (declaration **and** the `>= 0`
  guard), `every_library_leave_rewinds_the_count_it_numbered`, and
  `the_section_gate_ignores_its_tab_predicate_when_a_section_has_none` (both new gate
  properties default `-1`, and the predicate keeps its negative short-circuit — a `0`
  default silently deactivates all nine sections).
- `ui::hero_chips::tests` — `HERO_VIEWS` at 2, plus the new `BAND_HOSTS`.
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
   back to the count row, with no backdrop pop at the seam.
5. Right-click a track → Go to Artist from the Songs tab: lands on the Artists tab with
   the detail open; from there open one of its albums; back twice returns Artists → Songs.
6. Mouse-4 / Mouse-5 walk the history across tabs and details.
7. Miniplayer → full-window swap while a detail is open: the band remounts at the right
   size and the tab bar isn't stuck icon-only.
8. Light theme (Latte) with a detail open: the tab bar's selected label and the back
   button read against the blur, and the idle pane's tab colours read against `mantle`.
9. Genre Detail specifically: gradient backdrop, no blur, tile colours intact through the
   morph in both directions.
10. `RUST_LOG=info MELODIA_RSS_SAMPLE=1` — the `view=` tag names the tab and the open
    detail, and idle RSS after walking all five tabs and back sits where it did before.
