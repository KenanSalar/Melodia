use super::{IndexRows, chunk_chips_to_rows, chunk_indices, rows_to_model};
use slint::{Model, SharedString};

/// `estimated_chip_width` is `chars * 6.5 + 24`, so a 4-char chip measures
/// 50 px and two of them plus the 8 px gap need 108.
fn chips(texts: &[&str]) -> Vec<SharedString> {
    texts.iter().map(|t| SharedString::from(*t)).collect()
}

fn shapes(rows: &[Vec<SharedString>]) -> Vec<Vec<&str>> {
    rows.iter().map(|row| row.iter().map(SharedString::as_str).collect()).collect()
}

#[test]
fn no_chips_means_no_rows() {
    assert!(chunk_chips_to_rows(&[], 500.0, None).is_empty());
    assert!(chunk_chips_to_rows(&chips(&["FLAC"]), 500.0, Some(0)).is_empty());
}

#[test]
fn an_unlaid_out_strip_collapses_to_one_row() {
    // Zero width is the frame before the mount Timer fires. Everything goes in
    // one row rather than one row per chip — the real width lands immediately
    // after, and a momentary tall stack would reflow whatever sits above.
    let rows = chunk_chips_to_rows(&chips(&["FLAC", "2020", "Rock"]), 0.0, None);
    assert_eq!(shapes(&rows), vec![vec!["FLAC", "2020", "Rock"]]);
}

#[test]
fn a_roomy_strip_keeps_one_row() {
    let rows = chunk_chips_to_rows(&chips(&["FLAC", "2020"]), 500.0, None);
    assert_eq!(shapes(&rows), vec![vec!["FLAC", "2020"]]);
}

#[test]
fn an_uncapped_strip_wraps_rather_than_dropping() {
    // 108 px seats exactly two 4-char chips; 107 seats one.
    let rows = chunk_chips_to_rows(&chips(&["FLAC", "2020", "Rock"]), 107.0, None);
    assert_eq!(shapes(&rows), vec![vec!["FLAC"], vec!["2020"], vec!["Rock"]]);
}

#[test]
fn a_capped_strip_wraps_up_to_the_cap_then_drops() {
    // The hero band's contract: wrap into the slack it has, then stop. Two is
    // what `hero_chips::HERO_MAX_ROWS` asks for.
    let rows = chunk_chips_to_rows(&chips(&["FLAC", "2020", "Rock", "Jazz"]), 107.0, Some(2));
    assert_eq!(shapes(&rows), vec![vec!["FLAC"], vec!["2020"]]);
}

#[test]
fn a_one_row_cap_drops_everything_past_the_first_row() {
    let rows = chunk_chips_to_rows(&chips(&["FLAC", "2020", "Rock"]), 108.0, Some(1));
    assert_eq!(shapes(&rows), vec![vec!["FLAC", "2020"]]);
}

#[test]
fn an_oversized_chip_still_gets_its_row_under_a_cap() {
    // A chip wider than the whole strip is emitted anyway — dropping every
    // chip because the first one is long would leave the band silently empty.
    let rows = chunk_chips_to_rows(&chips(&["a very long single chip"]), 20.0, Some(1));
    assert_eq!(shapes(&rows), vec![vec!["a very long single chip"]]);
}

#[test]
fn the_model_mirrors_the_row_shape() {
    let rows = vec![chips(&["FLAC", "2020"]), chips(&["Rock"])];
    let model = rows_to_model(rows);
    assert_eq!(model.row_count(), 2);
    let widths: Vec<usize> = model.iter().map(|row| row.row_count()).collect();
    assert_eq!(widths, vec![2, 1]);
}

#[test]
fn chunk_indices_fills_rows_left_to_right() {
    assert_eq!(chunk_indices(7, 3), vec![vec![0, 1, 2], vec![3, 4, 5], vec![6]]);
    assert_eq!(chunk_indices(6, 3), vec![vec![0, 1, 2], vec![3, 4, 5]]);
    assert_eq!(chunk_indices(2, 5), vec![vec![0, 1]]);
}

#[test]
fn chunk_indices_has_no_rows_for_nothing_to_place() {
    assert!(chunk_indices(0, 4).is_empty());
    assert!(chunk_indices(-3, 4).is_empty());
}

/// `per_row` comes from a measured width, which is zero for the frame before
/// the first layout reports one — so it has to floor at one item per row
/// rather than loop forever or divide by zero.
#[test]
fn chunk_indices_floors_a_degenerate_row_width_at_one() {
    assert_eq!(chunk_indices(3, 0), vec![vec![0], vec![1], vec![2]]);
    assert_eq!(chunk_indices(3, -1), vec![vec![0], vec![1], vec![2]]);
}

/// A repeater rebuilds every instance under a model it can't prove is the one it already has, and
/// it asks by pointer. Handing back a fresh model for an unchanged shape rebuilt every settings chip
/// and swatch on each frame of a resize drag.
#[test]
fn an_unchanged_row_shape_hands_back_the_model_already_mounted() {
    let rows = IndexRows::default();

    let first = rows.rows(7, 3);
    let again = rows.rows(7, 3);

    assert_eq!(first, again, "the same shape built a second model, so its strip rebuilds");
}

#[test]
fn a_row_width_seating_fewer_items_gets_a_model_of_its_own() {
    let rows = IndexRows::default();
    let wider = rows.rows(7, 4);

    let narrower = rows.rows(7, 3);

    assert_ne!(narrower, wider, "a narrower strip kept the wider strip's rows");
}

#[test]
fn a_different_item_count_gets_a_model_of_its_own() {
    let rows = IndexRows::default();
    let longer = rows.rows(7, 3);

    let shorter = rows.rows(6, 3);

    assert_ne!(shorter, longer, "a shorter option list kept the longer list's rows");
}

// Every width past the item count is the one row, so a drag through them shares one model.
#[test]
fn every_row_width_seating_all_items_shares_one_model() {
    let rows = IndexRows::default();

    assert_eq!(rows.rows(4, 4), rows.rows(4, 5));
    assert_eq!(rows.rows(4, 4), rows.rows(4, 40));
}

// The fold's edge from below: one short of the item count still wraps, so it can't share the
// single row's model.
#[test]
fn a_row_width_one_short_of_every_item_is_a_shape_of_its_own() {
    let rows = IndexRows::default();
    let single_row = rows.rows(4, 4);

    let wrapped = rows.rows(4, 3);

    assert_ne!(wrapped, single_row, "a strip that wraps was handed the unwrapped single row");
}

#[test]
fn no_items_share_one_model_whatever_the_width() {
    let rows = IndexRows::default();

    assert_eq!(rows.rows(0, 1), rows.rows(0, 8));
}

#[test]
fn the_model_for_no_items_has_no_rows() {
    let rows = IndexRows::default();

    let model = rows.rows(0, 4);

    assert_eq!(model.row_count(), 0);
}

// A count below zero is malformed rather than reachable, and `chunk_indices` reads it as nothing.
#[test]
fn a_negative_item_count_shares_the_empty_model() {
    let rows = IndexRows::default();
    let empty = rows.rows(0, 4);

    let negative = rows.rows(-3, 4);

    assert_eq!(negative, empty, "a negative count built rows of its own");
}

// The floor `chunk_indices` applies, taken before the lookup so a degenerate width is one entry.
#[test]
fn a_degenerate_row_width_shares_the_one_per_row_model() {
    let rows = IndexRows::default();

    assert_eq!(rows.rows(3, 1), rows.rows(3, 0));
    assert_eq!(rows.rows(3, 1), rows.rows(3, -1));
}

#[test]
fn a_shared_model_still_mirrors_its_shape() {
    let rows = IndexRows::default();

    let model = rows.rows(7, 3);
    let widths: Vec<usize> = model.iter().map(|row| row.row_count()).collect();

    assert_eq!(widths, vec![3, 3, 1]);
}
