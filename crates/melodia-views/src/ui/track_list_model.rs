//! The Slint model for a list whose whole set is resident in a
//! [`TrackListCache`](crate::ui::track_list_cache::TrackListCache).
//!
//! A `VecModel` there is a second copy of every row the cache already holds: refcounts rather
//! than text, but a library-sized vector of them all the same. This holds a snapshot and the
//! filter's indices into it instead, and clones a row only when Slint asks for one, which a
//! virtualized `ListView` does for the rows on screen.
//!
//! The per-row `selected` flag is the model's own, a set beside the snapshot, so a click
//! repaints the rows whose flag moved and a refilter keeps the selection without restamping
//! anything. `is_favorite` and `rating` are the cache's: a patch writes them there, and
//! [`TrackListModel::repaint`] tells Slint which row to read again. UI thread only.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::BuildHasher;

use slint::{Model, ModelNotify, ModelRc, ModelTracker};

use crate::ui::track_list_cache::VisibleRows;
use melodia_ui::TrackListRow as UiTrackListRow;

#[derive(Default)]
pub struct TrackListModel {
    shown: RefCell<VisibleRows>,
    selected: RefCell<HashSet<i32>>,
    notify: ModelNotify,
}

impl TrackListModel {
    /// The model behind `rows`, where it is one.
    pub fn of(rows: &ModelRc<UiTrackListRow>) -> Option<&Self> {
        rows.as_any().downcast_ref()
    }

    /// [`Self::of`] for a list installed with one, logging a miss under `label` rather than
    /// passing over it: the list and its cache disagree from there on, and nothing else says so.
    pub fn installed<'a>(rows: &'a ModelRc<UiTrackListRow>, label: &str) -> Option<&'a Self> {
        let found = Self::of(rows);
        if found.is_none() {
            log::warn!("{label}: TrackListModel downcast failed, rows left as they were");
        }
        found
    }

    /// Show `next`. Where the same ids sit in the same places only the rows that read
    /// differently are repainted, so a library tick that moved nothing on screen rebuilds no
    /// `ListView` delegate; anything structural resets.
    pub fn show(&self, next: VisibleRows) {
        let prev = self.shown.replace(next);
        let repaint: Option<Vec<usize>> = {
            let shown = self.shown.borrow();
            let aligned = prev.len() == shown.len() && prev.ids().eq(shown.ids());
            aligned
                .then(|| (0..shown.len()).filter(|&pos| !prev.reads_same_at(&shown, pos)).collect())
        };
        match repaint {
            Some(rows) => rows.into_iter().for_each(|pos| self.notify.row_changed(pos)),
            None => self.notify.reset(),
        }
    }

    /// Every displayed id, in display order.
    pub fn ids(&self) -> Vec<i32> {
        self.shown.borrow().ids().collect()
    }

    /// Make `ids` the selected set, repainting the rows whose flag moved.
    pub fn select<S: BuildHasher>(&self, ids: &HashSet<i32, S>) {
        let moved: Vec<usize> = {
            let selected = self.selected.borrow();
            self.shown
                .borrow()
                .ids()
                .enumerate()
                .filter(|(_, id)| selected.contains(id) != ids.contains(id))
                .map(|(pos, _)| pos)
                .collect()
        };
        *self.selected.borrow_mut() = ids.iter().copied().collect();
        for pos in moved {
            self.notify.row_changed(pos);
        }
    }

    /// Select every displayed row, returning their ids in display order.
    pub fn select_all(&self) -> Vec<i32> {
        let ids = self.ids();
        self.select(&ids.iter().copied().collect::<HashSet<i32>>());
        ids
    }

    /// Ask Slint to read the row showing `id` again, after a patch to the cache's marks.
    pub fn repaint(&self, id: i64) {
        let pos = self.shown.borrow().ids().position(|shown| i64::from(shown) == id);
        if let Some(pos) = pos {
            self.notify.row_changed(pos);
        }
    }
}

impl Model for TrackListModel {
    type Data = UiTrackListRow;

    fn row_count(&self) -> usize {
        self.shown.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<UiTrackListRow> {
        let mut data = self.shown.borrow().row(row)?;
        data.selected = self.selected.borrow().contains(&data.id);
        Some(data)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
#[path = "tests/track_list_model_tests.rs"]
mod tests;
