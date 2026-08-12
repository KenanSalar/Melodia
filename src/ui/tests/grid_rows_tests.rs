use super::*;

/// Read a chunked result back as plain nested `Vec`s.
fn shape(rows: &[ModelRc<i32>]) -> Vec<Vec<i32>> {
    rows.iter().map(|r| r.iter().collect()).collect()
}

#[test]
fn fills_rows_left_to_right_and_leaves_the_last_short() {
    let rows = chunk_rows(&[1, 2, 3, 4, 5], 2, |&n| n, |m| m);
    assert_eq!(shape(&rows), vec![vec![1, 2], vec![3, 4], vec![5]]);
}

#[test]
fn nothing_to_place_is_no_rows() {
    let rows = chunk_rows::<i32, i32, _>(&[], 3, |&n| n, |m| m);
    assert!(rows.is_empty());
}

/// A grid mid-layout can report zero columns. One card per row is a visible
/// wrong; a zero-width `chunks` is a panic.
#[test]
fn a_degenerate_column_count_floors_at_one() {
    for columns in [0, -4] {
        let rows = chunk_rows(&[1, 2, 3], columns, |&n| n, |m| m);
        assert_eq!(shape(&rows), vec![vec![1], vec![2], vec![3]]);
    }
}

/// The four entity grids chunk *indices* and project through their `GridData`;
/// the three grid tabs and Browse chunk already-built rows. The borrowing form
/// is the first group's, which is the whole point of the `card` parameter — so a
/// chunk of indices must come back as the cards those indices name, in that
/// order.
#[test]
fn the_card_projection_runs_per_item() {
    let data = [10, 20, 30];
    let rows = chunk_rows(&[2usize, 0, 1], 2, |&i| data[i], |m| m);
    assert_eq!(shape(&rows), vec![vec![30, 10], vec![20]]);
}

// --- chunk_built_rows (the owning form) ---------------------------------

/// The owning form places cards exactly where the borrowing one does.
///
/// It exists to *move* its input rather than clone it, and moving is the part
/// that can silently reorder: the borrowing form indexes with `chunks`, where
/// this one drains an iterator, so a chunk built back-to-front or a `by_ref`
/// dropped between rows scrambles cards **within** a row while the row count,
/// the total and the last row's shortness all stay right. Comparing the two
/// forms over the same input is what makes that visible rather than plausible.
#[test]
fn the_owning_form_places_cards_exactly_as_the_borrowing_one_does() {
    for columns in [1, 2, 3, 5, 7] {
        let cards = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let expect = shape(&chunk_rows(&cards, columns, |&n| n, |m| m));
        let got = shape(&chunk_built_rows(cards, columns, |m| m));
        assert_eq!(got, expect, "columns={columns}");
    }
}

/// The two degenerate inputs the borrowing form is already held to, because a
/// drain loop fails them differently: an empty input must terminate rather than
/// push an empty row forever, and a zero column count must floor at one rather
/// than take(0) and spin.
#[test]
fn the_owning_form_handles_an_empty_input_and_a_degenerate_column_count() {
    let none = chunk_built_rows(Vec::<i32>::new(), 3, |m| m);
    assert!(none.is_empty());

    for columns in [0, -4] {
        let rows = chunk_built_rows(vec![1, 2, 3], columns, |m| m);
        assert_eq!(shape(&rows), vec![vec![1], vec![2], vec![3]], "columns={columns}");
    }
}
