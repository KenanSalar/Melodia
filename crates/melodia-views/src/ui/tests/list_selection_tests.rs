use std::collections::HashSet;

use slint::{Model, ModelExt, ModelRc, VecModel};

use super::{compute_click_selection, displayed_ids, restamp_selected, select_all_ids};
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
    let mut rows: Vec<UiTrackListRow> =
        (0..3).map(|i| UiTrackListRow { id: i, ..Default::default() }).collect();
    let selected: HashSet<i32> = [1].into_iter().collect();
    restamp_selected(&mut rows, &selected);
    assert!(!rows[0].selected);
    assert!(rows[1].selected);
    assert!(!rows[2].selected);
}

#[test]
fn restamp_with_empty_set_is_a_no_op() {
    let mut rows = vec![UiTrackListRow { id: 7, selected: true, ..Default::default() }];
    restamp_selected(&mut rows, &HashSet::new());
    // Empty set early-returns without clearing existing flags — restamp runs
    // on freshly-built (all-false) rows; clearing is stamp_rows_selected's job.
    assert!(rows[0].selected);
}

fn row(id: i32, enabled: bool, selected: bool) -> UiTrackListRow {
    UiTrackListRow { id, enabled, selected, ..Default::default() }
}

/// Browse's disk-only rows carry both halves of the predicate at once, so one of them going
/// missing looks like nothing until a folder holding an unimported file is selected.
#[test]
fn a_disk_only_row_is_not_something_a_selection_can_hold() {
    let rows: ModelRc<UiTrackListRow> = ModelRc::new(VecModel::from(vec![
        row(10, true, false),
        row(0, false, false),
        row(20, false, false),
        row(0, true, false),
        row(30, true, false),
    ]));
    assert_eq!(displayed_ids(&rows), [10, 30]);
}

/// The half [`displayed_ids`] cannot do, and the reason `select_all_ids` exists: a disabled row
/// left stamped from an earlier pick reads as selected while its id is not in the set.
#[test]
fn select_all_stamps_the_rows_it_keeps_and_unstamps_the_rest() {
    let model = VecModel::from(vec![
        row(10, true, false),
        row(0, false, true),
        row(20, true, true),
        row(30, false, true),
    ]);
    let rows: ModelRc<UiTrackListRow> = ModelRc::new(model);

    assert_eq!(select_all_ids(&rows), [10, 20]);

    let stamped: Vec<bool> = rows.iter().map(|r| r.selected).collect();
    assert_eq!(stamped, [true, false, true, false]);
}

#[test]
fn select_all_on_an_empty_model_yields_nothing() {
    let rows: ModelRc<UiTrackListRow> = ModelRc::new(VecModel::<UiTrackListRow>::default());
    assert_eq!(select_all_ids(&rows), Vec::<i32>::new());
}

/// The downcast-miss arm. It stamps nothing, which is `stamp_rows_selected`'s own early return,
/// but it still owes the caller the ids or Select All silently selects an empty set.
#[test]
fn a_model_that_is_not_a_vec_model_still_answers_with_its_ids() {
    let rows: ModelRc<UiTrackListRow> = ModelRc::new(
        VecModel::from(vec![row(10, true, false), row(0, true, false), row(20, true, false)])
            .map(|r: UiTrackListRow| r),
    );
    assert!(
        rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>().is_none(),
        "the fixture has to miss the downcast or it tests the arm above"
    );
    assert_eq!(select_all_ids(&rows), [10, 20]);
}
