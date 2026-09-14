# Track List Columns

Working doc. Delete when the feature ships.

**Status:** Phase 1 done and through the static gates. Waiting on the manual test; Phase 2 after.

## What the user sees today

Every track list column has a fixed pixel width. On a wide window they stay that size and leave
dead space on the right. On a narrow one Length, Year and Genre slide off the edge behind a
horizontal scrollbar. A divider drag changes only the column to its left and pushes everything
after it along, and Length has no divider at all. Widths reach `views.json` at shutdown.

## Goal

Columns grow and shrink with the width, each one can still be dragged to size, and saved widths
keep working, existing files included. No column ever disappears: under the sum of every column's
floor, all of them keep shrinking in proportion and text elides. No horizontal scroll at all.

## The model

| column | kind | floor | max | default |
|---|---|---|---|---|
| `#` | rigid (px) | 40 | 200 | 56 |
| Title | flex (weight) | 150 | none | 320 |
| Artist / Album / Genre | flex | 60 | none | 200 / 220 / 140 |
| Year | rigid | 50 | 200 | 72 |
| Length | rigid | 56 | 200 | 88 |

`ColumnWidths` on disk is unchanged. Rigid fields are pixels; flex fields are the width that
column had when last sized, read as a relative weight. An old file keeps its ratios.

**`resolve(columns, avail, gap)`** runs once per list per width change:

1. Content width is `avail` less the side padding and one gap between visible columns.
2. Rigid columns take their stored width clamped to their range.
3. Flex columns water-fill the rest by weight, a column under its floor pinned there.
4. If flex room is under the flex floors, flex sits at floors and rigid columns shrink toward
   theirs in proportion to slack.
5. Under the sum of all floors, each visible column gets `floor × content / Σfloors`.
6. Cumulative boundaries round to whole logical pixels, so header and rows cannot drift.

**`drag(press_columns, avail, gap, column, delta)`** from the press snapshot: the column touching
the divider on the gaining side grows, the losing side gives up space nearest first down to each
floor, and a gaining rigid column stops at its max. Rigid columns the drag touched store their new
px, every visible flex column stores its resolved px as its weight, and hidden flex weights scale
by the visible flex sum's factor. No slack means no change. Every visible column but the last gets
a divider.

## Structure

- `melodia-views` `ui/track_columns.rs` (new): `SPECS`, `resolve`, `drag`, conversions, `install`.
  Called from `boot/ui_setup/views.rs` beside `ui::chips::install`.
- `melodia-views` `ui/track_list_view.rs`: persistence over one `TrackColumns` value. Rust owns the
  default widths (`ColumnWidths::default()`) and each view's default visible set.
- `models.slint`: `TrackColumns`, `TrackColumnLayout`, `enum TrackColumn`.
- `globals/track-columns.slint` (new): `TrackColumnSizing`, two `pure callback`s, exported from
  `app-window.slint`.
- `components/track-list/`: both lists take `columns` and resolve one `layout`; header and row
  cells are placed by `x`/`width`, never inside a `HorizontalLayout`, so the width-derived layout
  can't fold into the list's own `layout_info` and loop.
- Nine globals, nine list mounts and nine popup mounts each collapse to one binding. The global's
  property is `track-columns`, Favorites, Recently Played and Browse already owning an `int`
  `columns` for their grids; the components keep `columns`.
- Deleted with the scroll: `outer-scroll`, `h-*`, `reserve-scrollbar-lane`, the horizontal bars in
  `TrackListScrollbars` and `CompositeScrollbars`, Search's sibling bar, the composite hosts' lane
  term and Browse's card-mode zeroing.

## Phases

- [x] **Phase 1**: implement, retire the pins whose subject is deleted, stop at `cargo fmt` +
  clippy, and confirm in the generated `app-window.rs` that no list's horizontal `layoutinfo`
  reads `layout` (header and row both report item-intrinsic or constant info).
- [ ] **Manual test** (Kenan): resize from maximized to the miniplayer threshold with the sidebar
  collapsed and wide; drag every divider both ways; toggle columns; restart; Playlist Detail
  reorder; Artist Detail and Browse scrolling; Search's Songs section.
- [ ] **Phase 2**: `resolve`/`drag` unit tests, pins against the scroller and layout coming back,
  and the `CLAUDE.md` / `ui-patterns.md` / `slint-pitfalls.md` updates.
