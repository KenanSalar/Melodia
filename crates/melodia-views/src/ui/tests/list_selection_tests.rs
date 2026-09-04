use std::collections::HashSet;

use super::{compute_click_selection, restamp_selected};
use melodia_ui::TrackListRow as UiTrackListRow;

/// Displayed order used by every shift-range case below.
const VISIBLE: [i32; 6] = [10, 20, 30, 40, 50, 60];

fn visible() -> Vec<i32> {
    VISIBLE.to_vec()
}

#[test]
fn plain_click_selects_single_row_and_moves_anchor() {
    let (sel, anchor) = compute_click_selection(2, vec![10, 20], visible, 4, 50, false, false);
    assert_eq!(sel, vec![50]);
    assert_eq!(anchor, 4);
}

#[test]
fn ctrl_click_adds_unselected_row() {
    let (sel, anchor) = compute_click_selection(0, vec![10], visible, 3, 40, false, true);
    assert_eq!(sel, vec![10, 40]);
    assert_eq!(anchor, 3);
}

#[test]
fn ctrl_click_removes_selected_row() {
    let (sel, anchor) = compute_click_selection(0, vec![10, 40], visible, 3, 40, false, true);
    assert_eq!(sel, vec![10]);
    assert_eq!(anchor, 3);
}

#[test]
fn shift_click_selects_range_and_keeps_anchor() {
    let (sel, anchor) = compute_click_selection(1, vec![20], visible, 4, 50, true, false);
    assert_eq!(sel, vec![20, 30, 40, 50]);
    assert_eq!(anchor, 1);
}

#[test]
fn shift_click_range_works_upward() {
    let (sel, anchor) = compute_click_selection(4, vec![50], visible, 1, 20, true, false);
    assert_eq!(sel, vec![20, 30, 40, 50]);
    assert_eq!(anchor, 4);
}

#[test]
fn shift_click_without_anchor_falls_back_to_single() {
    let (sel, anchor) = compute_click_selection(-1, vec![], visible, 2, 30, true, false);
    assert_eq!(sel, vec![30]);
    assert_eq!(anchor, 2);
}

#[test]
fn shift_click_on_empty_visible_list_falls_back_to_single() {
    let (sel, anchor) = compute_click_selection(1, vec![20], Vec::new, 3, 40, true, false);
    assert_eq!(sel, vec![40]);
    assert_eq!(anchor, 3);
}

#[test]
fn shift_click_clamps_out_of_range_indices() {
    // Anchor beyond the (shrunken) visible list clamps to the last row
    // instead of panicking on a bad slice bound.
    let (sel, anchor) = compute_click_selection(99, vec![60], visible, 4, 50, true, false);
    assert_eq!(sel, vec![50, 60]);
    assert_eq!(anchor, 99);
}

#[test]
fn shift_takes_priority_over_ctrl_when_anchored() {
    let (sel, _) = compute_click_selection(0, vec![10], visible, 2, 30, true, true);
    assert_eq!(sel, vec![10, 20, 30]);
}

#[test]
fn visible_ids_is_not_invoked_on_plain_or_ctrl_clicks() {
    let (sel, _) = compute_click_selection(
        0,
        vec![10],
        || unreachable!("plain click must not walk the visible rows"),
        2,
        30,
        false,
        false,
    );
    assert_eq!(sel, vec![30]);
}

#[test]
fn restamp_marks_only_selected_rows() {
    let mut rows: Vec<UiTrackListRow> = (0..3)
        .map(|i| UiTrackListRow {
            id: i,
            ..Default::default()
        })
        .collect();
    let selected: HashSet<i32> = [1].into_iter().collect();
    restamp_selected(&mut rows, &selected);
    assert!(!rows[0].selected);
    assert!(rows[1].selected);
    assert!(!rows[2].selected);
}

#[test]
fn restamp_with_empty_set_is_a_no_op() {
    let mut rows = vec![UiTrackListRow {
        id: 7,
        selected: true,
        ..Default::default()
    }];
    restamp_selected(&mut rows, &HashSet::new());
    // Empty set early-returns without clearing existing flags — restamp runs
    // on freshly-built (all-false) rows; clearing is stamp_rows_selected's job.
    assert!(rows[0].selected);
}
