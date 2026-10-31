---
paths:
  - crates/melodia-ui/ui/**/*.slint
  - crates/melodia-views/src/ui/**/*.rs
---

# UI patterns: what already exists

Reach for something here before building a second one. Each component's own header argues its
shape; this file is the catalogue plus the contracts no one file can hold. What *builds, looks
right, and is wrong* is `slint-pitfalls.md`.

## Catalogue

Paths are under `crates/melodia-ui/ui/components/` unless stated.

- **Overlays.** `Tooltip` (an in-tree pill) and `TooltipFrame` (its top-layer mount, for a host
  Slint paints before its occluder). `PopupSurface` wraps every `PopupWindow` body, `MenuSurface` a
  Now Playing menu's; `PopupFit` and `PopupColumn` place and scroll one whose rows can outgrow the
  window. `FocusLossWatcher` goes inside every popup, `PopupDismissCatcher` over a popup's empty
  regions, never a bare `TouchArea`.
- **Menus.** `context-menu.slint` holds every right-click menu's parts (`ContextMenuPopup`,
  `MenuItem`, `FavoriteMenuItem`, `MenuFlyout`, `CopyField`, `ContextClick`,
  `ContextMenuMetrics`); `track-list/track-context-menu.slint` is the row menu; `RatingMenuRow` is
  the rating entry in it and in the bar's overflow menu.
- **Pills and chips.** `ActionPill` holding `PillButton`s and `PillDivider`s, `IconButton` outside
  a chip, `SortPillRow` for a row of sort pills. `MetaChip`/`MetaChipStrip` state a fact,
  `FilledChip` is the clickable pill, `QualityChip` the bar's verdict over `GradeDot`.
- **Inputs.** `FieldFrame` under `LabeledInput`, `ControlledInput` and `MultilineInput`;
  `RemoveRowButton` closes an editor's row; `dialog/selectable-picker.slint` is the toolkit both
  picker dialogs wire up. `HoldTouchArea` for a press whose hold differs from its click,
  `SliderPointer` under every slider, `EdgeResizeHandle` for a resizable edge.
- **Grids.** `EntityCard` inside `grid/card-cell.slint`'s `CardCell`, in a chunked `ListView` sized
  by `GridGeometry` and `GridColumnsSync`. `HorizontalCardStrip` only for a capped carousel,
  `EntityCardGrid` otherwise. `CardFavoriteButton` is a card's heart, `grid/play-count-badge.slint`
  its count, `GridEmptyState` the empty block for grids and lists alike.
- **Lists.** `TrackList` with `TrackListScrollbars`; `TrackTitle` for any row showing a track's
  title; `DragSource`, `ListDropTarget` and `DragGhost` for an in-window drag; `WindowRectReport`
  and `FileDropBanner` for a file dragged in from the OS (`window_chrome::file_drop` has the route).
- **Heroes.** `TabSearchHeader` (the only `TabBar` mount), `MosaicTabHero` and `LibraryTabBand`,
  `MosaicHeroTile`, `HeroChipStrip`, and `HeroBackdropStack` (bands) / `NpBackdropStack` (Now
  Playing, miniplayer) over `HeroBlurBackdrop` and `AuroraBackdrop`.
- **Navigation.** `ViewTransition` around every page's mount, `SectionActiveGate` for a section's
  visibility, mounted on the `AppWindow` root in nav-index order.
- **Rounded edges.** `EdgeOutline` strokes inside a rounded clip; `RoundedFrame` rounds a panel
  without one, only over an opaque fill of exactly its `ground`; `TuckedFill` fills under a host's
  own rounded stroke.
- **Settings.** `SectionCard`, `SettingRow`, `ChipGroup` for a short preset set and `ChoiceList`
  for a long one, `ColorDotGrid`, `SettingSlider`, `PreampSlider`, and per effect a
  `settings/*-controls.slint` plus a `*Text` global shared by its card and its dialog.

## Contracts

- **A pill landing on a later-declared sibling is lifted with a constant `z`** at the level where
  the two are siblings, `z` keeping a layout's order.
- **A multi-selection grows no pill; the right-click menu carries it**: the count, every batch
  action, "Select all" wherever the host has a set and "Clear selection" wherever it holds one,
  whether or not the clicked item is in it.
- **Escape and a section leave drop a selection**, an unseen set being indistinguishable from none.
- **A floating surface edges with `Theme.surface2`, never `Theme.border`**, and a control resting
  on a card fills with `Theme.card-control` / `card-control-hover`. Card-only controls default to
  the pair; `FilledChip`, `SliderTrack` and `SearchBar` (`on-card`) take it from a card host.
- **Sorting** goes through `next_sort`, `persist_view_sort` and `persisted_sort`, and a view whose
  natural order is a synthetic field (`"position"`) through `next_sort_with_natural`. Where the
  order *is* the page, `sortable: false` instead.
- **Anything painting on a hero reads `HeroBackdrop`, never `Theme.*`.**
- **`ActionPill` and `SearchBar` inside a hero keep `Theme.floating-chrome-bg`**, safe only because
  the backdrop is pinned; a translucent placeholder's fill has to be a hero token.
- **A row's play replaces the queue with the view's displayed ids, and there is no single-track
  path** and no Play All pill; Shuffle is the one pill, and appending is the menu's.
- **Track-list insets belong to the list**: pages mount `TrackList` full width with no side padding.
- **A raise that crosses an `await` owes a `DialogClaim`, and a deferred raise is an opener too**,
  single caller or not: inline chrome bumps nothing. `dialog_openers.rs` walks only the kinds it
  lists, so a new dialog is on its own.
- **A popup host with flyouts closes through one `dismiss()` function**, every path it can reach
  calling it: a function rather than a callback, since a Rust handler would replace its body.
- **Notifications** route through `ui::shell::notifications::run_action` by kind, and Undo through
  `ui::undo`. **A new removal offers its undo through `ui::undo` rather than asking first**,
  keeping a confirmation only where undo doesn't cover it.
- **A leave shrinks the grid tier to proxies** (`hand_back_covers`), and a prewarm is guarded ahead
  of its decode, on `section_active()` or `SectionState::grid_on_screen`.
- **`ShortcutScope` is the only `FocusScope`**; put new keys there. **A `changed` mirror on each
  view-mounting branch selector regrabs focus**, Slint not handing it back when the focused item is
  destroyed; one unmounting a composite view also owes `CompositeScroll.reset()`.
- **A new detail view** is a `view_id::*` const, `set_last_detail_id` on open and close, and a seed
  fn.

## Where to reach

- **Grids.** The seven library grids are `paged_rows::grid::PagedGrid`s loaded only through
  `ui::card_grid`, each a `CardGrid` stating its query and the facts beside its cards; a grid over
  another card type implements `CardGrid`. A grid that builds its cards (Browse, Radio's three)
  chunks them through `ui::grid_rows::chunk_built_rows` into `model_diff::replace_rows`.
- **A list that can hold the library is `ui::paged_rows::PagedRows`, never a `VecModel` copy.** The
  library-sized Songs tabs and the four details are `track_rows::TrackRows`, loading through
  `PagedTrackList`.
- **A drag lifting a pick lifts every pick** (`picked-run()`), and every tier moves through
  `melodia_core::utils::block_move`.
- **Shared wiring.** The four details go through `callbacks::track_detail`, the four grid tabs
  through `callbacks::grid_tab`, the nine track lists' queue actions through `callbacks::track_list`
  and eight of them through `wire_row_selection`, Browse keeping its own; a new one implements the
  trait.
- **Filtering.** Every filter box narrows through `Needle`, never `to_lowercase().contains`. A list
  in memory matches through `ui/row_match.rs`, a library-sized one through the store's folded
  columns: `tracks.search_text`, packed from `SearchFields`, and each entity's `search_name`.
- **Covers.** No row carries a decoded cover: a row asks `RowCovers.request(path, generation)`, so
  a new `TrackList` needs no plumbing. Prewarm in display order, deduped through
  `grid_prewarm::unique_artwork_paths`.
- **Audio installers seed through `ui::settings_bind::read_or_default`**; `playback_settings`,
  `file_watching`, `updater_settings` and `diagnostics` keep `if let Ok(s)` so Slint's defaults
  survive a failed read.
- **A platform-gated control reads `library::playback`'s `*_SUPPORTED` constants**, never a
  `cfg!(target_os)` of its own.

## The three tabbed pages

Favorites (nav 2), Recently Played (nav 8) and My Library (nav 3) share one contract. Each page's
tests pin its own copy, so a fourth tabbed page owes the same tests.

- **`<Page>.tab-count` is the sole tab count**, which `clamp_tab` reads.
- **Load only the mounted tab**; a tick reaching the page under another tab marks that tab dirty.
- **Counts hold `UNFETCHED_COUNT` (`-1`) until fetched. Rewind if and only if you clear, and if you
  clear, mark dirty.**
- **A pick writes no count**: the entering tab paints what it holds and refreshes behind it. The
  pick clears the shared needle, so the tab being left is marked dirty.
- **A gated load consumes its dirty flag**, and a load that stores nothing re-arms it.
- **Section guards go after the slow part**, the leave not having happened before it; a grid's load
  asks its ticket instead. Every store `release_section_state` wipes goes under the gate.
- Shared helpers: `ui::tab_bar::{clamp_tab, UNFETCHED_COUNT}`, `ui::card_grid`, `ui::most_played`,
  `ui::paged_rows`, `ui::grid_prewarm`, `ui::mosaic_hero::{compose_off_thread, MosaicGuard}`.

## The Settings page

- **A row's label and description are declared once**, read by both the row and `row-visible`, or a
  reworded description stops matching its own row; a toggle whose sub-rows hang off it owes each
  sub-row's pair. Nothing enforces this.
- **Geometry is `SettingsPage`'s and nothing in a card measures**: the view publishes `body-avail`,
  the global derives `card-w`, and controls wrap at their drawn width.
