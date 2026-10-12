# Plan: `FlexboxLayout` for the chip strips, the Settings columns, the Radio pills and the mosaic picker

Working doc. Delete when every step has shipped and passed by hand.

Status: **not started** · Owner: Kenan · Created: 2026-10-12

> **Validated 2026-10-12** against the Slint 1.18.1 sources in the local cargo registry
> (`i-slint-compiler-1.18.1/builtin_elements.rs`, `i-slint-core-1.18.1/layout.rs`, taffy 0.10.1)
> and the live tree on `refactor/upgrade-dependencies`.

Phase 3, item 1 of `docs/plans/SLINT_NATIVE_ADOPTION.md`. Steps 1–4 cover the chip strips (the tracker's item 1). Steps 5–7 are three further workarounds picked during planning, and they run after the chip strips land. There are three manual checkpoints: after step 1, after step 4, and after step 7.

## Context

**What you see today.** Five places wrap a row of chips onto more lines: the Now Playing metadata chips, the hero band's chips (four detail heroes, two mosaic heroes, the station detail), every Settings / onboarding / tag-editor `ChipGroup`, the two accent `ColorDotGrid`s, and Search's recent-search pills. None of them wraps at the width actually drawn:

- **Now Playing and the heroes** wrap from a guess. Rust charges every character 6.5 px (`ui::chips::CHAR_W`), so a label in wider glyphs over-packs its row, and on an over-packed row *every* chip elides, not just the one that didn't fit. A label in narrower glyphs wraps early instead.
- **Settings chip groups** size every row off the widest chip (a hidden ruler of real chips), so a row of short labels wraps long before it is full.
- **Recent searches** use the same 6.5 px guess.
- **Every resize drag** sends the strip's width to Rust on each pointer motion. Rust re-packs the rows and re-publishes a nested `[[string]]` model, with three caches whose only job is to skip a model reset when the shape didn't change.

**Why.** Slint 1.16 had no wrapping layout. 1.18.1 ships `FlexboxLayout` (verified in the registry sources, not behind any flag). It wraps by default, measures each item's real width, and reports the height it needs at the width its parent gives it (`layoutinfo-v` reads the flex's own `width`; `i-slint-core-1.18.1/layout.rs:2351`).

**Outcome.**
- Each strip wraps where its chips actually stop fitting.
- Unchanged: which chips appear and in what order, the hero's two-row limit (rows past the second still drop out whole), Now Playing's reserved row and centred alignment, and every click, selection and tooltip.
- Removed: Rust's estimate, the width channel, the `[[…]]` models, the `Wrap` global, the hidden ruler and the `avail-width` plumbing that existed only to feed the ruler's arithmetic.

## Decisions settled here

1. **The hero cap becomes a clip.** `MetaChipStrip` gains `in property <int> max-rows: 0` (zero means uncapped).
   - When it's set, the strip's height is capped at `max-rows` rows plus their gaps, with `clip: true`.
   - Chips are a uniform 26 px, so the third row disappears whole. That matches what `pack_rows` does today: it breaks lines greedily in the same way and drops the whole tail.
   - `HeroChipStrip` sets `max-rows: 2`. `HERO_MAX_ROWS`'s argument ("measured rather than picked: a second row fits the slack") moves to that binding.
   - The clip has no radius, so it's a scissor rather than a layer.
2. **`avail-width` goes, along with everything that fed it.** A flex wraps against the width its layout gives it, so nobody needs to tell it a width. This deletes:
   - `ChipGroup.avail-width` and `ColorDotGrid.avail-width`
   - `SettingsPage.row-content-w`, whose only readers are those two defaults
   - the tag editor's `tabs-w` (and its eight-line argument)
   - the onboarding panel's `content-w` input and the card's `content-w`
   - `RecentSearches.avail-width` and `RecentChip.width-cap`

   A lone long chip still shrinks to its line and elides, because flex shrinks an item that is alone on its line.
3. **`ColorDotGrid`'s first-row test reads the layout, not arithmetic.** `tip-side: cell.y < root.cell-h ? above : below` uses the cell's laid-out `y`, so it can't disagree with the flex's own line breaks. If the compiler flags a loop, the fallback is `i < floor((width + gap) / (32px + gap))` as a private function.
4. **Strip roots don't take surplus height.**
   - `ChipGroup` and `ColorDotGrid` keep a bound `height`, now `flow.preferred-height`, where it used to be `Wrap.rows-height`. A parent with surplus, such as the tag editor's fixed-height column, therefore can't spread their lines apart. The flex reports `max: MAX, stretch: 0`, and `cross-axis-line-alignment` defaults to `stretch`.
   - `MetaChipStrip` sets `cross-axis-line-alignment: start` for the same reason.
5. **Out of scope, but now possible** (listed, not done): the band's chip strip joining `hero-shown`, and `MetaChipStrip` inside onboarding (the strip's `changed watched-w` tracker blocked both). Also Search's Top Result and Songs stacking on a narrow panel, the Browse breadcrumb wrapping, and `SettingRow` dropping its control under the label (each a design change of its own).
6. **Not a fit for flex, checked:**
   - virtualized card grids (flex doesn't virtualize)
   - station card tags (fixed card height inside a `ListView`)
   - lyrics measuring (word wrap plus a Rust-owned offset table)
   - track columns (per-column widths)
   - the miniplayer (a continuous morph)
   - the tab and search header and the band pill rows (fixed band row height)
   - the centring wrappers (that's tracker item 4, `cross-axis-alignment` on box layouts)

## Structure (who owns what afterwards)

| File | Owns |
|---|---|
| `components/meta-chip-strip.slint` | The wrap (flex), the reserved row, the optional row cap and clip, alignment. No width channel, no Timer, no tracker. |
| `components/hero-chip-strip.slint` | Hero brushes, the fade, `max-rows: 2` and its argument. |
| `globals/hero-chips.slint` / `globals/player.slint` | A flat `chips: [string]` each. `recompute` / `recompute-chip-rows` are deleted. |
| `ui/hero_chips.rs` | Which chips a hero has, and who published them (`owner`). `PublishedChips` shrinks to the owner; `install`, `write_rows` and `HERO_MAX_ROWS` go. |
| `ui/now_playing/source_change.rs` | `publish_chips` writes `visible_chip_texts(&meta)` as one flat model. |
| `components/settings/chip-group.slint`, `color-dot-grid.slint` | One `FlexboxLayout` over `options` / `colors`, iterated with their own index (`for option[idx]`, `for color[i]`, keeping `i` for `window_border_tests`). |
| `views/search/recent-searches.slint` | A flex over `Search.recent-rows`. |

Deleted outright: `globals/wrap.slint` (and its import and export in `app-window.slint`), `ui/chips.rs`, `ui/tests/chips_tests.rs`, `pub mod chips` in `ui/mod.rs`, and the `chips::install` / `hero_chips::install` calls in `crates/melodia/src/boot/ui_setup/views.rs`. All of these files shrink; none comes near the 800-line cap.

## Steps

**Step 0: the tracker.** Shrink `docs/plans/SLINT_NATIVE_ADOPTION.md`'s `FlexboxLayout` section to a pointer at this doc, so the decisions live in one place. Also fix its stale `sidebar.slint:186` line (the third duration-zero gate is at `:187`).

**Step 1: Settings strips (the prototype), then stop for your look.**
- `chip-group.slint`:
  - Delete the ruler, `chip-w`, `per-row` and `avail-width`.
  - Root becomes `Rectangle { min-width: 0px; height: flow.preferred-height; flow := FlexboxLayout { spacing: Theme.pad-sm; for option[idx] in root.options: Chip { … } } }`.
  - Rewrite the "Slint 1.16 has no Flow" header.
- `color-dot-grid.slint`: the same shape. Each cell is `cell := VerticalLayout { height: root.cell-h; alignment: center; ColorDot { … tip-side: … } }`, which keeps today's 36 px cell and 8 px gap.
- Delete the `avail-width` chain from decision 2. That touches:
  - `settings-page.slint` (`row-content-w` goes; the `page-w` seed note loses its chip clause, and `row-icon-w`'s note loses `row-content-w`)
  - `setting-row-stacked.slint:32` (comment)
  - `tag-editor-body.slint` (`tabs-w`)
  - `appearance-panel.slint` (`content-w` input, its four passes, and the header note)
  - `onboarding-overlay.slint` (the card's `content-w` and its pass)
- `wrap.slint` loses `chunk-indices`, `per-row` and `rows-height`. `chips.rs` loses `chunk_indices`, `IndexRows` and `IndexRowsModel`, and the `on_chunk_indices` wiring. `chips_tests.rs` loses its 14 index tests.
- Run the static gates, then stop. This is where a height-for-width mistake shows up first: a card too tall, too short, or lines spread apart.

**Step 2: Recent searches.**
- Replace the packed rows with `FlexboxLayout { spacing: Theme.pad-sm; for t in Search.recent-rows: RecentChip { … } }`. Drop `avail-width`, `chip-chrome-w`'s pass-through of `width-cap`, and the `search-view.slint:82` binding.
- `Wrap` is then empty: delete `wrap.slint` and its two lines in `app-window.slint`, plus `PackedLabels`, `Packed` and `install`'s `on_pack_labels`.
- `search/fetch.rs::push_recent_rows_to_slint` moves to the tree's `set_vec` on the model `search/mod.rs` seeds. Its paragraph arguing for a fresh model existed only for the `pure callback`.

**Step 3: heroes.**
- `meta-chip-strip.slint`:
  - `in property <[string]> chips` replaces `rows`.
  - Add `max-rows`.
  - Delete `measured`, `watched-w` and the Timer.
  - `rows-box := VerticalLayout { if root.show-rows: FlexboxLayout { spacing-horizontal: Theme.pad-sm; spacing-vertical: Theme.pad-xs; alignment: centred ? center : start; cross-axis-line-alignment: start; for t in root.chips: MetaChip { … } } }`.
  - `min-height` / `max-height` get capped when `max-rows > 0`.
  - Re-derive the `min-height` / `min-width: 0px` notes against the flex rather than copying them.
- `hero-chip-strip.slint`: `chips: HeroChips.chips`, `max-rows: 2`, and `arrive-t: root.chips.length > 0`. Drop `measured`, and drop the tracker clause from the `fade` note.
- `hero-chips.slint`: `rows` becomes `chips: [string]`, and `recompute` goes.
- `hero_chips.rs`:
  - `publish` and `clear` set the owner and write `ModelRc::from(Rc::new(VecModel::from(chips)))`.
  - `PUBLISHED` becomes the owner alone.
  - Delete `install`, `write_rows`, `HERO_MAX_ROWS` and the `chips` import.
  - Rewrite module doc lines 3–5 and 25–26 to point at the strip's two-row clip.
  - `hero_backdrop_tests` pins `publish`'s signature, so it stays.

**Step 4: Now Playing.**
- `player.slint`: `chip-rows` becomes `chips: [string]`, and `recompute-chip-rows` goes.
- `now-playing-view.slint`: the mount binds `chips: Player.chips` and drops `measured`. Rewrite the comment's "Rust re-chunking" line.
- `now_playing/mod.rs`: delete `chip_texts`, `chip_last_width`, `chip_last_shape`, their initialisers, `wire_recompute_chip_rows` and its call.
- `source_change.rs::publish_chips(player, meta)` becomes two writes, and its `np_state` parameter goes.
- `metadata.rs`: fix the two comments that point at `ui::chips`.
- `chips.rs` is now empty: delete it, `chips_tests.rs`, `pub mod chips` and `ui::chips::install(app)`.

Run the static gates, then stop for your manual pass of steps 2–4.

### Beyond the chip strips (the three picked during planning)

**Step 5: Settings body columns, then stop for your look.**

- **What you see today:** at a wide window two columns of cards sit side by side. Narrower, they stack, the second column under the first. Below 800 px the column shrinks with the panel.
- **What keeps working:** the same threshold (two 800 px columns plus their gap), the same masonry (each column flows on its own), every card mounted once (`has-matches` intact), and search staying single-column.
- **What changes underneath:**
  1. A new `components/settings/settings-columns.slint` holds the page shape once:
     - `SettingsColumns inherits FlexboxLayout` (`alignment: center`, `cross-axis-alignment: start`, `cross-axis-line-alignment: start`, `spacing: Theme.pad-lg`, `@children`)
     - `SettingsColumn inherits VerticalLayout` (`preferred-width` and `max-width` at `SettingsPage.card-cap`, `alignment: start`, `spacing: Theme.pad-lg`, `@children`)
     - It carries the masonry / contiguous-halves argument that `library-page.slint`'s header holds today, so the five pages stop restating it.
     - If a component can't inherit `FlexboxLayout`, the fallback is a `Rectangle` root with the flex inside.
  2. Each of the five `views/settings/pages/*.slint` swaps `GridLayout` for `SettingsColumns` and its two column `VerticalLayout`s for `SettingsColumn`, losing every `row:` / `col:` line.
  3. `settings-view.slint`:
     - The body's cap becomes `max-width: root.searching ? SettingsPage.card-cap : 2 * SettingsPage.card-cap + Theme.pad-lg`. A one-column cap in search leaves no room for two 800 px columns on a line, so search stacks with no direction switch.
     - Delete `two-col`, `two-col-latched`, `watched-two-col` and its handler, the `SettingsPage.page-w` / `body-cols` writes in `changed width` and in the Timer, and the `content-w` note about `body-avail`.
     - The local `page-w` mirror and its Timer stay, because the header row still needs them.
  4. `globals/settings-page.slint`: delete `page-w`, `body-cols`, `body-w`, `body-avail`, `card-w`, `grid-row` and `grid-col`. `card-cap` and `row-icon-w` stay.
  5. `section-card.slint`: delete `card-width` (nothing overrides it) and its `preferred-width` / `max-width` pins, since the column now owns the width. Rewrite its note.
- **Two behaviour changes worth knowing:**
  - The 24 px latch goes, because flex has no hysteresis. A slow drag right across the threshold can flip the second column back and forth.
  - The flip no longer jumps the page to the top. Slint's own out-of-bounds correction still applies, since `sv.content-y` carries no binding.
- **Tests** (`settings_page_tests.rs`):
  - `the_two_column_flip_reserves_two_full_cards` is re-derived to pin the column basis at `card-cap` and the body cap at two columns plus the gap.
  - `every_column_takes_its_own_cell` and `grid_indices` are deleted, along with the mechanism they pin.
  - `the_page_width_seed_is_the_rows_floor` is untouched.
- **Risk:** this nests a flex inside a flex (the chip strips inside a column inside the page), all height-for-width. Step 1 has already proven the inner half by then.

**Step 6: Radio Browse suggestion pills.**
- `views/radio/browse-tab.slint:48`: the `HorizontalLayout` becomes `FlexboxLayout`, keeping the paddings, with `spacing: Theme.pad-sm` both ways (the `ChipGroup` gap, since a wrapped second line is new). Drop the "Rust caps the row at what one line holds" note.
- `suggest.rs`:
  - Delete `MAX_SUGGESTIONS` and the `truncate`. The list is bounded at five by construction (one shape plus one per facet list).
  - Rewrite "it is the cap that makes that matter" and "the row has four slots" as reading-order rules.
- `the_cap_drops_a_guess_before_it_drops_a_shape` becomes the shape-leads ordering check over all five.
- **What you'd see:** on a narrow window the pills wrap onto a second line instead of eliding, and the rare five-match needle shows its fifth pill.

**Step 7: Playlist mosaic picker grid.**
- `playlist-mosaic-picker.slint`:
  - Delete `cand-cols`, `cand-stride`, `grid-row-h`, `total-rows`, `grid-content-h`, the fixed-height inner `Rectangle`, the per-tile `x` / `y`, and the hand-set `content-height`.
  - `cand-sv`'s direct child becomes `VerticalLayout { padding: Theme.pad-xs; FlexboxLayout { spacing: root.cand-gap; cross-axis-line-alignment: start; for path[i] in Dialog.mosaic-candidates: cand := Rectangle { width: root.cand-tile; height: root.cand-tile; … } } }`, so the scroll height follows the layout. The existing `OverlayScrollbar` mount is unchanged.
  - Drop the "Slint has no `flex-wrap`" note.
- **What you'd see:** six columns at the dialog's usual width, and five on a narrow window instead of a clipped sixth.

Run the static gates after steps 5–7, then stop for your manual pass.

**Stale prose in the blast radius** (code comments, written with each step): `onboarding-overlay.slint:16` drops `MetaChipStrip` from the tracker ban list, `station-card.slint:13-15` drops its tracker reason, and `crates/melodia/tests/onboarding.rs:74` gets its comment fixed.

**Existing tests the change breaks**, updated in the same turn:
- `hero_chips_tests.rs`:
  - re-anchor `every_chip_strip_takes_its_brushes_from_its_backdrop` (it cuts at `measured(`)
  - delete `the_strip_reports_its_width_on_resize_and_at_mount` (it pins the deleted channel)
  - re-derive `the_strip_rows_hang_off_a_layout_not_off_the_root` to `VerticalLayout { if root.show-rows: FlexboxLayout`
  - rewrite `the_strip_spaces_its_rows_tighter_than_its_chips` against `spacing-vertical` / `spacing-horizontal`
  - update the `HERO_MAX_ROWS` mentions in messages to name `max-rows`
- `library_tab_band_tests.rs`:
  - re-pin `arrive-t` to `chips`
  - in `the_chip_strip_outlives_every_morph_it_is_painted_in`, drop the no-`if`-encloses half, whose only premise was the tracker, and keep the `hero-shown` gate half. **Open: Kenan to confirm before step 3; keep that half standing if he'd rather.**

No new tests: those wait for your go.

**Docs pass, after your manual pass and your go:**
- `.claude/rules/slint-pitfalls.md`:
  - the repeater entry's `IndexRows` / `PackedLabels` example
  - the `MetaChipStrip` conditional-child example
  - the `estimated_chip_width` clause
  - the tracker example and its test reference
  - the "no `Flow` element" line
  - possibly a new entry: flex lines stretch by default, and there is no `clip` on a `FlexboxLayout`
- `.claude/rules/ui-patterns.md`: the `HeroChipStrip` block, the "chip strip cannot join" line, "the wrapping strips wrap through Rust", and the `row-content-w` paragraph.
- `.claude/rules/visualizer.md:75-77`.
- For the Settings body:
  - `ui-patterns.md`'s "one cell per column", `card-cap` and `row-content-w` paragraphs
  - `slint-pitfalls.md`'s masonry and run-time `row`/`col` entries (still true of `GridLayout`, but their worked example moves)
  - CLAUDE.md's "two-column flip stays derived from `card-cap`" line
- Tick the tracker's item 1, plus the three additions.

## Verification

**Static** (every step):
- `cargo fmt --all --check`
- `cargo clippy --all-targets --locked --workspace -- -D warnings` (Slint's compile-time binding-loop check runs here too)
- `cargo test --locked --workspace`, covering `hero_chips_tests`, `library_tab_band_tests`, `window_border_tests`, `translations`, `onboarding`, `tooltip_mounts` and `scrollbars`. No `@tr` changes, and the template records file names only, so the catalogs don't move.

**Manual** (Kenan's; Claude doesn't launch the app):
- After step 1:
  - Settings at a narrow and a wide window: every chip group and both accent grids wrap where they stop fitting, cards hug their content, and lines aren't spread apart.
  - Accent tooltips: above on the first row, below on later rows.
  - Settings search (all five pages mounted).
  - Tag editor tabs for a single track and for several, in a long locale.
  - Onboarding step 2, re-opened twice from Settings ▸ About.
- After step 4:
  - Recent searches: add past the cap, remove, clear all; a very long term elides on a line of its own.
  - Album, artist, playlist and station heroes at a narrow window: two rows at most, the third gone whole, band height unchanged, and the open/close morph still fades the chips.
  - Favorites and Recently Played mosaic heroes.
  - Now Playing: chips centred, wrap uncapped, artwork doesn't reflow on a track change, a stale fetch, or a track with no metadata.
  - Resize drags across all of the above.
  - One wide-glyph locale (el or vi) for the estimate fix.
- After step 7:
  - Settings on every tab:
    - Two columns appear at the same width as today.
    - Narrower, the second column sits under the first.
    - Below 800 px the column shrinks with the panel.
    - Searching stacks everything in one column at any width.
    - A slow drag across the threshold, to judge the lost latch.
  - Radio Browse: type a needle that matches several facets at a narrow window, and the pills wrap.
  - Playlist artwork picker: a playlist with more than 12 tracks scrolls, six columns at full width, five at a narrow window, and selection is unchanged.
