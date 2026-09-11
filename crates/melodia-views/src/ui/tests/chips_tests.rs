use super::{chunk_chips_to_rows, chunk_indices, rows_to_model};
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
