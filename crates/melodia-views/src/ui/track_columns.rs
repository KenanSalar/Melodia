//! Column geometry for every track list: where a page's stored columns sit at one width, and what
//! a divider drag leaves stored.
//!
//! A rigid column (`#`, Year, Length) keeps its stored pixel width while there is room for it. A
//! flex column (Title, Artist, Album, Genre) stores the width it had when it was last sized and
//! reads it as a weight, so the four split whatever the rigid ones leave in the proportion the user
//! set. The list's width only ever feeds [`resolve`]. Nothing is stored on a resize, which is what
//! makes one reversible; only [`drag`] changes what `views.json` keeps.
//!
//! No column is ever dropped to make room. Once the list is narrower than every visible column's
//! floor combined, they all go below their floors together and their text elides.

use melodia_ui::{AppWindow, TrackColumn, TrackColumnLayout, TrackColumnSizing, TrackColumns};
use slint::ComponentHandle;

const COLUMN_COUNT: usize = 7;

// Display order, and the index into every per-column array here.
const NUMBER: usize = 0;
const TITLE: usize = 1;
const ARTIST: usize = 2;
const ALBUM: usize = 3;
const GENRE: usize = 4;
const YEAR: usize = 5;
const LENGTH: usize = 6;

/// Past this a rigid column's content is padding; the room belongs to the flex columns.
const RIGID_MAX: f32 = 200.0;

/// Under this, two rigid widths differ only by float error.
const RESIZE_EPSILON: f32 = 0.01;

#[derive(Clone, Copy)]
enum Kind {
    Rigid,
    Flex,
}

#[derive(Clone, Copy)]
struct Spec {
    kind: Kind,
    /// The narrowest the column goes while the list can seat every visible floor.
    floor: f32,
}

// Title's floor seats the 36 px cover, the favourite heart and a few characters of the name.
const SPECS: [Spec; COLUMN_COUNT] = [
    Spec { kind: Kind::Rigid, floor: 40.0 },
    Spec { kind: Kind::Flex, floor: 150.0 },
    Spec { kind: Kind::Flex, floor: 60.0 },
    Spec { kind: Kind::Flex, floor: 60.0 },
    Spec { kind: Kind::Flex, floor: 60.0 },
    Spec { kind: Kind::Rigid, floor: 50.0 },
    Spec { kind: Kind::Rigid, floor: 56.0 },
];

/// One page's columns in display order. Title and Length are always visible.
#[derive(Clone, Copy)]
struct Columns {
    widths: [f32; COLUMN_COUNT],
    visible: [bool; COLUMN_COUNT],
}

/// Where each visible column sits; a hidden one keeps zeros.
#[derive(Clone, Copy, Default)]
struct Placement {
    x: [f32; COLUMN_COUNT],
    width: [f32; COLUMN_COUNT],
}

/// Where `columns` sits across a list `avail` wide, with `gap` at each end and between cells.
fn resolve(columns: &Columns, avail: f32, gap: f32) -> Placement {
    place(columns, &sized(columns, avail, gap), gap)
}

/// The columns a divider drag leaves stored. The divider is `column`'s right edge, moved `delta`
/// from where it stood when `pressed` was taken.
///
/// The column against the divider on the side gaining room takes all of it, so the divider stays
/// under the pointer. The side losing room gives it up nearest first, each column only down to its
/// floor, and a rigid column stops growing at its max.
fn drag(pressed: &Columns, avail: f32, gap: f32, column: usize, delta: f32) -> Columns {
    let before = sized(pressed, avail, gap);
    let shown: Vec<usize> = shown(pressed).collect();
    let Some(at) = shown.iter().position(|&i| i == column) else {
        return *pressed;
    };
    let (left, right) = shown.split_at(at + 1);
    // The last visible column has no divider.
    let Some(&neighbour) = right.first() else {
        return *pressed;
    };

    let mut after = before;
    let moved = if delta > 0.0 {
        let wanted = delta.min(headroom(column, before[column]));
        let moved = give_up(&mut after, right.iter().copied(), wanted);
        after[column] += moved;
        moved
    } else {
        let wanted = (-delta).min(headroom(neighbour, before[neighbour]));
        let moved = give_up(&mut after, left.iter().rev().copied(), wanted);
        after[neighbour] += moved;
        moved
    };

    if moved <= 0.0 {
        return *pressed;
    }
    stored_after(pressed, &before, &after)
}

/// Each visible column's width at `avail`, before rounding.
fn sized(columns: &Columns, avail: f32, gap: f32) -> [f32; COLUMN_COUNT] {
    let content = content_width(columns, avail, gap).max(0.0);
    let floors: f32 = shown(columns).map(|i| SPECS[i].floor).sum();
    let mut widths = [0.0; COLUMN_COUNT];

    if content < floors {
        for i in shown(columns) {
            widths[i] = SPECS[i].floor * content / floors;
        }
        return widths;
    }

    for i in rigid_shown(columns) {
        widths[i] = stored_width(columns, i).min(RIGID_MAX);
    }
    let rigid: f32 = rigid_shown(columns).map(|i| widths[i]).sum();
    let flex_floors: f32 = flex_shown(columns).map(|i| SPECS[i].floor).sum();
    let flex_room = content - rigid;

    if flex_room >= flex_floors {
        share_by_weight(columns, &mut widths, flex_room);
    } else {
        for i in flex_shown(columns) {
            widths[i] = SPECS[i].floor;
        }
        shrink_rigid(columns, &mut widths, flex_floors - flex_room);
    }
    widths
}

/// The width the visible columns share: `avail` less one gap per column plus one, which is a gap
/// at each end and one between each pair.
fn content_width(columns: &Columns, avail: f32, gap: f32) -> f32 {
    let gaps: f32 = shown(columns).map(|_| gap).sum();
    avail - gap - gaps
}

/// Splits `room` across the visible flex columns by weight. A column whose share would fall under
/// its floor is pinned there and the rest is shared again. `room` covers every flex floor.
fn share_by_weight(columns: &Columns, widths: &mut [f32; COLUMN_COUNT], room: f32) {
    let mut pinned = [false; COLUMN_COUNT];
    loop {
        let mut weight = 0.0;
        let mut taken = 0.0;
        for i in flex_shown(columns) {
            if pinned[i] {
                taken += SPECS[i].floor;
            } else {
                weight += stored_width(columns, i);
            }
        }
        if weight <= 0.0 {
            break;
        }

        let per_weight = (room - taken) / weight;
        let mut pinned_any = false;
        for i in flex_shown(columns) {
            if !pinned[i] && stored_width(columns, i) * per_weight < SPECS[i].floor {
                pinned[i] = true;
                pinned_any = true;
            }
        }
        if !pinned_any {
            for i in flex_shown(columns) {
                widths[i] =
                    if pinned[i] { SPECS[i].floor } else { stored_width(columns, i) * per_weight };
            }
            return;
        }
    }

    for i in flex_shown(columns) {
        widths[i] = SPECS[i].floor;
    }
}

/// Takes `shortfall` off the visible rigid columns in proportion to how far each sits above its
/// floor. [`sized`] only calls this once every floor fits, so the slack always covers it.
fn shrink_rigid(columns: &Columns, widths: &mut [f32; COLUMN_COUNT], shortfall: f32) {
    let slack: f32 = rigid_shown(columns).map(|i| widths[i] - SPECS[i].floor).sum();
    if slack <= 0.0 {
        return;
    }
    for i in rigid_shown(columns) {
        widths[i] -= shortfall * (widths[i] - SPECS[i].floor) / slack;
    }
}

/// Rounds each column's edges rather than its width, so the widths still add up to what was sized
/// and the header and every row land on the same pixels.
fn place(columns: &Columns, widths: &[f32; COLUMN_COUNT], gap: f32) -> Placement {
    let mut placement = Placement::default();
    let mut edge = gap;
    for i in shown(columns) {
        let left = edge.round();
        edge += widths[i];
        placement.x[i] = left;
        placement.width[i] = (edge.round() - left).max(0.0);
        edge += gap;
    }
    placement
}

/// How much further column `i` may grow from `width`.
fn headroom(i: usize, width: f32) -> f32 {
    match SPECS[i].kind {
        Kind::Rigid => (RIGID_MAX - width).max(0.0),
        Kind::Flex => f32::INFINITY,
    }
}

/// Takes up to `wanted` from the columns in `from`, in that order, each only down to its floor.
/// Returns what it took.
fn give_up(
    widths: &mut [f32; COLUMN_COUNT],
    from: impl Iterator<Item = usize>,
    wanted: f32,
) -> f32 {
    let mut taken = 0.0;
    for i in from {
        let remaining = wanted - taken;
        if remaining <= 0.0 {
            break;
        }
        let take = remaining.min((widths[i] - SPECS[i].floor).max(0.0));
        widths[i] -= take;
        taken += take;
    }
    taken
}

/// What a drag stores. A rigid column it resized keeps its new width and one it didn't keeps what
/// it had, since a width squeezed by a narrow window is not one the user chose. Every visible flex
/// column keeps its width as its weight, and a hidden flex weight scales with them so the column
/// comes back in the same proportion.
///
/// **Unless the list was already squeezing the rigid columns at the press.** Then every visible
/// one keeps what the drag left: [`resolve`] seats rigid widths first, so restoring theirs hands
/// them back the room the drag gave a flex column, and a different column grows under the pointer.
fn stored_after(
    pressed: &Columns,
    before: &[f32; COLUMN_COUNT],
    after: &[f32; COLUMN_COUNT],
) -> Columns {
    let old_weight: f32 = flex_shown(pressed).map(|i| stored_width(pressed, i)).sum();
    let new_weight: f32 = flex_shown(pressed).map(|i| after[i]).sum();
    let hidden_scale = if old_weight > 0.0 { new_weight / old_weight } else { 1.0 };
    let rigid_squeezed = rigid_shown(pressed)
        .any(|i| before[i] + RESIZE_EPSILON < stored_width(pressed, i).min(RIGID_MAX));

    let mut stored = *pressed;
    for i in 0..COLUMN_COUNT {
        match (SPECS[i].kind, pressed.visible[i]) {
            (Kind::Rigid, true)
                if rigid_squeezed || (after[i] - before[i]).abs() > RESIZE_EPSILON =>
            {
                stored.widths[i] = after[i];
            }
            (Kind::Rigid, _) => {}
            (Kind::Flex, true) => stored.widths[i] = after[i],
            (Kind::Flex, false) => stored.widths[i] = stored_width(pressed, i) * hidden_scale,
        }
    }
    stored
}

/// The stored width of column `i`, floored, reading anything a hand-edited file could carry that
/// isn't a width as the floor.
fn stored_width(columns: &Columns, i: usize) -> f32 {
    let width = columns.widths[i];
    if width.is_finite() { width.max(SPECS[i].floor) } else { SPECS[i].floor }
}

fn shown(columns: &Columns) -> impl Iterator<Item = usize> {
    (0..COLUMN_COUNT).filter(|&i| columns.visible[i])
}

fn rigid_shown(columns: &Columns) -> impl Iterator<Item = usize> {
    shown(columns).filter(|&i| matches!(SPECS[i].kind, Kind::Rigid))
}

fn flex_shown(columns: &Columns) -> impl Iterator<Item = usize> {
    shown(columns).filter(|&i| matches!(SPECS[i].kind, Kind::Flex))
}

fn index_of(column: TrackColumn) -> usize {
    match column {
        TrackColumn::Number => NUMBER,
        TrackColumn::Title => TITLE,
        TrackColumn::Artist => ARTIST,
        TrackColumn::Album => ALBUM,
        TrackColumn::Genre => GENRE,
        TrackColumn::Year => YEAR,
        TrackColumn::Length => LENGTH,
    }
}

impl From<&TrackColumns> for Columns {
    fn from(columns: &TrackColumns) -> Self {
        Self {
            widths: [
                columns.w_number,
                columns.w_title,
                columns.w_artist,
                columns.w_album,
                columns.w_genre,
                columns.w_year,
                columns.w_length,
            ],
            visible: [
                columns.show_number,
                true,
                columns.show_artist,
                columns.show_album,
                columns.show_genre,
                columns.show_year,
                true,
            ],
        }
    }
}

impl From<Placement> for TrackColumnLayout {
    fn from(placement: Placement) -> Self {
        let Placement { x, width } = placement;
        Self {
            number_x: x[NUMBER],
            number_w: width[NUMBER],
            title_x: x[TITLE],
            title_w: width[TITLE],
            artist_x: x[ARTIST],
            artist_w: width[ARTIST],
            album_x: x[ALBUM],
            album_w: width[ALBUM],
            genre_x: x[GENRE],
            genre_w: width[GENRE],
            year_x: x[YEAR],
            year_w: width[YEAR],
            length_x: x[LENGTH],
            length_w: width[LENGTH],
        }
    }
}

/// `columns` with its seven widths replaced, visibility and artwork left as they were.
fn with_widths(columns: &TrackColumns, widths: &[f32; COLUMN_COUNT]) -> TrackColumns {
    TrackColumns {
        w_number: widths[NUMBER],
        w_title: widths[TITLE],
        w_artist: widths[ARTIST],
        w_album: widths[ALBUM],
        w_genre: widths[GENRE],
        w_year: widths[YEAR],
        w_length: widths[LENGTH],
        ..*columns
    }
}

/// Wires `TrackColumnSizing`. Call once during startup, before the window shows: a list resolved
/// ahead of the handler caches an empty layout until its width next moves.
pub fn install(ui: &AppWindow) {
    let sizing = ui.global::<TrackColumnSizing>();
    sizing.on_resolve(|columns, avail, gap| resolve(&Columns::from(&columns), avail, gap).into());
    sizing.on_drag(|pressed, avail, gap, column, delta| {
        let dragged = drag(&Columns::from(&pressed), avail, gap, index_of(column), delta);
        with_widths(&pressed, &dragged.widths)
    });
}
