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
/// the two Favorites tabs chunk already-built rows. Both go through the same
/// call, which is the whole point of the `card` parameter — so a chunk of
/// indices must come back as the cards those indices name, in that order.
#[test]
fn the_card_projection_runs_per_item() {
    let data = [10, 20, 30];
    let rows = chunk_rows(&[2usize, 0, 1], 2, |&i| data[i], |m| m);
    assert_eq!(shape(&rows), vec![vec![30, 10], vec![20]]);
}
