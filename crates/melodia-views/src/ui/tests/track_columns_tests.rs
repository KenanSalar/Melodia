//! The column layout every track list resolves through: the three ways a list's width is shared,
//! the boundaries between them, and what a divider drag leaves stored.
//!
//! The stored widths are spelled here rather than read off `ColumnWidths::default()`, so a retune
//! of the defaults moves no expected number below.

use super::*;

const GAP: f32 = 8.0;

/// Far under a pixel, and far over the float error the arithmetic below accumulates.
const TOLERANCE: f32 = 1e-3;

/// `#`, Title, Artist, Album, Genre, Year, Length.
const STORED: [f32; COLUMN_COUNT] = [56.0, 320.0, 200.0, 220.0, 140.0, 72.0, 88.0];

const ALL_VISIBLE: [bool; COLUMN_COUNT] = [true; COLUMN_COUNT];

/// Favorites and Recently Played: no `#`, Genre or Year.
const FAVORITES_VISIBLE: [bool; COLUMN_COUNT] = [false, true, true, true, false, false, true];

fn columns(widths: [f32; COLUMN_COUNT], visible: [bool; COLUMN_COUNT]) -> Columns {
    Columns { widths, visible }
}

fn stored() -> Columns {
    columns(STORED, ALL_VISIBLE)
}

fn with_width(index: usize, width: f32) -> Columns {
    let mut widths = STORED;
    widths[index] = width;
    columns(widths, ALL_VISIBLE)
}

/// A [`Placement`] that prints and compares, rounded edges being whole pixels.
#[derive(Debug, PartialEq)]
struct Seated {
    x: [f32; COLUMN_COUNT],
    width: [f32; COLUMN_COUNT],
}

fn seated(columns: &Columns, avail: f32) -> Seated {
    let Placement { x, width } = resolve(columns, avail, GAP);
    Seated { x, width }
}

fn close(actual: &[f32; COLUMN_COUNT], expected: &[f32; COLUMN_COUNT]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < TOLERANCE)
}

// --- resolve ---------------------------------------------------------------------

#[test]
fn a_roomy_list_shares_what_the_rigid_columns_leave_by_weight() {
    assert_eq!(
        seated(&stored(), 1000.0),
        Seated {
            x: [8.0, 72.0, 342.0, 513.0, 701.0, 824.0, 904.0],
            width: [56.0, 262.0, 163.0, 180.0, 115.0, 72.0, 88.0],
        }
    );
}

#[test]
fn a_list_short_of_the_rigid_widths_squeezes_only_the_rigid_columns() {
    assert_eq!(
        seated(&stored(), 580.0),
        Seated {
            x: [8.0, 65.0, 223.0, 291.0, 359.0, 427.0, 498.0],
            width: [49.0, 150.0, 60.0, 60.0, 60.0, 63.0, 74.0],
        }
    );
}

#[test]
fn a_list_short_of_every_floor_shrinks_every_column_by_one_ratio() {
    // Exactly half the floors, so exactly half of each.
    assert_eq!(
        seated(&stored(), 302.0),
        Seated {
            x: [8.0, 36.0, 119.0, 157.0, 195.0, 233.0, 266.0],
            width: [20.0, 75.0, 30.0, 30.0, 30.0, 25.0, 28.0],
        }
    );
}

#[test]
fn the_columns_meet_their_floors_together_at_540() {
    let cases = [
        (539.0, [39.9160, 149.6849, 59.8739, 59.8739, 59.8739, 49.8950, 55.8824]),
        (540.0, [40.0, 150.0, 60.0, 60.0, 60.0, 50.0, 56.0]),
        (541.0, [40.2286, 150.0, 60.0, 60.0, 60.0, 50.3143, 56.4571]),
    ];
    for (avail, expected) in cases {
        let widths = sized(&stored(), avail, GAP);
        assert!(close(&widths, &expected), "at {avail}: {widths:?}");
    }
}

#[test]
fn the_rigid_columns_reach_their_stored_widths_at_610() {
    let cases = [
        (609.0, [55.7714, 150.0, 60.0, 60.0, 60.0, 71.6857, 87.5429]),
        (610.0, [56.0, 150.0, 60.0, 60.0, 60.0, 72.0, 88.0]),
        (611.0, [56.0, 150.0, 60.0, 61.0, 60.0, 72.0, 88.0]),
    ];
    for (avail, expected) in cases {
        let widths = sized(&stored(), avail, GAP);
        assert!(close(&widths, &expected), "at {avail}: {widths:?}");
    }
}

/// Below 692.5 Title's share falls under its floor, so it is pinned there and the other three
/// share what is left, each a little narrower than an unpinned split would make it.
#[test]
fn title_leaves_its_floor_at_692_5() {
    let cases = [
        (691.5, [56.0, 150.0, 93.3929, 102.7321, 65.375, 72.0, 88.0]),
        (692.5, [56.0, 150.0, 93.75, 103.125, 65.625, 72.0, 88.0]),
        (693.5, [56.0, 150.3636, 93.9773, 103.375, 65.7841, 72.0, 88.0]),
    ];
    for (avail, expected) in cases {
        let widths = sized(&stored(), avail, GAP);
        assert!(close(&widths, &expected), "at {avail}: {widths:?}");
    }
}

#[test]
fn a_hidden_column_takes_no_room_and_sits_at_zero() {
    assert_eq!(
        seated(&columns(STORED, FAVORITES_VISIBLE), 1000.0),
        Seated {
            x: [0.0, 8.0, 393.0, 637.0, 0.0, 0.0, 904.0],
            width: [0.0, 377.0, 236.0, 259.0, 0.0, 0.0, 88.0],
        }
    );
}

/// A width a hand-edited `views.json` could carry reads as the floor rather than as a weight that
/// takes the whole list or none of it.
#[test]
fn a_stored_width_that_is_not_a_width_reads_as_the_floor() {
    let at_floor = seated(&with_width(TITLE, 150.0), 1000.0);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 0.0] {
        assert_eq!(seated(&with_width(TITLE, bad), 1000.0), at_floor, "a stored {bad}");
    }
}

#[test]
fn a_rigid_column_stored_past_its_max_is_seated_at_the_max() {
    let widths = sized(&with_width(YEAR, 250.0), 1000.0, GAP);
    assert!((widths[YEAR] - RIGID_MAX).abs() < TOLERANCE, "{widths:?}");
}

// --- drag ------------------------------------------------------------------------

#[test]
fn a_divider_dragged_right_grows_its_column_and_takes_from_the_next() {
    let dragged = drag(&stored(), 1000.0, GAP, NUMBER, 10.0);

    let expected = [66.0, 251.8182, 163.6364, 180.0, 114.5455, 72.0, 88.0];
    assert!(close(&dragged.widths, &expected), "{:?}", dragged.widths);
    // The divider lands where the pointer took it: 64 at the press, 10 to the right.
    let Placement { x, width } = resolve(&dragged, 1000.0, GAP);
    assert!((x[NUMBER] + width[NUMBER] - 74.0).abs() < TOLERANCE);
}

#[test]
fn a_column_giving_up_room_stops_at_its_floor_and_the_next_gives_the_rest() {
    // 144 of the 200 fit under `#`'s max. Title has 111.8 above its floor, Artist the other 32.2.
    let dragged = drag(&stored(), 1000.0, GAP, NUMBER, 200.0);

    let expected = [200.0, 150.0, 131.4545, 180.0, 114.5455, 72.0, 88.0];
    assert!(close(&dragged.widths, &expected), "{:?}", dragged.widths);
}

#[test]
fn a_rigid_column_grows_no_wider_than_its_max() {
    let dragged = drag(&stored(), 2000.0, GAP, NUMBER, 300.0);

    let expected = [200.0, 481.4545, 390.9091, 430.0, 273.6364, 72.0, 88.0];
    assert!(close(&dragged.widths, &expected), "{:?}", dragged.widths);
}

#[test]
fn a_divider_dragged_left_grows_the_column_after_it() {
    // Length can take 112 before its max. Year gives 22, Genre 54.5, Album the last 35.5.
    let dragged = drag(&stored(), 1000.0, GAP, YEAR, -500.0);

    let expected = [56.0, 261.8182, 163.6364, 144.5455, 60.0, 50.0, 200.0];
    assert!(close(&dragged.widths, &expected), "{:?}", dragged.widths);
}

#[test]
fn a_drag_with_nowhere_to_take_room_from_stores_nothing() {
    // At 540 every column already sits on its floor.
    let dragged = drag(&stored(), 540.0, GAP, TITLE, 20.0);
    assert!(close(&dragged.widths, &STORED), "{:?}", dragged.widths);
}

#[test]
fn a_list_short_of_every_floor_ignores_a_drag() {
    let dragged = drag(&stored(), 500.0, GAP, TITLE, 20.0);
    assert!(close(&dragged.widths, &STORED), "{:?}", dragged.widths);
}

#[test]
fn the_last_column_has_no_divider_to_drag() {
    let dragged = drag(&stored(), 1000.0, GAP, LENGTH, -40.0);
    assert!(close(&dragged.widths, &STORED), "{:?}", dragged.widths);
}

#[test]
fn a_hidden_column_has_no_divider_to_drag() {
    let dragged = drag(&columns(STORED, FAVORITES_VISIBLE), 1000.0, GAP, GENRE, 40.0);
    assert!(close(&dragged.widths, &STORED), "{:?}", dragged.widths);
}

/// At 580 the rigid columns are already squeezed. Storing only the ones the drag moved handed `#`
/// back its full 56 on the next resolve, and Title shrank under the pointer to 163. The divider
/// sits at 235 either way, so only the widths show it.
#[test]
fn a_drag_in_a_squeezed_list_keeps_the_width_it_gave_the_column() {
    let dragged = drag(&stored(), 580.0, GAP, TITLE, 20.0);

    assert_eq!(
        seated(&dragged, 580.0),
        Seated {
            x: [8.0, 65.0, 243.0, 311.0, 379.0, 447.0, 505.0],
            width: [49.0, 170.0, 60.0, 60.0, 60.0, 50.0, 67.0],
        }
    );
}

/// A rigid width seated below what is stored is the list's doing, not the user's, so a drag that
/// doesn't move it has no business storing it.
#[test]
fn a_drag_leaves_a_rigid_width_it_did_not_move_as_it_was_stored() {
    let dragged = drag(&with_width(YEAR, 250.0), 1000.0, GAP, TITLE, 30.0);
    assert!((dragged.widths[YEAR] - 250.0).abs() < TOLERANCE, "{:?}", dragged.widths);
}

/// A hidden column's weight scales with the visible ones', so showing it again brings it back in
/// the proportion it was hidden at.
#[test]
fn a_hidden_flex_weight_scales_with_the_visible_ones() {
    // The visible flex weights sum 740 at the press and 1072 after it.
    let dragged = drag(&columns(STORED, FAVORITES_VISIBLE), 1200.0, GAP, TITLE, 30.0);
    assert!((dragged.widths[GENRE] - 202.8108).abs() < TOLERANCE, "{:?}", dragged.widths);
}
