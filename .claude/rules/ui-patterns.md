---
paths:
  - crates/melodia-ui/ui/**/*.slint
  - crates/melodia-views/src/ui/**/*.rs
  - crates/melodia/src/boot/**/*.rs
  - crates/melodia-core/src/utils/toast.rs
  - crates/melodia-ui/build.rs
---

# UI patterns: what already exists

Reach for something here before building a second one. Each component's own header argues its
shape; this file is the catalogue plus the contracts no one file can hold. What *builds, looks
right, and is wrong* is `slint-pitfalls.md`.

## Catalogue

Paths are under `crates/melodia-ui/ui/components/` unless stated.

- **Overlays.** `Tooltip` (an in-tree pill) and `TooltipFrame` (its top-layer mount, for a host
  Slint paints before its occluder). `PopupSurface` wraps every `PopupWindow` body; `PopupFit` and
  `PopupColumn` place and scroll one whose rows can outgrow the window. `FocusLossWatcher` goes
  inside every popup, `PopupDismissCatcher` over a popup's empty regions, never a bare `TouchArea`.
- **Menus.** `context-menu.slint` holds every right-click menu's parts (`ContextMenuPopup`,
  `MenuItem`, `FavoriteMenuItem`, `MenuFlyout`, `CopyField`, `ContextClick`,
  `ContextMenuMetrics`); `track-list/track-context-menu.slint` is the row menu, the only place its
  entries are mounted; `RatingMenuRow` is the rating entry in it and in the bar's overflow menu.
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
  its count (mounted only by `EntityCard`, so `StationCard`, an `EntityCard` host like any grid,
  owes the count and not the markup), `GridEmptyState` the empty block for grids and lists alike.
- **Lists.** `TrackList` with `TrackListScrollbars`; `TrackTitle` for any row showing a track's
  title; `DragSource`, `ListDropTarget` and `DragGhost` for an in-window drag; `WindowRectReport`
  and `FileDropBanner` for a file dragged in from the OS (`desktop-shell.md` has the route).
- **Heroes.** `TabSearchHeader` (the only `TabBar` mount), `MosaicTabHero` and `LibraryTabBand`,
  `MosaicHeroTile`, `HeroChipStrip`, and `HeroBackdropStack` (bands) / `NpBackdropStack` (Now
  Playing, miniplayer) over `HeroBlurBackdrop` and `AuroraBackdrop`.
- **Rounded edges.** `EdgeOutline` strokes inside a rounded clip; `RoundedFrame` rounds a panel
  without one, only over an opaque fill of exactly its `ground`; `TuckedFill` fills under a host's
  own rounded stroke, never under a drop shadow.
- **Settings.** `SectionCard`, `SettingRow`, `ChipGroup` for a short preset set and `ChoiceList`
  for a long one, `ColorDotGrid`, `SettingSlider`, `PreampSlider`, and per effect a
  `settings/*-controls.slint` plus a `*Text` global shared by its card and its dialog.

## Tooltips

- **The window edge is the pill's; a clip or a later sibling is the host's.** The pill flips and
  slides inside the client rect, so no host picks a side to dodge the window. A host at the content
  panel's edge, whose clip the pill can't see, aligns it to its trailing edge
  (`IconButton.tooltip-align-end`, `TooltipFrame.align-end`); an unlabelled `PillButton` does so
  itself through `icon-only`. A pill landing on a later-declared sibling is lifted with a constant
  `z` at the level where the two are siblings, `z` keeping a layout's order.
- **A band publishes an anchor and draws no pill** (`tooltip_mounts.rs`). An anchor crossing a
  component boundary leaves through `out` properties, a global, or a zero-width collapse rather
  than an `if`, an id inside an `if` being unreadable from outside.
- **`sidebar-tip` is the one hand-rolled mount**: its `x` rides the live rail and a `held` latch
  carries it across the gaps between rows. `changed watched-mini-render` clears it, the miniplayer
  swap destroying the rail with no unmount hook; a new path that destroys the rail owes the same.
- **A mount inside a `PopupWindow` passes `in-popup: true`**, as both volume readouts do.

## Selection

- **A multi-selection grows no pill; the right-click menu carries it**: the count, every batch
  action, "Select all" wherever the host has a set and "Clear selection" wherever it holds one,
  whether or not the clicked item is in it. The batch entries need the clicked item in the set
  (`Selection.acts-on-set`, read by the row, card and station menus alike).
- **Escape and a section leave drop a selection**, an unseen set being indistinguishable from none.
  Escape's arm is last in `shortcut-scope.slint`'s chain. Most surfaces empty `selected-ids` with
  their rows; `Tracks` and `Browse` keep their rows, so each clears on its own. `CardSelection`
  clears in each of the four section-gate closures, in the closure itself, which is what
  `card_selection.rs` reads; Radio keeps its rows and still clears its card picks. `BrowseCardGrid`
  is the exemption, its cards being its list rows.
- **A card menu offers both directions only where it doesn't know the state**, and gates an entry
  on the operation, never on the kind (`card_selection.rs` holds `card-is-smart` to Edit Rules).

## Colours, sorting

- **A floating surface edges with `Theme.surface2`, never `Theme.border`**, and a control resting
  on a card fills with `Theme.card-control` / `card-control-hover`. Card-only controls default to
  the pair; `FilledChip`, `SliderTrack` and `SearchBar` (`on-card`) take it from a card host. Both
  are palette slots, which `themes::toward` fills for the generated palettes.
- **Sorting** goes through `next_sort`, `persist_view_sort` and `persisted_sort`, and a view whose
  natural order is a synthetic field (`"position"`, `"rank"`, `"recency"`) through
  `next_sort_with_natural`. Where the order *is* the page, `sortable: false` instead. A
  `SortPillRow` mount owes equal-length arrays (`melodia_testkit::sort_mount_arrays`).

## Grids

- **The seven library grids are `paged_rows::grid::PagedGrid`s loaded only through
  `ui::card_grid`**, each a `CardGrid` stating its query and the facts beside its cards. A chunk
  keeps its `ModelRc` while Slint holds it, the repeater comparing by pointer, so a landed card
  repaints only its cell. A load re-reads the column count as it lands; launch loads only the shown
  tab's grid. A grid over another card type implements `CardGrid`.
- **A grid that builds its cards** (Browse, Radio's three) chunks them through
  `ui::grid_rows::chunk_built_rows` into `model_diff::replace_rows`.
- **A grid that fits on one row sizes its cards for the cards it has**: each of the five mounts
  reads `lone-row-cards` off its own row model, never a published count. `max-card-w` is mirrored by
  `grid_prewarm::MAX_CARD_W` (`grid_prewarm_tests` reads the `.slint`).
- **A card's hover controls are the host's**, in `EntityCard`'s overlay slot. The host owes
  `overlay-hovered` for each and frames them off `tile-size` or `select-box-bottom`, never `parent`
  (`slint-pitfalls.md`); a control up while idle sits beside the card on `IconButton.fade`.
- **A card's heart reads the row that keeps it**: `EntityStripRow.is_favorite`, or on Browse the
  list row through `row_index`, so one toggle patches both presentations.

## Hero bands

- **Anything painting on a hero reads `HeroBackdrop`, never `Theme.*`**, six views sharing it.
  `backdrop::kind`, the flag's only Rust reader, picks between two unrelated answers: the **blur**
  solves a scrim driving the composite into a dark band and seeds every tier from the artwork's
  hue; the **aurora** paints `Theme.base` under a neutral chrome and leaves the washes alone.
  **The setting is restart-gated**, the artwork tiers deciding at construction whether to build a
  blurred half. The `BackdropSample` is produced by the decoder in its `spawn_blocking`, never by
  the publisher on the UI thread.
- **A coverless square takes `placeholder`, not `chrome`**, which on the aurora is a neutral ink and
  paints a pale slab. Anything derived from `chrome` fades with `transparentize`
  (`hero_backdrop_tests` walks for it).
- **Each stack wrapper's `shown` is the host's question**: ungated on the mosaic bands and Now
  Playing, `detail-open` on `LibraryTabBand`, and on the miniplayer the drain `MiniBackdrop`
  unmounts after. `backdrop_mounts.rs` pins the gates and their forwarding.
- **Every hero has washes, so the aurora gates on the setting alone.** A coverless hero substitutes
  seeds: Genre Detail its `GenreStops` tile pair (swapping it for the dimmed pair paints plausibly
  on both arms, hence a pin), anything else `Theme.accent` as two seeds under the
  `aurora::WASH_MAX_TONE` ceiling, the number to tune by eye. A genre has no `BackdropSample`, so it
  publishes through `apply_gradient`.
- **`ActionPill` and `SearchBar` inside a hero keep `Theme.floating-chrome-bg`**, safe only because
  the backdrop is pinned; a translucent placeholder's fill has to be a hero token.
- **No layer may ease out of a held tier**: My Library's globals often describe a hero closed tabs
  ago, so the idle value has to be honest, falling back on `detail-open`.
- **A hero publishes only while it is the one on screen.** `apply_detail_artwork` guards only its
  `HeroBackdrop` write, the view's own slots filling while hidden; `apply_genre_hero` and
  `hero_chips::publish` likewise. On the four details the gate is a live `tab_is_mounted` read after
  `on_applied`: a section shadow lags a cross-section drill and goes false under Now Playing. A
  launch reopens only the shown tab's detail (`SectionState::defer_restore`).
- **A leave is not a teardown on a tabbed page.** `*Detail.*-id >= 0` means "this banner is in the
  globals", `open_*` writing the id last, in the tick with the cover, blur, solve and chips. Colours
  and images go back on `hero-collapsed` or the page's teardown, gated on `the_band_is_up` (nav
  still 3) rather than the section predicate; chips clear a step earlier (`clear_if_stale`).
- **`HeroChips` publishers take their facts as arguments and read back no Slint property.** Facts a
  stats row lacks come from the worker (`queries::track::scope_facts`, `genre_lines`,
  `most_played_totals`, folded by `ui::hero_folds`), and `release_section_state` resets those folds.
  A spread of one is stated as nothing, an empty set as no band, and a band states facts about the
  page's set, never the current filter.
- **`Theme.hero-title-size` is the one title size**, pinned over `HERO_VIEWS`.
- **The curated publish is gated whole**, unlike a detail view's: the page's leave wipes its models
  and forgets the `MosaicGuard`, so slots filled while hidden have nothing to be ready for.
- **Now Playing's accent tiers live on `Player`**: reach for a tier, never a fresh `.with-alpha(…)`.

## Lists and playback

- **The row menu's `view-context` drops the entry naming the current surface and picks the remove
  arm.** A third list's remove arm is another `if` there, a label passed in missing `@tr`. "Go to"
  calls `Nav.go-to-*` directly; the bar reads its ids through `ui::track_links`.
- **The item a menu was raised on stays lit**, an open popup taking its hover: a row reads its
  popup's `is-open` into its one `hovered`, a card gets `EntityCard.menu-open` (`context_click.rs`).
  Copy acts on the effective set in the order handed over, one line per item.
- **A drag carries `Drag.payload`, written from both trees**: a row arms it,
  `winit_filter::disarm_drag` clears it on every press, and a list whose rows move under a drag
  revokes its origin. **A new
  reorderable list owes the revoke**, or the lifted index lands on another track. A press on a
  picked row lifts every pick (`picked-run()`), and every tier moves through
  `melodia_core::utils::block_move`. The queue panel takes another list's items through
  `Queue.drop-in`; Up Next's `split-row` sits on the repeat-all wrap, which
  `up_next::play_order_move` refuses to cross.
- **A list that can hold the library is `ui::paged_rows::PagedRows`, never a `VecModel` copy.** The
  Songs tabs and the four details are `track_rows::TrackRows`, loading through `PagedTrackList`;
  the queue panel's `QueueRows` picks by row, a track queued twice being two `QueueEntry`s, and
  reads the queue through `queue_panel::rows::read_queue`.
- **The queue panel.** `Queue.shown` (docked with room or the overlay open, less Now Playing and the
  miniplayer) is computed on the `AppWindow` root, which writes it and `Queue.floating` in one
  handler before firing `shown-changed`; Rust's rows and private cover tier follow that.
  `Queue.toggle()` is Rust's, closing Now Playing where `globals/queue.slint` can't import `Nav`. A
  narrow window takes room back in order (sidebar to rail, panel to its minimum, panel aside) so the
  content keeps `Theme.content-min-w`, published through `changed` mirrors, and every part of the
  sidebar draws from `Nav.sidebar-rail`, never the persisted `sidebar-collapsed`. Past the last step
  the same `QueuePanel` opens as an overlay; `Queue.open` persists, `Queue.overlay-open` doesn't.
  **Narrowing hides a docked panel rather than floating it** (Kenan): a resize must never raise
  something that dims the page and takes the keys.
- **Shared wiring.** The four details go through `callbacks::track_detail`, the four grid tabs
  through `callbacks::grid_tab`, the nine track lists' queue actions through `callbacks::track_list`
  and their selection through `wire_row_selection`; a fifth of any implements the trait. A library
  change under an open detail owes the grid a reload, which each tab's `wire_close` pays through
  `grid_tab::reload_owed_grid`.
- **`play-row` replaces the queue with the view's displayed ids** (`player_play_tracks`,
  `play_row_start`). No single-track path and no Play All pill; Shuffle is the one pill, and
  appending is the menu's.
- **Track-list insets belong to the list**: pages mount `TrackList` full width with no side padding,
  Browse keeping a `page-inset` for its header and grid and Search's Songs passing `0px`. Bottom
  padding never goes on the root (the dead-strip pitfall).
- **The Now Playing right column is one column with three arms** (Up Next, lyrics, station). A
  fourth column would reflow the artwork whenever availability changes, `content-width` driving the
  chip wrap, strip and cover.
- **The transport's hold timings, tooltips and scan step are read off `Player`**, never spelled at
  a mount.

## Dialogs, toasts

- **A raise that crosses an `await` owes a `DialogClaim`, and a deferred raise is an opener too**,
  single caller or not: inline chrome bumps nothing. `dialog_openers.rs` walks for both.
- **Notifications** route through `ui::shell::notifications::run_action`; a card prop is `data:`,
  Slint reserving `row`. **Backend failures toast through `utils::toast`**; routine ones (favorite,
  rating, nav) stay `spawn_logged!`.

## Filtering

- **Every filter box narrows through `Needle`**, never `to_lowercase().contains`, `fold_needle`
  being its only constructor. A list in memory matches through `ui/row_match.rs`, a library-sized
  one through the store's folded columns: `tracks.search_text`, packed from `SearchFields`, and each
  entity's `search_name` (`library-data.md`). The box's fold is deliberately looser than the FTS
  side's, except years: a substring here, a prefix there. A card grid sorts on its name and matches
  on its `search_name`. On the tabbed pages one needle is shared across tabs, and a pick clears it.
- **Radio's Browse box narrows nothing**: the needle is the directory query and its other readings
  are scope pills. A pill and a filter chip write the same `StationSearch` fields, so
  `facets::apply_pick` is their only writer and a pill lands as one `edit_query`.
- **My Library's one `SearchBar` feeds nine surfaces** through `my_library/filter.rs::dispatch`, a
  write because an element can't bind another global's property. Grid and list globals take
  `apply-filter(text)`, details `filter-changed(text)`, every arm calling `set_filter` first. A
  move of the surface under the box reseats it through `detail-scope-changed()` →
  `filter::sync_box`, from five `changed` mirrors and from the last statement of each detail's open.

## Covers

- **No row carries a decoded cover**: a row asks `RowCovers.request(path, generation)`, so a new
  `TrackList` needs no plumbing. Prewarm in display order, deduped through
  `grid_prewarm::unique_artwork_paths`.
- **Every card grid shares `ui::grid_prewarm::tier()`.** A tier stays private on decode size
  (Search's strips, the row tier), lifetime (the queue panel's, `CoverThumbs::backed_by` the row
  tier) or layout (Radio's logos).
- **A leave shrinks the grid tier to proxies** (`hand_back_covers`), and a prewarm is guarded ahead
  of its decode, on `section_active()` or `SectionState::grid_on_screen`. **Radio's logo tier is
  released outright**, its card laying out off the decoded size, and its generation rewinds to `0`.
- **No uncapped lookup decodes on the thread that asks**: `get_or_schedule_opt` schedules a miss and
  `ui::cover_generation::notify_on_decode` bumps the generation the binding reads. A surface with no
  generation to come back on uses `grid_cover_blocking`; bounded strips and one-shot slots stay
  inline. `ui::cover_generation::tests` walks for both.
- **`covers-generation` is a repaint token** and on a grid nothing else: `grid_prewarm::grid_cover`
  schedules every miss. Only Radio's leave reads `0` as cold (`radio::covers::logo_cover`).
- **The grid tier is sized off the content panel's width** (`grid_prewarm::tune_for_display`,
  `WindowChrome.content-panel-width`), never the window's.

## Releasing what the UI pins

- **A detail close releases its global images** through `detail_view::release_detail_hero_images`,
  or `SharedPixelBuffer` Arcs pin the cover and both blur slots. On My Library it waits for
  `hero-collapsed`, a `Timer` in the band armed and cancelled by the morph's edge, so the collapse
  doesn't paint a fallback glyph; the page teardown is the backstop.
- **Dialogs release through the one `on_closed`**: `invoke_closed_teardown()`, the two `image`
  globals reset to `Image::default()`, then `allocator::trim`. The teardown is a `public function`,
  a callback's single slot letting Rust's handler replace a default body. A new dialog kind pinning
  an image extends that handler.

## Input

- **Native dialogs go through `ui::file_dialog::parented`** (root `CLAUDE.md`).
- **`ShortcutScope` is the only `FocusScope`**; put new keys there. **A `changed` mirror on each
  view-mounting branch selector regrabs focus**, Slint not handing it back when the focused item is
  destroyed; one unmounting a composite view also owes `CompositeScroll.reset()`. The queue's
  mirror grabs on its rising edge only. Keys other than Esc and Ctrl+Z gate on `!Dialog.open`, Q
  on `!MiniPlayer.active`, Ctrl+B on `!Nav.sidebar-yields`. Ctrl+Z sits above that gate and acts
  only while `Notifications.undo-row >= 0`, as the notice's Undo button works over a dialog.
- **Whether the docked queue holds the keys is decided per press in the winit filter**
  (`claim_queue_keys`, off a hover sentinel's `Queue.pointer-in`). The sentinel takes the wheel its
  children leave, a row's menu hands the keys back as it closes, and the overlay always holds them.
  The queue's Esc, Ctrl+A and Delete sit at the root gated on `queue-keys`.
- **View transitions.** `Nav.pending-enter-from` is written synchronously before the `if` flips
  (`ui/nav_transition.rs`), and every Slint-side mount writes its own edge, an unwritten one being
  whatever the last navigation left. `enabled: false` turns a view's animation off, `slide: false`
  keeps the fade for a view something else moves.
  - **The launch doesn't animate**: `Nav.suppress-enter-animation` is declared raised, never raised
    from Rust, `AppWindow::new()` already instantiating the declared page. The first mount to settle
    lowers it one statement after `shown` flips, gated on `enabled` (`ui::startup_motion_tests`).
    `MiniPlayerSwitch` raises it again for the crossfading rebuild out of the miniplayer.
  - **A page with sub-views nests a second transition**, disarmed at mount and armed in the tab
    bar's `selected`, its direction from `bar.previous-index`. My Library slides only while the band
    isn't morphing.

## Settings and nav wiring

- **Section visibility goes through `SectionActiveGate`**, mounted on the `AppWindow` root in
  nav-index order: one per section, plus one per tab only where a tab leave has to be a section
  leave. A new mount passes `mini-active: mini-switch.active`, which leads the global by a beat;
  `tab-index`/`current-tab` default `-1`, a `0` deactivating every section.
  - **My Library mounts five at index 3**, so a tab leave is its view's section leave. Its page
    leave rides `page-active-changed` off `changed watched-nav-idx`, behind a Rust `Cell<bool>`
    latch, a `changed` handler being unable to tell which index it left.
  - **Ask what a tab leave hands back before mounting a second gate.** Favorites, Recently Played
    and Radio mount one each; on Radio a per-tab gate would discard a directory answer that cost a
    round trip.
  - **Gates fire on transitions only**, so `install_views` writes the persisted nav index, with
    `seed_tab`, before `wire_all` seeds each `section_active` shadow. The trackers baseline silently
    inside `AppWindow::new()` with `mini-switch.active` true, so every gate starts `false` and a
    section seeded `true` would stay active all session.
- **`views.json` keys nav state by view id**; only the back button resets a detail id. A new detail
  is a `view_id::*` const, `set_last_detail_id` on open and close, and a seed fn.
- **Audio installers seed through `ui::settings_bind::read_or_default`**; `playback_settings`,
  `file_watching` and `updater_settings` keep `if let Ok(s)` so Slint's defaults survive a failed
  read.
- **`TabBar` is mounted once, inside `TabSearchHeader`**, which a page with tabs takes whole. A host
  reads `previous-index` for the tab being left, a cell writing `selected-index` before `selected`
  fires. The Rust half is `clamp_tab(tab, tab_count)`, clamping every persisted tab index on read.

## The three tabbed pages

Favorites (nav 2), Recently Played (nav 8) and My Library (nav 3) share one contract. **Read this
block first; each page's section below is deltas only.**

- **`<Page>.tab-count` is the sole tab count**: `clamp_tab` reads it, and a per-page test pins it
  against the `tab-*` constants, the body branches and both `@tr` arrays.
- **Load only the mounted tab.** A tick reaching the page under another tab marks that tab dirty for
  its pick.
- **Counts hold `UNFETCHED_COUNT` (`-1`) until fetched** and rewind with the model clears, so
  neither `== 0` nor `> 0` fires on stale data; a plural `Text` reads one through a `>= 0` ternary.
  **Rewind if and only if you clear, and if you clear, mark dirty.**
- **A pick writes no count**: the entering tab paints what it holds and refreshes behind it. The
  pick clears the shared needle, so the tab being left is marked dirty.
- **A gated load consumes its dirty flag** (seeded `true`), `release_section_state` re-arms it, and
  a load that stores nothing re-arms it too (`PagedTrackList::load_dropped`,
  `CardGrid::load_failed`).
- **Section guards go after the slow part**, the leave not having happened before it; a grid's load
  asks its ticket instead. Every store `release_section_state` wipes goes under the gate.
- **A pick can't await a prewarm**, so its rows go through the non-hopping
  `apply_filtered_tracks_now`.
- **Covers ride one tier no leave clears, and a reload repaints only what moved** (`PagedRows::show`
  and the card re-read), so nothing hashes a tab's contents.
- **A leave owes `mark_dirty()` for exactly what it hands back.**
- Shared helpers: `ui::tab_bar::{clamp_tab, UNFETCHED_COUNT}`, `ui::card_grid`, `ui::most_played`,
  `ui::paged_rows`, `ui::grid_prewarm`, `ui::mosaic_hero::{compose_off_thread, MosaicGuard}`.

### Per-page deltas

- **Favorites**: `refresh_hero` is ungated, the band stating the same facts on every tab. All three
  tabs sort and filter in SQL and keep their lists across a pick, each with a dirty flag of its own.
  A pick warms `GRID_PREWARM_AHEAD` covers, not the tier's capacity, which would evict its own work.
- **Recently Played**: neither tab sorts, by decision, so there is no sort state at all. `sortable:
  false` threads `TrackList` → `TrackListHeader` → `HeaderColumn` → `HeaderCell`, and the middle
  links are the ones to pin. The band states the recency set on both tabs.
- **My Library**: a tab switch is a section switch. The back arrow closes the detail
  (`origin-nav-index` `-1`), the tab bar naming the detail's tab throughout, and `Origin` survives
  only as `still_current`. Navigation toward an opening detail rides a `PendingNav` into
  `on_applied`, so a path skipping that hook lands the navigation itself or nothing. Only
  `on_tab_changed` records history, a pick being the one tab move that is the user's own.

## The Settings page

- **A non-empty search mounts every category page at once**, which is the whole cross-category
  search; `search-input` and `search-query` are joined by a `FilterThrottle` for that reason.
- **A new category** is a page file, a `category-*` constant, a router branch and a search mount,
  both inline arrays in the panel, a `SettingsCategory` variant, and `category-name:` on every
  section. `category-count` is the sole count, and `settings_page_tests` holds the rest to it.
  Reordering renumbers `views.json`'s index, so it takes a new key. A link from outside the page
  goes through `SettingsPage.open-category`.
- **A card's search term carries its category's name** as a term of its own, users typing it.
  **A row's label and description are declared once**, read by both the row and `row-visible`, or a
  reworded description stops matching its own row; a toggle whose sub-rows hang off it owes each
  sub-row's pair. Nothing enforces this.
- **Geometry is `SettingsPage`'s and nothing in a card measures**: the view publishes `body-avail`,
  the global derives `card-w`, and controls wrap at their drawn width. The panel and the cards start
  level through `SectionCard`'s inner title and `SettingsPage.card-header-h`; a heading above a
  card breaks that. Each card carries the gap under it, so a section filtered away leaves no hole.
- **The body stays box layouts**; only the wrapping strips are `FlexboxLayout`s, a flex body
  measuring each card at an uncommitted width (`slint-pitfalls.md`).
- **A colour that follows something is a switch above its grid, never a swatch in it**, the grid's
  index held at -1 while it is on.
- **Language is a category of its own**, so a reader stranded in a language they can't read finds
  the way back by the panel's glyph.
