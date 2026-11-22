# Plan: loading skeletons and card-strip panels (#169)

Status: **Phases 1–4 done** and hand-checked, except two dialog checks the dev library can't
reach: the kept-out list, which it has no files for, and Phase 3's seed step of two, its 14
duplicate groups loading in the first fetch. Phase 5 next.
Branch `feat/skeletons-and-strip-panels`.

## What you see now, and after

**Rows and cards still loading.** When you drag the Songs scrollbar through a big library, every
row the fetch hasn't reached yet looks like a track with broken tags. Its title is empty, it says
"Unknown artist" and "Unknown album", and hovering it brings up a heart and stars it hasn't
loaded. The grids do the same:

- an album card says "Unknown artist";
- artist, genre and playlist cards say "0 tracks";
- a genre tile has no colour.

In the Duplicate Tracks dialog, a copy that is still loading shows "From a cue sheet".

None of this is missing data. The row is `PagedRow::placeholder(id)`, a struct of defaults
(`crates/melodia-views/src/ui/paged_rows/mod.rs:96`), and every component draws those defaults as
if they were real tags.

After the change, a row or card that is still loading draws rounded bars where its text goes and a
plain tile where its cover goes. Bar lengths vary from row to row. Hover, selection, click, play,
queue and the menu all work on it as they do now. The real row replaces it on the frame its fetch
lands. A row that has loaded looks exactly as it does today, "Unknown artist" included for a track
that really has no artist.

**Card strips.** Search's Albums and Artists strips sit straight on the page, and so does the artist
page's Albums strip, which has a thin line under it. After the change:

- each strip and its heading sit on one rounded `surface0` panel, the colour of the Duplicate
  Tracks and welcome-guide cards;
- the line goes;
- the artist page's strip still collapses and remembers it, and when collapsed the panel shrinks to
  a slim bar holding the heading and its arrow;
- card hover, heading hover and the strip's scrollbar stay visible on the panel.

**What keeps working throughout.**

- The paging is untouched (`PagedRows`, its window, one fetch at a time, `ROW_CAP`), so scrolling
  cost and memory stay as they are.
- Strip cards, their clicks and sideways scrolling.
- The collapse flag in `views.json`.

## Why this shape

- **One flag, set in one place.**
  - Every row struct a `PagedRows` holds gains `pending: bool`. Only `PagedRow::placeholder` sets it
    `true`, and it defaults to `false`.
  - So a `VecModel` row (Search, Browse, the strips, Up Next) never reads as loading, and nothing a
    fetch builds has to say so.
  - The dialogs' two guesses at the same fact give way to it: `kept-out-body.slint:29`
    (`path != ""`) and `duplicate-row.slint:62` (`title != ""`).
  - The flag rides only rows already cached under `ROW_CAP`, so nothing grows with the library.
- **Gated by `if`, swapping the text and the tile for the skeleton.**
  - In Slint 1.19 a `ListView` builds a fresh instance for each row that scrolls in and drops the
    one that leaves (`i-slint-core-1.19.0/src/model/repeater.rs:452,488,517-526`). `row_changed`
    updates a live instance in place (`:761-771`).
  - So during a fast drag, each new row is built on whichever branch is true at that moment.
  - A pending row then builds:
    - bars in place of shaped text (an empty `Text` is still shaped,
      `textlayout/sharedparley/shaping.rs:263-271`);
    - a `SkeletonTile` in place of an `ArtworkImage`, whose rounded clip is a texture layer
      (`artwork-image.slint:53-54`).
  - That makes a drag through unloaded rows cheaper than today, not dearer.
  - The cost is one sub-tree swap per visible row when a fetch lands (`repeater.rs:1278-1305`).
    That happens once per landing, not once per frame.
  - Bars that are always mounted and gated by colour would avoid the swap. But they keep the text
    shaping and the texture layer on every pending row, during the drag frames that matter.
- **A bar takes the ink of the text it stands in for**, dimmed by one new float,
  `Theme.fade-skeleton`, which sits beside `fade-disk-only` and `fade-lifted`.
  - The bar has to show on fills that differ from one another:
    - the hover pill, `surface0`, and the selected pill, an accent tint
      (`selection-pill.slint:17-19`);
    - the panel and the dialog cards, `surface0`;
    - the playing row, whose ink is the accent.
  - No single colour stands apart from all of them, but the text's own ink already does.
  - This adds no colour, so it needs no `Palette` slot, and a custom theme's text colour carries
    its skeleton with it.
- **Bar widths come from the row's place, by arithmetic.**
  - Slint 1.19 has no hash or random builtin (`i-slint-common-1.19.0/src/builtin_functions.rs`) and
    no `%` operator. `Math.mod` works in `f32` and stays integer-valued for integer input
    (`i-slint-compiler-1.19.0/src/builtin_macros.rs:258-282`).
  - `SkeletonBar` uses one of a few fixed fractions of its `span`, picked by
    `Math.mod(seed * 2, 5)`, which puts consecutive seeds far apart.
  - The seed is the row's place in its list plus a per-column offset, so the bars in one row
    differ from each other too. Not the id: neighbouring rows' ids are unrelated, so one pair in
    five matched, and Phase 1's hand check caught a run of four identical rows in title order.
    A card is seeded by its row and column in the grid (Phase 2), and a dialog row by its place
    plus its line (Phase 3).
  - It needs no array literal and no global.
- **No shimmer and no pulse.** Rows fill in a moment after scrolling slows, and an animation would
  keep the window redrawing as long as any skeleton is up.
- **The panel is a plain rounded fill, never a clip.**
  - A rounded `clip: true` renders its contents into a texture that is redrawn on every scroll
    frame. A `background` with `border-radius` and no clip is a single path
    (`draw_border_rectangle`).
  - The strip keeps its square, radius-less clip. It is inset from the panel's edge by at least the
    panel's radius, so its square corners stay inside the curve.
- **One strip component, extracted before it gets a second panel.**
  - The artist page builds its own copy of `HorizontalCardStrip` by hand: a heading, a `ScrollView`,
    an `EntityCard` loop and an `OverlayScrollbar` (`artist-detail.slint:48-120`).
  - Giving both pages a panel therefore starts with mounting the one component on both.

## Structure

| File | Owns |
|---|---|
| `crates/melodia-ui/ui/components/skeleton.slint` (new) | `SkeletonBar` and `SkeletonTile`: how a loading row looks, the width rule, the dim |
| `theme.slint` | `fade-skeleton`, the dim's amount |
| `models.slint` | `pending` on `TrackListRow`, `QueueRow`, `AlbumRow`, `ArtistRow`, `GenreRow`, `PlaylistRow`, `EntityStripRow`, `KeptOutRow`, `DuplicateRow` |
| `paged_rows/mod.rs` | `PagedRow::placeholder`'s contract: `pending` is set, and the row still works by id |
| `EntityCard` | a card's skeleton, so each of the five grid hosts passes one flag |
| `TrackListRowItem` with `DataCell`, `QueueRowItem`, `KeptOutRowItem`, `DuplicateRowItem` | each row's own cells |
| `HorizontalCardStrip` | the panel, the heading (plain or collapsible), the scroller and its bar, for all three strips |
| `albums/mod.rs` | the one `AlbumStats` → strip-card builder, used by Search and the artist page |

Duplication collapsed in the blast radius, each in the phase that touches it:

- **Phase 1:** the seven "Blank but live" comments on the `placeholder` impls fold into the trait
  doc. The queue impl keeps its one line about acting by position.
- **Phase 3:** the dialogs' two `loaded` guesses give way to `pending`.
- **Phase 4:**
  - the artist page's hand-built strip gives way to `HorizontalCardStrip`;
  - `search::to_slint_album_strip_row` takes its subtitle as an argument and moves to
    `albums/mod.rs`, the shape `to_slint_artist_strip_row` already has. Search passes the artist
    name and the artist page the year, so both build album cards one way;
  - `HorizontalCardStrip`'s `empty-text`, `show-play-count` and `card-gap`, which no mount sets,
    are deleted, along with the `heading: ""` escape only the artist page's own header needed.

## Prior art

- **sonora**, the only one of the clones with skeletons.
  - Worth taking:
    - one bar per column, at a fraction of the column's width picked from a fixed list, "so the
      column edges do not line up into a grid" (`sonora/crates/ui/src/table/mod.rs:58-60,1046-1047`);
    - a square for the thumbnail column (`:1044`);
    - bars only for rows inside the viewport (`:1018-1023`);
    - the skeleton drawn under the artwork until it decodes (`ui/src/artwork.rs:1102-1110`).
  - Not to copy:
    - the per-element opacity pulse (`ui/src/skeleton.rs:52-73`), which asks for a new frame every
      frame while one is visible;
    - a new list of elements built per cell per frame (`table/mod.rs:1010-1065`).
  - It also never mixes skeleton and real rows in one table. Ours mix by design.
- **rox** draws a loading cover and a missing cover as the same plain tile with a faint glyph, so a
  cover that lands fills it without a flash (`rox/crates/rox-panels/src/grid.rs:1408-1460`,
  `library.rs:879-881`).
  - Not to copy: every row repainting the group tile with the last draw winning, and the
    `Vec<Thumb>` allocated on each call (`library.rs:895-900`).
- **Strawberry** and **fooyin** show one "Loading…" row, or a centred message for the whole view
  (`strawberry/src/collection/collectionmodel.cpp:195-210`,
  `fooyin/src/gui/playlist/playlistview.cpp:527-542`). That answers a different kind of wait.
  - Not to copy: Strawberry looks up each row's cover inside `data()`, allocating for every
    uncached row (`collectionmodel.cpp:1111,1135-1143`).

## Phases

### Phase 1: the flag, the skeleton parts, the track lists and the queue (done)

**Rust**

- `models.slint` gets `pending: bool` on all nine structs.
- The nine `placeholder` impls set `pending: true`: `track_rows.rs:39`, `queue_rows.rs:279`,
  `albums/mod.rs:129`, `artists/mod.rs:133`, `genres/mod.rs:102`, `playlists/mod.rs:179`,
  `most_played.rs:28`, `duplicates/kept_out.rs:251`, `duplicates/rows.rs:20`. `PagedRow::placeholder`'s
  doc states the contract.
- The eleven builders that list every field add `pending: false`: `tracks/mod.rs:81`,
  `queue_rows.rs:308`, `albums/mod.rs:136`, `artists/mod.rs:140`, `genres/mod.rs:111`,
  `playlists/mod.rs:186`, `most_played.rs:36`, `search/mod.rs:178` and `:191`, `favorites/rows.rs:53`,
  `duplicates/kept_out.rs:273`.
- `now_playing/up_next.rs:367` built a queued track the fetch didn't return through
  `QueueRow::placeholder`. That row is gone, not loading, so it now builds
  `QueueRow { id, ..QueueRow::default() }` and never reads as pending.
- The two test helpers that build a landed row from `placeholder(id)`
  (`paged_rows/tests/grid_tests.rs:57`, `paged_rows/tests/paged_rows_tests.rs:102`) set
  `pending: false`, as a real fetch does.

**Slint**

- `components/skeleton.slint`:
  - `SkeletonBar` takes `ink`, `seed`, `span` and the font size of the text it replaces, which sets
    its height. `SkeletonTile` takes `ink`, a size and a radius.
  - Both are childless rounded `Rectangle`s, with no `opacity` and no `animate`.
  - A bar's `span` is a width the host already knows, such as a column's `layout.*_w` or a card's
    `tile-size`. Otherwise the bar is placed by `x`/`y` inside a plain cell. It must never be a
    fraction of the layout it sits in, which would feed back into that layout's own size.
- `Theme.fade-skeleton`.
- `TrackListRowItem` (`components/track-list/track-list-row.slint`), reading `row-data.pending`:
  - **Tile and text:** the artwork becomes a `SkeletonTile`. The title, artist, album, genre, year
    and length become bars; `DataCell` takes `pending` and `seed`.
  - **Number:** under `"index"` it keeps showing the position, a fact the row already has. Under
    `"track_number"` it becomes a bar, because there the number is the tag's.
  - **Marks and hover controls:** no favourite mark, no hover stars and no hover heart;
    `favorite-button-shown` gains `!pending`. So the length bar stays put under the pointer.
  - **Unchanged:** the pill, the checkbox, click, double-click play, drag and the menu.
- `QueueRowItem` (`views/queue-panel/queue-row.slint`): a tile, a title bar and a duration bar. The
  position number and the hover remove button stay, since both act by position.

**Docs**

- `.claude/rules/ui-patterns.md` gets `SkeletonBar` and `SkeletonTile` on its Lists catalogue line.
- It also gets a contract: a row or card held by a `PagedRows` draws its skeleton when `pending` is
  set, never its defaults, and only `placeholder` sets it.

**Done when** the gates pass and Songs, the four details, Favorites' Songs and the queue show
skeletons whenever a drag outruns the fetch.

### Phase 2: the grids (done)

- `EntityCard` takes `pending`, and `row-index` and `column-index` for its place in the grid. While
  `pending` is set the card shows:
  - a `SkeletonTile` in the tile's place, round when `circular`;
  - a title bar in `Theme.text`'s ink and a subtitle bar in `subtext0`'s. Both bars swap in as one
    block with `Theme.pad-xs` between them, since a bar is shorter than the line box it replaces.
    Phase 3 moved that reason onto `SkeletonBar`'s height, which every stack of bars shares;
  - no `subtitle2` and no hover zoom. The play-count and smart-playlist badges need no gate: a
    placeholder's zero count and `is_smart: false` already hide them, and the zero rule stays the
    leaf's (`play_count_badge_tests.rs`).
- The bar seed is `2 * row + column`. The doubled row keeps every touching card, diagonals included,
  off a shared width. The place in the whole list would match a whole column at 5 or 10 columns.
  The rule lives in `EntityCard`, so a host passes the two indices and nothing else.
- The title and subtitle inks and the tile radius are named once, the text and its bar or tile
  each reading them.
- Selection box, click, context menu and drag are unchanged.
- The five grid hosts pass `pending`, `row-index` and `column-index`: `album-grid.slint`,
  `artist-grid.slint`, `genre-grid.slint`, `playlist-grid.slint` and `entity-card-grid.slint`. The
  last covers Most Played on both pages, and Favorite Artists, which pages the same row type.
- `entity-card-grid.slint` hides `CardFavoriteButton` while pending, because the heart shows a fact
  the card doesn't have yet. The playlist hover trio stays, since it acts by id.
- Constraints to hold (`crates/melodia/tests/`):
  - `opacity_layers.rs`: `entity-card.slint` keeps its single layer;
  - `context_click.rs`: every hover read stays on `card-hovered`;
  - `card_selection.rs`: the selection tint stays un-animated.

**Done when** the four library grids and Most Played paint skeleton cards during a fast scroll.

### Phase 3: the dialog lists (done)

- Each row reads `pending` and swaps its text block for a block of bars, with `alignment: center`
  and `Theme.pad-xs` between bars. Each bar takes the ink and font size of its line.
  - **Span:** a named `text-w`, the text column's width, derived from the card width the host
    binds.
  - **Seed:** `row-index + 2 × line`, the row's place in the list. The same line in consecutive
    rows lands two widths apart, and one row's lines all differ. Stepping two per line also keeps
    a row's last bar off the next row's first, which a step of one matched in every two-line row.
    `KeptOutBody` and `DuplicatesBody` pass `idx`, and `DuplicateRowItem` hands it to both
    children.
- `KeptOutRowItem` (`components/dialog/kept-out-body.slint`): the `path != ""` guess gives way to
  `pending`. A file name bar and a folder bar, and Bring Back waits for `!pending`.
- `GroupHeaderRow` (`components/dialog/duplicate-row.slint`): the `title != ""` guess gives way to
  `pending`. A title bar and an artist bar. The match chip, the Dismissed chip and the copy count
  wait for the fetch.
- `CopyRow`: the whole three-line column swaps for three bars, so each chip and fact waits with its
  line.
  - This covers "From a cue sheet", which a loading copy showed because `removable` defaults to
    false.
  - It covers Now Playing too. That chip is a fact the row already has, but splitting the line to
    keep it for the moment before a fetch lands isn't worth it.
  - The checkbox and the pointer cursor need no gate: `removable` defaulting to false already
    holds them back, the same rule as Phase 2's badges.
- The issue's list leaves out the finder's rows, but they load as you scroll, which is the issue's
  own test for being in scope.

**Done when** both lists show skeletons on their `surface0` cards during a fast scroll.

### Phase 4: the strip panels (done)

**Rust**

- `albums::to_slint_album_strip_row(a, subtitle)` replaces `search::to_slint_album_strip_row`, and
  Search passes the artist name.
- `ArtistDetail.albums` becomes `[EntityStripRow]` (`globals/artists.slint:30`). Its rows take the
  year as subtitle, formatted in Rust. The sites are `artists/detail.rs:129,188,227`,
  `artists/mod.rs:115` and `artists/callbacks/lifecycle.rs:38`.

**Slint**

- `HorizontalCardStrip`:
  - **Panel:** the root fills with `Theme.surface0` at `Theme.radius-lg`, the radius `TopResultCard`
    and `SectionCard` use for page cards. It is padded by at least that radius, with no clip and no
    border.
  - **Collapsing:** new inputs `collapsible` and `collapsed`, and `callback toggled()`. A collapsible
    heading is a `CollapsibleSectionHeader`. The strip body sits behind `if !collapsed`, so
    collapsing still unmounts the scroller that `hand_back_covers` waits on
    (`artists/callbacks/detail.rs:57-59`).
  - **Bar and cards:** the bar takes `track-color: Theme.scrollbar-track-on-card`, and cards pass
    `on-card: true`.
  - **Removed:** `empty-text`, `show-play-count`, `card-gap` (now `Theme.pad-md`, the 12 px it
    defaulted to) and the `heading: ""` escape.
- `EntityCard.on-card`: the hover fill becomes `card-control` instead of `surface0`, because the
  panel is `surface0`. This matches what `SearchBar.on-card` does.
- `CollapsibleSectionHeader` hovers to `card-control` instead of `surface1`. Its only mount now sits
  on the panel, and in several palettes `surface1` is almost the same as `surface0`.
- `artist-detail.slint`:
  - the hand-built strip and the 1 px line give way to one `HorizontalCardStrip` mount: 140 × 184
    cards, `request-album-cover`, `open-album`, and `collapsible`;
  - the wrapper's bottom padding now keeps the panel off the list header.
- `search-view.slint` is unchanged apart from the panel it inherits.
- **Coverless hover zoom:** the strip binds `cover-scale-hover` to the rule every grid and the
  artist page use, 1.15 for a card with no artwork path and 1.04 for one with a cover. Search's
  coverless strip cards move from 1.04 to 1.15 with it (decided 2026-11-22).

**Constraints to hold**

- `scrollbars.rs`:
  - the strip's `ScrollView` stays in `components/grid/`, which keeps the grid floor met;
  - removing the artist page's scroller and bar leaves the tree-wide floors well clear.
- `palette_fade.rs`: every brush `animate` stays gated.
- `hero_chips_tests.rs`: the strings `artist-detail.slint` must not contain.
- `view_state_tests.rs`: the collapse flag.
- `@tr("Albums")` stays in `artist-detail.slint`, so the translation catalogue doesn't change.

**Docs**

- `ui-patterns.md`, its Grids line: `HorizontalCardStrip` is the one strip, and it sits on its own
  panel.
- `ui-patterns.md`, its card-control contract: `EntityCard` takes `on-card`.

**Done when** all three strips sit on panels and the artist page's collapses to a slim bar.

### Phase 5: Tests (not started)

## Verification, every phase

- `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings` and
  `cargo test --locked --workspace`.
- `code-style.md`'s comment audit over the phase's diff.
- No translation run, since no `@tr` string is added, changed or removed.
- No RSS run, since nothing new is held per track and the flag rides rows the cache already caps.
- I stop at the static gates. The hand test is yours, from the issue:
  - drag the Songs scrollbar from top to bottom in a big library;
  - give the wheel or touchpad a fast flick;
  - hover, select and play a row while it is still a skeleton;
  - scroll each grid fast;
  - open a long queue after playing everything from Songs;
  - check a track with no artist tag;
  - collapse and expand the artist page's Albums strip, on an artist with one album and on one with
    many;
  - run a search with album and artist results;
  - do all of it in a light theme and a dark one.

## Risks and notes

- **A row deleted since its list was read** comes back missing from its fetch and stays a skeleton
  until the library tick that deleted it reloads the list (`paged_rows/mod.rs:586-588`). This is
  short-lived, and accurate, since it is no longer a track.
  - The duplicates finder follows no library tick, only a new find re-reading it, so there a copy
    deleted while the dialog is open stays a skeleton until the next find. So does its group's
    header when that copy was the group's lead, the header being built from it
    (`duplicates/rows.rs`'s `build`).
- **A pending row's menu** opens with defaults for its heart and rating. It corrects itself when the
  row lands, because the menu reads the live row.
  - A pending kept-out row's Copy Path copies nothing until the row lands, for the same reason.
  - Its Open Containing Folder returns on the empty path (`duplicates/kept_out.rs:199-202`).
  - A pending group header offers Not Duplicates even on a dismissed group.
- **KDE light:** its `crust` is almost the same as `surface0` (`kde.rs:62-63`), so the strip's
  on-card track is as faint there as every dialog's track is today.
- **Search:** the panels beside the bordered `TopResultCard` need a look by eye, as the issue says.
