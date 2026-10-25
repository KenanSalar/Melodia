use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{Model, ModelExt, ModelRc, VecModel};

use super::{
    NO_ANCHOR, RowSelectionView, clear_row_selection, compute_click_selection, displayed_ids,
    handle_row_click, keep_selection_within, restamp_selected, select_all_ids, select_all_rows,
};
use crate::ui::fixtures::track_keyset;
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use melodia_ui::TrackListRow as UiTrackListRow;

/// Displayed order used by every shift-range case below.
const VISIBLE: [i32; 6] = [10, 20, 30, 40, 50, 60];

const ROW_MENU: &str =
    include_str!("../../../../melodia-ui/ui/components/track-list/track-context-menu.slint");

fn visible() -> Vec<i32> {
    VISIBLE.to_vec()
}

#[test]
fn plain_click_selects_single_row_and_moves_anchor() {
    let (sel, anchor) = compute_click_selection(20, vec![10, 20], visible, 4, 50, false, false);
    assert_eq!(sel, vec![50]);
    assert_eq!(anchor, 50);
}

#[test]
fn ctrl_click_adds_unselected_row() {
    let (sel, anchor) = compute_click_selection(10, vec![10], visible, 3, 40, false, true);
    assert_eq!(sel, vec![10, 40]);
    assert_eq!(anchor, 40);
}

#[test]
fn ctrl_click_removes_selected_row() {
    let (sel, anchor) = compute_click_selection(10, vec![10, 40], visible, 3, 40, false, true);
    assert_eq!(sel, vec![10]);
    assert_eq!(anchor, 40);
}

#[test]
fn shift_click_selects_range_and_keeps_anchor() {
    let (sel, anchor) = compute_click_selection(20, vec![20], visible, 4, 50, true, false);
    assert_eq!(sel, vec![20, 30, 40, 50]);
    assert_eq!(anchor, 20);
}

#[test]
fn shift_click_range_works_upward() {
    let (sel, anchor) = compute_click_selection(50, vec![50], visible, 1, 20, true, false);
    assert_eq!(sel, vec![20, 30, 40, 50]);
    assert_eq!(anchor, 50);
}

#[test]
fn a_shift_click_on_the_anchor_selects_it_alone() {
    let (sel, anchor) = compute_click_selection(30, vec![30, 40], visible, 2, 30, true, false);
    assert_eq!(sel, vec![30]);
    assert_eq!(anchor, 30);
}

#[test]
fn shift_click_without_anchor_falls_back_to_single() {
    let (sel, anchor) = compute_click_selection(NO_ANCHOR, vec![], visible, 2, 30, true, false);
    assert_eq!(sel, vec![30]);
    assert_eq!(anchor, 30);
}

#[test]
fn shift_click_on_empty_visible_list_falls_back_to_single() {
    let (sel, anchor) = compute_click_selection(20, vec![20], Vec::new, 3, 40, true, false);
    assert_eq!(sel, vec![40]);
    assert_eq!(anchor, 40);
}

/// A range measured from a row nobody can see would select rows the user never looked at.
#[test]
fn a_shift_click_whose_anchor_is_filtered_out_selects_the_row_alone() {
    let (sel, anchor) = compute_click_selection(70, vec![70], visible, 4, 50, true, false);
    assert_eq!(sel, vec![50]);
    assert_eq!(anchor, 50);
}

/// The index Slint hands over is trusted only where it names the clicked id; a row model rebuilt
/// under the click, or an index past its end, is looked up by id instead.
#[test]
fn a_shift_click_finds_its_row_by_id_wherever_the_index_does_not_name_it() {
    let cases = [(0, "a stale index naming another row"), (99, "past the end"), (-1, "negative")];
    for (idx, case) in cases {
        let (sel, anchor) = compute_click_selection(20, vec![20], visible, idx, 50, true, false);
        assert_eq!((sel, anchor), (vec![20, 30, 40, 50], 20), "{case}");
    }
}

/// Browse lists a disk-only row as `0` so the index still names its row, and nothing selects one.
#[test]
fn a_shift_range_leaves_out_the_rows_listed_as_zero() {
    let shown = || vec![10, 0, 20, 0];
    let (sel, anchor) = compute_click_selection(10, vec![10], shown, 2, 20, true, false);
    assert_eq!((sel, anchor), (vec![10, 20], 10));
}

/// Every disk-only row shares the `0`, so one picked would light them all. Browse refuses the
/// click itself; this is the line behind it, and it holds on every branch a click can take.
#[test]
fn no_click_picks_a_row_listed_as_zero() {
    let shown = || vec![10, 0, 20];
    let cases = [
        (false, false, Vec::new(), "a plain click"),
        (false, true, vec![10], "a Ctrl click"),
        (true, false, vec![10], "a Shift click"),
    ];
    for (shift, ctrl, expected, case) in cases {
        let (sel, _) = compute_click_selection(10, vec![10], shown, 1, 0, shift, ctrl);
        assert_eq!(sel, expected, "{case}");
    }
}

#[test]
fn shift_takes_priority_over_ctrl_when_anchored() {
    let (sel, _) = compute_click_selection(10, vec![10], visible, 2, 30, true, true);
    assert_eq!(sel, vec![10, 20, 30]);
}

#[test]
fn visible_ids_is_not_invoked_on_plain_or_ctrl_clicks() {
    let (sel, _) = compute_click_selection(
        10,
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

// --- a list view's whole selection, over the Songs lists' model ---

/// What a list view's global holds of its selection, showing `fixtures::four_tracks` as the two
/// Songs lists show their rows.
struct FakeView {
    anchor: Cell<i32>,
    selected: Rc<VecModel<i32>>,
    rows: Rc<TrackRows>,
}

impl FakeView {
    /// Showing the tracks in title order, 2 4 3 1, under `needle`.
    fn showing(needle: &str) -> Self {
        let rows = TrackRows::new(|_request| {});
        rows.show(track_keyset(needle, "asc"));
        Self { anchor: Cell::new(NO_ANCHOR), selected: Rc::new(VecModel::default()), rows }
    }

    fn refilter(&self, needle: &str) {
        self.rows.show(track_keyset(needle, "asc"));
    }

    /// Unfiltered, in reverse title order: 1 3 4 2.
    fn resort_descending(&self) {
        self.rows.show(track_keyset("", "desc"));
    }

    /// `(selected-ids, the anchor, each row's flag)`, the three things a click writes.
    fn state(&self) -> (Vec<i32>, i32, Vec<bool>) {
        let flags = self.rows.iter().map(|row| row.selected).collect();
        (self.selected.iter().collect(), self.anchor.get(), flags)
    }
}

impl RowSelectionView for FakeView {
    fn anchor(&self) -> i32 {
        self.anchor.get()
    }
    fn set_anchor(&self, anchor: i32) {
        self.anchor.set(anchor);
    }
    fn selected_ids(&self) -> ModelRc<i32> {
        ModelRc::from(Rc::clone(&self.selected))
    }
    fn replace_selected_ids(&self, _ids: ModelRc<i32>) {}
    fn track_rows(&self) -> ModelRc<UiTrackListRow> {
        ModelRc::from(Rc::clone(&self.rows))
    }
    fn bind_select_row(&self, _f: impl FnMut(i32, i32, bool, bool) + 'static) {}
    fn bind_select_all(&self, _f: impl FnMut() + 'static) {}
    fn bind_clear_selection(&self, _f: impl FnMut() + 'static) {}
}

#[test]
fn a_plain_click_writes_the_set_the_anchor_and_the_rows() {
    let view = FakeView::showing("");

    handle_row_click(&view, 1, 4, false, false);

    assert_eq!(view.state(), (vec![4], 4, vec![false, true, false, false]));
}

/// The range is the rows on screen, in the order they sit, not the order they were fetched in.
#[test]
fn a_shift_click_ranges_over_the_rows_on_screen() {
    let view = FakeView::showing("");
    handle_row_click(&view, 0, 2, false, false);

    handle_row_click(&view, 2, 3, true, false);

    assert_eq!(view.state(), (vec![2, 4, 3], 2, vec![true, true, true, false]));
}

#[test]
fn a_shift_click_under_a_filter_skips_the_hidden_rows() {
    let view = FakeView::showing("ha");
    handle_row_click(&view, 0, 2, false, false);

    handle_row_click(&view, 1, 3, true, false);

    assert_eq!(view.state(), (vec![2, 3], 2, vec![true, true]));
}

/// Track 2 moves from the top row to the bottom one under the resort, so a range kept by
/// position would run from whatever now sits on top.
#[test]
fn a_shift_click_after_a_resort_ranges_from_the_row_last_clicked() {
    let view = FakeView::showing("");
    handle_row_click(&view, 0, 2, false, false);
    view.resort_descending();

    handle_row_click(&view, 1, 3, true, false);

    assert_eq!(view.state(), (vec![3, 4, 2], 2, vec![false, true, true, true]));
}

/// A Ctrl-click adds to the set as it stands, hidden rows and all.
#[test]
fn a_ctrl_click_keeps_a_selection_the_filter_hides() {
    let view = FakeView::showing("");
    handle_row_click(&view, 3, 1, false, false);
    view.refilter("ha");

    handle_row_click(&view, 0, 2, false, true);
    view.refilter("");

    assert_eq!(view.state(), (vec![1, 2], 2, vec![true, false, false, true]));
}

/// Select All is not a click, so a shift-click after it still ranges from the row last clicked.
#[test]
fn select_all_keeps_the_anchor_and_takes_only_the_rows_on_screen() {
    let view = FakeView::showing("");
    handle_row_click(&view, 2, 3, false, false);
    view.refilter("ha");

    select_all_rows(&view);

    assert_eq!(view.state(), (vec![2, 3], 3, vec![true, true]));
}

/// A clear that wrote the set and left the rows stamped would paint every row as selected on a
/// model that keeps its own.
#[test]
fn clear_drops_the_anchor_and_unstamps_every_row() {
    let view = FakeView::showing("");
    handle_row_click(&view, 0, 2, false, false);
    handle_row_click(&view, 1, 4, false, true);

    clear_row_selection(&view);

    assert_eq!(view.state(), (Vec::new(), NO_ANCHOR, vec![false; 4]));
}

// --- keeping a selection to the set behind the list ---

/// Track 3 left the set, so the next batch action must not still act on it.
#[test]
fn a_refetch_drops_a_track_it_no_longer_carries_from_the_selection() {
    let view = FakeView::showing("");
    handle_row_click(&view, 0, 2, false, false);
    handle_row_click(&view, 2, 3, false, true);

    keep_selection_within(&view, || HashSet::from([1, 2, 4]));

    assert_eq!(view.state(), (vec![2], NO_ANCHOR, vec![true, false, false, false]));
}

/// The anchor alone can name a track that left, after a Ctrl-click took it back out of the set.
#[test]
fn a_refetch_drops_an_anchor_it_no_longer_carries() {
    let view = FakeView::showing("");
    handle_row_click(&view, 2, 3, false, false);
    handle_row_click(&view, 2, 3, false, true);

    keep_selection_within(&view, || HashSet::from([1, 2, 4]));

    assert_eq!(view.state(), (Vec::new(), NO_ANCHOR, vec![false; 4]));
}

/// A filter hides a pick, which the next refilter paints again, so the set a selection is kept
/// to is the whole one rather than the rows on screen.
#[test]
fn a_refilter_keeps_a_hidden_selection() {
    let view = FakeView::showing("");
    handle_row_click(&view, 3, 1, false, false);
    view.refilter("ha");

    keep_selection_within(&view, || HashSet::from([1, 2, 3, 4]));

    assert_eq!(view.state(), (vec![1], 1, vec![false, false]));
}

/// The reload a removal brings prunes nothing under a filter, and a pick left behind out of sight
/// is one a Ctrl-click carries into the next batch action.
#[test]
fn a_removal_under_a_filter_drops_the_removed_rows_from_the_selection() {
    let view = FakeView::showing("ha");
    handle_row_click(&view, 0, 2, false, false);
    handle_row_click(&view, 1, 3, false, true);

    track_rows::remove_rows_from(&view, &view.rows, &[3]);

    assert_eq!(view.state(), (vec![2], NO_ANCHOR, vec![true]));
}

#[test]
fn a_removal_keeps_a_pick_the_filter_hides() {
    let view = FakeView::showing("");
    handle_row_click(&view, 3, 1, false, false);
    view.refilter("ha");

    track_rows::remove_rows_from(&view, &view.rows, &[3]);

    assert_eq!(view.state(), (vec![1], 1, vec![false]));
}

#[test]
fn nothing_to_keep_builds_no_set() {
    let view = FakeView::showing("");

    keep_selection_within(&view, || unreachable!("an empty selection has nothing to prune"));

    assert_eq!(view.state(), (Vec::new(), NO_ANCHOR, vec![false; 4]));
}

/// On a library-sized list the picks can sit thousands of rows from the row right-clicked, and
/// with Clear gated on that row being picked the user scrolled back to find one first.
#[test]
fn the_row_menu_offers_clear_selection_wherever_the_list_holds_a_set() {
    let gate = ROW_MENU
        .split_once("label: @tr(\"Clear selection\");")
        .and_then(|(head, _)| head.trim_end().lines().last())
        .map(str::trim);

    assert_eq!(
        gate,
        Some("if root.show-selection-actions && root.selection-count > 0: MenuItem {")
    );
}
