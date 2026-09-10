//! The two walks, driven over real `VecModel`s.
//!
//! Both contracts are invisible from the outside: whether a row was written back, and how far the
//! walk got before it stopped. Neither shows up in the model's contents, so each case is staged so
//! that a skipped write and a redundant one leave different data behind.

use std::rc::Rc;

use slint::{FilterModel, Model, ModelRc, VecModel};

use super::{patch_rows_where, patch_track_row_by_id};
use melodia_ui::TrackListRow as UiTrackListRow;

/// `tag` is what a write leaves behind: the patch closure moves it whatever it reports, so the
/// model afterwards says whether `set_row_data` ran rather than whether the closure did.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: i32,
    tag: u32,
}

fn rows(ids: &[i32]) -> (Rc<VecModel<Row>>, ModelRc<Row>) {
    let vec: Vec<Row> = ids.iter().map(|&id| Row { id, tag: 0 }).collect();
    let model = Rc::new(VecModel::from(vec));
    let rc = ModelRc::from(Rc::clone(&model));
    (model, rc)
}

fn tags(model: &VecModel<Row>) -> Vec<u32> {
    (0..model.row_count()).filter_map(|i| model.row_data(i).map(|row| row.tag)).collect()
}

fn track_rows(ids: &[i32]) -> (Rc<VecModel<UiTrackListRow>>, ModelRc<UiTrackListRow>) {
    let vec: Vec<UiTrackListRow> = ids
        .iter()
        .map(|&id| UiTrackListRow {
            id,
            ..UiTrackListRow::default()
        })
        .collect();
    let model = Rc::new(VecModel::from(vec));
    let rc = ModelRc::from(Rc::clone(&model));
    (model, rc)
}

fn ratings(model: &VecModel<UiTrackListRow>) -> Vec<i32> {
    (0..model.row_count()).filter_map(|i| model.row_data(i).map(|row| row.rating)).collect()
}

#[test]
fn a_row_the_patch_reports_unmoved_is_not_written_back() {
    let (model, rc) = rows(&[1, 2, 3]);

    // The closure mutates every row and reports none of them moved, which is the shape of a
    // favourite toggle landing on rows that already hold the value.
    patch_rows_where(&rc, "test", |row| {
        row.tag = 9;
        false
    });

    assert_eq!(tags(&model), vec![0, 0, 0]);
}

#[test]
fn only_the_rows_the_patch_reports_moved_are_written_back() {
    let (model, rc) = rows(&[1, 2, 3]);

    patch_rows_where(&rc, "test", |row| {
        row.tag = 9;
        row.id == 2
    });

    assert_eq!(tags(&model), vec![0, 9, 0]);
}

/// A model that is not a `VecModel` is left as it was rather than half-patched through the
/// `ModelRc`, which forwards `set_row_data` to whatever is behind it.
#[test]
fn a_model_the_walk_cannot_downcast_is_left_alone() {
    let (model, rc) = rows(&[1, 2]);
    let filtered: ModelRc<Row> = ModelRc::new(FilterModel::new(rc, |_| true));

    patch_rows_where(&filtered, "test", |row| {
        row.tag = 9;
        true
    });

    assert_eq!(tags(&model), vec![0, 0]);
}

#[test]
fn the_id_walk_patches_the_row_it_names_and_no_other() {
    let (model, rc) = track_rows(&[1, 2, 3]);

    patch_track_row_by_id(&rc, 2, |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 5, 0]);
}

/// Ids are unique in these models, so the walk returns at the first match rather than cloning
/// every row past it. Two rows sharing an id is the only way to see where it stopped.
#[test]
fn the_id_walk_stops_at_the_first_row_it_matches() {
    let (model, rc) = track_rows(&[1, 2, 2]);

    patch_track_row_by_id(&rc, 2, |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 5, 0]);
}

#[test]
fn an_id_no_row_carries_leaves_the_model_as_it_was() {
    let (model, rc) = track_rows(&[1, 2, 3]);

    patch_track_row_by_id(&rc, 99, |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 0, 0]);
}
