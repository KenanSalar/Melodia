# Plan: `cross-axis-alignment` retires the centring wrappers

Working doc. Delete when every phase has shipped and passed by hand.

Status: **phase 1 done, phase 2 next** · Owner: Kenan · Created: 2026-10-12

> **Validated 2026-10-12** against the Slint 1.18.1 sources in the local cargo registry
> (`i-slint-compiler-1.18.1/passes/{lower_layout,default_geometry}.rs`,
> `i-slint-compiler-1.18.1/llr/lower_layout_expression.rs`, `i-slint-core-1.18.1/layout.rs`,
> `i-slint-core-1.18.1/items/text.rs`) and the live tree on `refactor/upgrade-dependencies`.

Phase 3, item 4 of `docs/plans/SLINT_NATIVE_ADOPTION.md`. Three phases, each validating alone,
with a manual checkpoint after each.

## Context

**What you see.** Nothing moves. Every icon, label, pill and button stays where it is today, at
every window width, theme and locale. The two exceptions are flagged below; each fixes a
misalignment that is already there.

**What goes is source.** A fixed-size child of a box layout is centred on the cross axis by a
second layout wrapped round it, e.g. `VerticalLayout { alignment: center; Icon {} }` inside a row.
A sweep of the whole tree found the idiom in nearly every shared row, dialog and page. Slint
1.17/1.18 added `cross-axis-alignment` on the layout and `cross-axis-self-alignment` on a child, so
the wrappers that exist only for centring can go.

**The engine facts that decide every call**, all read in the 1.18.1 sources:

- **Plain stretch path.** With neither property set, a cell is stretched over the cross axis and
  clamped only to its *explicit* min/max (`clamp_cross_stretch_size`, `passes/lower_layout.rs:1515`).
  A cell with a fixed cross size sits at the start edge. This is the original pitfall.
- **Ortho solve.** Setting either property moves *every* cell of that layout onto
  `solve_box_layout_ortho` (`lower_layout.rs:1594-1604`, `i-slint-core layout.rs:1355`).
  - A centred cell gets its preferred size, capped by its max and 100 % of the space, so a
    word-wrapped child still wraps. A wrapping child is measured at the width it is laid out at.
  - A cell left on stretch is now also clamped by its *implicit* min/max.
  - The layout writes every cell's position, so an explicit `x`/`y` on a cell becomes a build error
    ("cannot be set for elements placed in this layout", `lower_layout.rs:2277-2281`). That is loud,
    not silent.
- **The cap.** Layout info ignores alignment (`box_layout_info_ortho`, `layout.rs:1423`). A wrapper
  with non-stretch `alignment` reports an unbounded max (`box_layout_info`, `layout.rs:1413-1417`).
  Without it, the child's fixed size becomes the parent's max on that axis.
- **Builtins are unbounded.** `Text` and `TextInput` report no vertical max (`items/text.rs:843`),
  so as stretched siblings they are unaffected by the ortho solve.
- **Plain parents centre for you.** A sized child outside any layout is centred by
  `default_geometry.rs:164-173`.
- **No speed to gain.** Single-cell wrappers were already folded at compile time
  (`optimize_single_cell_layouts`, `lower_layout.rs:1143`), so this is a clarity change only.

## The rules (applied per site)

- **R1: every child centres.** Every child of the parent is a retiring wrapper, has a fixed cross
  size, or is an invisible spacer.
  - Drop the wrappers and put `cross-axis-alignment: center` on the parent.
  - A wrapper's `*-stretch`, `if` or `z` moves onto its child.
  - A kept stack's own `alignment: center` becomes dead and goes.
- **R2: mixed parent.** Siblings must keep stretching.
  - Put `cross-axis-self-alignment: center` on each fixed-size child that was wrapped.
  - Only where every stretched sibling's implicit cross max is unbounded: a builtin, a `Rectangle`
    whose children are all `if`, or a layout whose own cells are all unbounded. Otherwise keep the
    wrapper (K4).
- **R3: lone `Text` in a parent that isn't R1.** The `Text` takes its own `vertical-alignment` or
  `horizontal-alignment` and keeps stretching. The parent stays on the plain stretch path.
- **R4: explicit `y:` centring a layout cell** (it only works while the parent has no cross
  padding). It becomes `cross-axis-self-alignment: center`, and is forced anyway once the parent
  goes ortho.
- **R5: a both-ways pair in a plain `Rectangle`** around one sized child goes whole, and
  default geometry centres the child. The same applies to `PillButton`'s and `PillDivider`'s
  single centring layouts, whose header note argues the pitfall this item retires.
- **Cap check, every site.** Where the wrapped child is fixed-size and the parent's cross size comes
  from a host that could hand it more (a stretching column with surplus, or a host on the ortho
  solve), confirm that host has no surplus for it; otherwise keep the wrapper.
  - Rows with an explicit cross size pass trivially.
  - Hosts to check: `ScanProgress` (music panel, Library card), `DialogToggleRow` (its three other
    `@children` hosts), the NP header row, the browse header row, the library-section row.

**Kept, each for a reason 1.18 doesn't touch:**

- **K1: `alignment: start` wrappers round fixed children.** Positionally they're no-ops; they exist
  for the unbounded max. Sites:
  - the Favorites/Recently Played pill rows
  - Artist Detail's Albums header
  - the two heroes' artwork columns
  - the dialog "Add" rows (`name-list-editor`, `smart-playlist-editor-body`, `tag-editor-body`)
  - `StarRating`'s root
- **K2: the wrapper carries padding, or holds a slot for an `if` inside it.** Sites:
  - NP visualizer strip (`padding-top`)
  - "Searching…" and Show more (`padding-top`)
  - queue row title (`padding-right`)
  - bar quality chip (`padding-right`)
  - `TabSearchHeader`'s search cell (`padding-bottom`)
  - the onboarding and stacked-row icons (`start` + `padding-top`)
  - the check slots in `FlyoutOption` and `DropdownOption`
- **K3: real groups.** These lay out or swap several children. Sites:
  - the bar's transport/seek/trailing columns
  - `MiniTimeline`'s seek column
  - the band title row
  - the breadcrumb trail root
  - the queue header's title/count stack
  - the five text stacks in settings and dialog rows
- **K4: a stretched sibling would start answering to a bounded max.** The queue row's artwork and
  remove button: its clipped duration cell reports a 26 px max while the heart shows.
- **K5: wraps an `@children` slot the component can't name.** `OnboardingFeatureRow`'s control
  slot.

## Phase 1: shared chrome and rows (one edit reaches every mount)

Status: **done 2026-10-13, passed by hand.** Done as listed below, with three calls made along the
way:

- The note at 60-61 in `now-playing-bar.slint` lost its "the sidebar's row pattern" tail, since the
  sidebar rows are no longer that pattern.
- The bar's quality-chip wrapper is K2 (`padding-right`). It stays, minus its dead `alignment`.
- A `Text` that already set `vertical-alignment: center` keeps it. Centred at its preferred height
  it is inert, but it still holds if a row ever hands the line box less than that.

- **`components/action-pill.slint`**
  - R5: `chrome` and the divider bar sit straight in their 32 px roots.
  - R1 on `layout` (icon, label, sort arrow).
  - Rewrite the header note (lines 8-10) and the per-glyph notes that cite "plain centring".
- **R1 rows, plain conversions:**
  - `context-menu.slint` `MenuItem`
  - `sort-menu-popup.slint` `SortMenuRow`
  - `quality-chip.slint`
  - `custom-titlebar.slint` (mark + title)
  - `collapsible-section-header.slint`
  - `grid/play-count-badge.slint`
  - `track-list/column-toggle-popup.slint`
  - `track-list/track-list-header.slint`
  - `now-playing/overflow-row.slint`
  - `now-playing/sleep-timer-flyout.slint`
  - the `rating-row` in `now-playing/overflow-menu.slint`
  - `now-playing/lyrics-controls.slint`
- **R1 rows with extra work:**
  - `grid/grid-empty-state.slint` (icon, title, body, action).
  - `track-list/track-list-row.slint`: R1 on the title cell (the stretch moves to the `Text`;
    retune the note at 355-359). R1 + R4 on the length cell (the heart loses its `y:`).
  - `layout/sidebar.slint`: R1 on both rows.
  - `layout/now-playing-bar.slint`:
    - R1 on the track-info row (the text stack takes the stretch) and on the transport and
      trailing button rows.
    - R1 on the station row in the seek column.
    - The three columns stay (K3/K4). Keep the note at 60-61.
- **R2 and R3 sites:**
  - `search-bar.slint`: R2 on the magnifier, R4 on the clear button. The field keeps its full
    height, being a `TextInput`.
  - `meta-chip.slint`: R3.
  - `tab-bar.slint` `TabBarCell`: R2 on the icon box. The clipped label keeps stretching.
  - `now-playing/up-next-list.slint` `UpNextRow`: R2 on the artwork, R3 on the duration. The
    clipped title column keeps stretching, so marks past the line box survive.
  - `now-playing/flyout-option.slint`: R3 on the label. The check slot is K2.
- **R5 sites:** `now-playing/play-button.slint` (the glyph takes `visible:`),
  `now-playing/now-playing-cover.slint` and `hero/mosaic-hero-tile.slint`.
- **Other:**
  - `now-playing/np-empty-state.slint`: R1 on the outer column (the grid centres) and on the
    glyph cell.
  - `hero/library-tab-band.slint`: R2 on the title badge.
  - `mini-player/mini-controls.slint`:
    - R1 on `MiniTransport` and `MiniTrailingControls`.
    - On `MiniTimeline`: R1 on the root (shuffle takes `z: 1`; the seek column keeps its
      stretch), and R2 on the live chip inside the seek column.
  - `mini-player/mini-captions.slint`: R1 on `MiniCaptionsRow` (the spacer goes to 0 height). Keep
    the 24-26 note.
  - `station-facts.slint` `DetailRow`: R1. **Visible:** the 28 px action button moves down to the
    row's centre line. Today it sits 3 px high, beside centred text in a row pinned to 34 px.

- [x] Static gates (2026-10-13). fmt, clippy and `cargo test` are clean, and no test needed
      changing. In the generated tree, every converted component solves on the ortho path. The
      search field's cell there reports an unbounded max at stretch 1, so it keeps its height.
- [x] Manual pass (2026-10-13). Every row read as before.

## Phase 2: settings, dialogs, onboarding

Status: **not started**

- **`components/settings/`:**
  - R1: `folder-row.slint`, `section-button.slint` (its `if`s move onto the glyph and image),
    `setting-slider.slint`, `scan-progress.slint`, and `dropdown.slint`'s trigger.
  - R3: `dropdown.slint` `DropdownOption` label.
  - R2: `setting-row.slint` icon. The text stack and the scrimmed control box keep stretching.
- **`components/dialog/`, R1:**
  - `preamp-slider-row.slint`, `replaygain-body.slint`.
  - `dialog-toggle-row.slint`. Equalizer's docked dropdown and Reset lose their wrappers too,
    being cells of that root.
  - `name-list-editor.slint`'s row, `credit-editor.slint`'s row and its pill/preview row.
  - Both `selectable-picker.slint` rows.
  - `notification-stack.slint`'s card row. The message stack's centring note moves to the row.
  - `export-playlist-picker.slint`'s empty state.
- **`components/dialog/`, other:**
  - `playlist-mosaic-picker.slint`: R2 on the 220 px preview; R1 on the empty preview, the dot
    row and the hint; the hint's wrapper simply goes, since its `Text` already centres its lines.
  - `smart-playlist-editor-body.slint`: `RowLabel` becomes `inherits Text`. Its doc comment goes,
    the two label rows take R1, and the flex mount already centres.
- **`views/settings/`:**
  - `library-section.slint`: R1 on the manage row, R2 on the empty-state glyph.
  - `overflow-menu-section.slint` `OverflowCheckCell`: R1.
  - `update-section.slint`: R3 (`horizontal-alignment: right`).

- [ ] Static gates
- [ ] Manual pass

## Phase 3: pages

Status: **not started**

- **`views/now-playing-view.slint`:** R1 on the header row. The strip stays (K2).
- **Search:**
  - `views/search-view.slint`: R1 on the search-bar row.
  - `search/top-result-card.slint`: the heading pair becomes one `Text { height: 32px;
    vertical-alignment: center; }`.
  - `search/songs-section.slint`: R3 on "Songs".
  - `search/no-results.slint`: R1 on the root.
  - `search/recent-searches.slint`: R1 on the header row.
- **Browse:**
  - `browse/folder-row.slint`: R1.
  - `browse/breadcrumb-bar.slint`: R3 on the pill label, R1 on each segment row.
  - `browse-view.slint`: R2 on back, home and the view pill. The breadcrumb keeps stretching.
- **Queue:**
  - `views/queue-sheet.slint`: R1 on the header; empty state and drop overlay flatten to one
    `VerticalLayout { alignment: center; cross-axis-alignment: center; … }` each.
  - `views/queue-row-item.slint`: R1 + R4 on the duration cell, R5 on the number and checkbox
    cells. Artwork and remove stay (K4); the title stays (K2).
- **Radio:**
  - `radio/suggestion-pill.slint`: R1.
  - `radio/facet-chip.slint`: R1 on both rows.
  - `radio/station-detail.slint` `StatusLine`: R1. **Visible:** the status glyph moves onto the
    text's centre line, which its note (24-25) says the wrapper was for. Today the glyph sits high
    of it, the wrapper being round the taller `Text`.
- **`my-library/playlist-detail.slint`:** R1 on the drop banner.

- [ ] Static gates
- [ ] Manual pass

## Tests the change breaks (fixed with it)

- `radio/tests/facets_tests.rs:151` reads `if root.count > 0: VerticalLayout {`. Its subject is
  an `if` rather than `visible`, so the needle becomes `if root.count > 0: Text {`.
- Re-check `caption_tests.rs`'s `row_layout_body` (mini captions row) and `settings_page_tests.rs`'
  card-child needles against the edited files. The gate decides.
- Translations: no `@tr` changes. The template records file-only locations and source order is
  kept, so no catalog moves.

## Verification

- **Per phase, the static gates:**
  - `cargo fmt --all --check`
  - `cargo clippy --all-targets --locked --workspace -- -D warnings`. This compiles the Slint tree,
    so a stray `y:` on an ortho cell fails here.
  - `cargo test --locked --workspace`
- **Ground truth:** in the newest `target/debug/build/melodia-ui-*/out/app-window.rs`, every
  converted component gains a `solve_box_layout_ortho`, and no K4 layout does.
- **Manual pass per phase.** Check that rows are unchanged at the narrowest and widest windows:
  - Phase 1:
    - Pills with and without sort arrows, and right-click and sort menus.
    - Search bar typing and clearing; tab bar compact and full.
    - Titlebar; sidebar collapsed, expanded and drag-resized.
    - The bar across widths, with buttons moved into overflow, and with a station.
    - Track rows: hover heart and stars, RTL title elide. Header sort chevron.
    - Up Next, lyrics controls, overflow and sleep flyouts.
    - Empty states and the play-count badge.
    - Miniplayer at min and max size, with a station; KDE and macOS-style captions.
    - Station facts (the moved button).
  - Phase 2:
    - Every Settings card at the narrowest width, especially the wrapping manage row and slider
      rows.
    - Dropdowns; equalizer and ReplayGain; tag editor names, credits and lyrics.
    - Export and Add-to-Playlist pickers; mosaic picker; smart editor; toasts with an action.
    - Welcome card.
  - Phase 3:
    - NP header with and without a station; Search (results, none, recent, show more); Browse
      header and breadcrumbs.
    - Queue sheet: empty, drop hover, row hover, selection.
    - Radio suggestion and facet pills, the station status line (moved glyph).
    - Playlist drop banner.
- **Once all three manual passes are done,** the docs:
  - Rewrite `.claude/rules/slint-pitfalls.md:107-108` into the entry the engine facts above make.
    It covers the cross-axis property, the ortho solve clamping stretched siblings to implicit
    min/max, the explicit `x`/`y` build error, and the cap that a wrapper used to hide.
  - Mark item 4 done in `docs/plans/SLINT_NATIVE_ADOPTION.md`, with its outcome under the section.
