//! What the model over a cached list tells Slint to read again, and what it hands back.
//!
//! A `slint::FilterModel` stands in for the `ListView`: it re-runs its predicate for exactly the
//! row a change names and for every row on a reset, so the ids it records are the rows a list
//! would have read back, and its own count moves only when a change was structural.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use slint::{FilterModel, Model, ModelRc};

use super::TrackListModel;
use crate::ui::fixtures::{four_tracks, visible_rows};
use crate::ui::track_list_cache::{TrackListCache, VisibleRows};
use melodia_core::utils::needle::fold_needle;
use melodia_ui::TrackListRow as UiTrackListRow;

type Reread = Box<dyn Fn(&UiTrackListRow) -> bool>;

/// A model and the list reading it, the ids of every row read back since it was built in `seen`.
struct Watched {
    model: Rc<TrackListModel>,
    list: FilterModel<ModelRc<UiTrackListRow>, Reread>,
    seen: Rc<RefCell<Vec<i32>>>,
}

impl Watched {
    fn showing(rows: VisibleRows) -> Self {
        let model = Rc::new(TrackListModel::default());
        model.show(rows);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&seen);
        let reread: Reread = Box::new(move |row| {
            recorder.borrow_mut().push(row.id);
            true
        });
        let list = FilterModel::new(ModelRc::from(Rc::clone(&model)), reread);
        seen.borrow_mut().clear();
        Self { model, list, seen }
    }

    fn reread(&self) -> Vec<i32> {
        self.seen.borrow().clone()
    }

    fn selected_flags(&self) -> Vec<bool> {
        self.model.iter().map(|row| row.selected).collect()
    }
}

/// [`four_tracks`] in title order, 2 4 3 1.
fn stored() -> TrackListCache {
    let cache = TrackListCache::new();
    cache.store(four_tracks(), "title", "asc");
    cache
}

fn ids(ids: &[i32]) -> HashSet<i32> {
    ids.iter().copied().collect()
}

/// The only way a heart or a star change reaches the screen on either Songs list.
#[test]
fn a_repaint_notifies_only_the_row_showing_that_id() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    cache.set_rating(3, 5);

    watched.model.repaint(3);

    assert_eq!(watched.reread(), [3]);
}

#[test]
fn a_patched_mark_reads_through_the_shown_row() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));

    cache.set_rating(3, 5);
    cache.set_favorite(3, true);

    let row = watched.model.row_data(2).map(|row| (row.id, row.rating, row.is_favorite));
    assert_eq!(row, Some((3, 5, true)));
}

/// Every library tick refetches the list, and a reset rebuilds every `ListView` delegate for rows
/// that read the same.
#[test]
fn a_refetch_of_unchanged_rows_notifies_nothing() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    cache.store(four_tracks(), "title", "asc");

    watched.model.show(visible_rows(&cache, ""));

    assert_eq!(watched.reread(), Vec::<i32>::new());
}

/// A rating changed elsewhere arrives only through a refetch, which compares the marks as well as
/// the rows.
#[test]
fn a_refetch_changing_one_mark_repaints_only_that_row() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    let mut refetched = four_tracks();
    refetched[2].rating = 5;
    cache.store(refetched, "title", "asc");

    watched.model.show(visible_rows(&cache, ""));

    assert_eq!(watched.reread(), [3]);
}

/// A list told row by row about a change in length keeps the count it had.
#[test]
fn a_narrower_list_announces_its_new_length() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));

    watched.model.show(visible_rows(&cache, "ha"));

    assert_eq!(watched.list.row_count(), 2);
}

#[test]
fn a_wider_list_announces_its_new_length() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, "ha"));

    watched.model.show(visible_rows(&cache, ""));

    assert_eq!(watched.list.row_count(), 4);
}

#[test]
fn selecting_repaints_only_the_rows_whose_flag_moved() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    watched.model.select(&ids(&[2, 4]));
    watched.seen.borrow_mut().clear();

    watched.model.select(&ids(&[4, 3]));

    assert_eq!(watched.reread(), [2, 3]);
}

/// The set is the model's, kept beside whichever rows it shows, so a filter hides a selection
/// rather than dropping it.
#[test]
fn the_selection_survives_a_refilter() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    watched.model.select(&ids(&[3]));

    watched.model.show(visible_rows(&cache, "ha"));

    assert_eq!(watched.selected_flags(), [false, true]);
}

/// A Songs tab leave shows nothing and keeps the set, so the tab comes back as it was left.
#[test]
fn a_hidden_list_paints_its_selection_again_on_return() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));
    watched.model.select(&ids(&[3]));
    watched.model.show(VisibleRows::default());

    watched.model.show(visible_rows(&cache, ""));

    assert_eq!(watched.selected_flags(), [false, false, true, false]);
}

/// Hiding the list is how a tab leave hands the library-sized snapshot back.
#[test]
fn a_hidden_list_holds_no_snapshot() {
    let cache = stored();
    let snapshot = cache.snapshot();
    let watched = Watched::showing(VisibleRows::new(Arc::clone(&snapshot), &fold_needle("")));

    watched.model.show(VisibleRows::default());

    assert_eq!(Arc::strong_count(&snapshot), 2, "the cache's and this test's, and no other");
}

#[test]
fn a_row_past_the_last_reads_as_none() {
    let cache = stored();
    let watched = Watched::showing(visible_rows(&cache, ""));

    let edge = (watched.model.row_data(3).map(|row| row.id), watched.model.row_data(4));

    assert_eq!(edge, (Some(1), None), "(the last row, one past it)");
}
