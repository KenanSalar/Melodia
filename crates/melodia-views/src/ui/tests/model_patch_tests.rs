//! The id walk, driven over real `VecModel`s.
//!
//! How far the walk got before it stopped is invisible from the outside, so each case is staged
//! so that a walk going too far, or not at all, leaves different data behind.

use std::collections::HashSet;
use std::rc::Rc;

use slint::{FilterModel, Model, ModelRc, VecModel};

use super::patch_track_rows_by_ids;
use melodia_ui::TrackListRow as UiTrackListRow;

fn track_rows(ids: &[i32]) -> (Rc<VecModel<UiTrackListRow>>, ModelRc<UiTrackListRow>) {
    let vec: Vec<UiTrackListRow> =
        ids.iter().map(|&id| UiTrackListRow { id, ..UiTrackListRow::default() }).collect();
    let model = Rc::new(VecModel::from(vec));
    let rc = ModelRc::from(Rc::clone(&model));
    (model, rc)
}

fn ratings(model: &VecModel<UiTrackListRow>) -> Vec<i32> {
    (0..model.row_count()).filter_map(|i| model.row_data(i).map(|row| row.rating)).collect()
}

/// A model that is not a `VecModel` is left as it was rather than patched through the `ModelRc`,
/// which forwards `set_row_data` to whatever is behind it.
#[test]
fn a_model_the_walk_cannot_downcast_is_left_alone() {
    let (model, rc) = track_rows(&[1, 2]);
    let filtered: ModelRc<UiTrackListRow> = ModelRc::new(FilterModel::new(rc, |_| true));

    patch_track_rows_by_ids(&filtered, &HashSet::from([2]), |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 0]);
}

#[test]
fn the_id_walk_patches_the_row_it_names_and_no_other() {
    let (model, rc) = track_rows(&[1, 2, 3]);

    patch_track_rows_by_ids(&rc, &HashSet::from([2]), |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 5, 0]);
}

/// Ids are unique in these models, so the walk returns once every id has matched rather than
/// cloning every row past the last. Two rows sharing an id is the only way to see where it
/// stopped.
#[test]
fn the_id_walk_stops_at_the_first_row_it_matches() {
    let (model, rc) = track_rows(&[1, 2, 2]);

    patch_track_rows_by_ids(&rc, &HashSet::from([2]), |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 5, 0]);
}

#[test]
fn an_id_no_row_carries_leaves_the_model_as_it_was() {
    let (model, rc) = track_rows(&[1, 2, 3]);

    patch_track_rows_by_ids(&rc, &HashSet::from([99]), |row| row.rating = 5);

    assert_eq!(ratings(&model), vec![0, 0, 0]);
}
