use slint::Model;

use super::*;

/// Read a chunked result back as plain nested `Vec`s.
fn shape(rows: &[ModelRc<i32>]) -> Vec<Vec<i32>> {
    rows.iter().map(|r| r.iter().collect()).collect()
}

/// Moving is the part that can silently reorder: a chunk built back-to-front or a `by_ref`
/// dropped between rows scrambles cards **within** a row while the row count, the total and
/// the last row's shortness all stay right.
#[test]
fn fills_rows_left_to_right_and_leaves_the_last_short() {
    let cases: [(i32, Vec<Vec<i32>>); 3] = [
        (1, vec![vec![1], vec![2], vec![3], vec![4], vec![5]]),
        (2, vec![vec![1, 2], vec![3, 4], vec![5]]),
        (3, vec![vec![1, 2, 3], vec![4, 5]]),
    ];
    for (columns, expected) in cases {
        let rows = chunk_built_rows(vec![1, 2, 3, 4, 5], columns, |m| m);

        assert_eq!(shape(&rows), expected, "columns={columns}");
    }
}

/// An empty input must terminate rather than push an empty row forever.
#[test]
fn nothing_to_place_is_no_rows() {
    let rows = chunk_built_rows(Vec::<i32>::new(), 3, |m| m);

    assert!(rows.is_empty());
}

/// A grid mid-layout can report zero columns. One card per row is a visible wrong; a
/// `take(0)` is a loop that never ends.
#[test]
fn a_degenerate_column_count_floors_at_one() {
    for columns in [0, -4] {
        let rows = chunk_built_rows(vec![1, 2, 3], columns, |m| m);

        assert_eq!(shape(&rows), vec![vec![1], vec![2], vec![3]], "columns={columns}");
    }
}
