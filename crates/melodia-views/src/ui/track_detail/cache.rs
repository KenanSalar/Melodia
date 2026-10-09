//! The Rust half of an open detail, kept beside its Slint model so a click, a sort or a
//! filter keystroke never reads the model back to learn which rows it holds.

use std::collections::HashSet;

use parking_lot::Mutex;

use super::{DetailGlobal, selection};
use crate::ui::row_match::{self, Needle};
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::util::clamp_i64_to_i32;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;

/// One detail's cached rows, needle and selection shadow.
pub struct DetailCache {
    /// The entity on screen, `-1` while the grid is. The library-changed subscriber and the
    /// grid prewarm both ask it.
    pub(super) id: Mutex<i64>,
    /// The displayed subset in display order, in lockstep with the Slint `tracks` model:
    /// selection and play-row map index to id through it.
    pub(super) tracks: Mutex<Vec<RsTrackListRow>>,
    /// The canonical set in display order, which the filter walks.
    pub(super) all_tracks: Mutex<Vec<RsTrackListRow>>,
    /// The selection currently stamped onto the model, so a click re-stamps only what flipped.
    pub(super) applied_selection: Mutex<HashSet<i32>>,
    /// Lets a refresh re-filter without reading the Slint property back.
    pub(super) filter: Mutex<Needle>,
}

impl Default for DetailCache {
    fn default() -> Self {
        Self {
            id: Mutex::new(-1),
            tracks: Mutex::new(Vec::new()),
            all_tracks: Mutex::new(Vec::new()),
            applied_selection: Mutex::new(HashSet::new()),
            filter: Mutex::new(Needle::default()),
        }
    }
}

impl DetailCache {
    pub fn id(&self) -> i64 {
        *self.id.lock()
    }

    pub fn set_id(&self, id: i64) {
        *self.id.lock() = id;
    }

    /// Ids of the displayed rows, in display order: play-row and shuffle act on what the
    /// filter left visible, not on the whole entity.
    pub fn track_ids(&self) -> Vec<i64> {
        self.tracks.lock().iter().map(|r| r.id).collect()
    }

    pub fn flip_favorite(&self, id: i64, fav: bool) {
        self.patch_row(id, |r| r.is_favorite = fav);
    }

    pub fn flip_rating(&self, id: i64, rating: i32) {
        self.patch_row(id, |r| r.rating = rating);
    }

    pub fn set_filter(&self, text: &str) {
        *self.filter.lock() = row_match::fold_needle(text);
    }

    pub fn needle(&self) -> Needle {
        self.filter.lock().clone()
    }

    pub fn is_filtered(&self) -> bool {
        !self.filter.lock().is_empty()
    }

    /// Seats a freshly opened entity's `tracks` as both the canonical and the displayed set, the
    /// needle cleared to match.
    pub fn seat_unfiltered(&self, tracks: Vec<RsTrackListRow>) {
        self.filter.lock().clear();
        self.all_tracks.lock().clone_from(&tracks);
        *self.tracks.lock() = tracks;
    }

    /// Replaces the canonical set and leaves the displayed one to the next filter pass, which
    /// is what keeps a refresh under a live needle from flashing unfiltered rows.
    pub fn set_all_tracks(&self, tracks: Vec<RsTrackListRow>) {
        *self.all_tracks.lock() = tracks;
    }

    /// Sorts the canonical and displayed sets with `sort`, returning the displayed ids in their
    /// new order. Both move together, so a filter widened later still comes back sorted.
    pub fn sort_both(&self, mut sort: impl FnMut(&mut [RsTrackListRow])) -> Vec<i32> {
        sort(&mut self.all_tracks.lock());
        let mut tracks = self.tracks.lock();
        sort(&mut tracks);
        tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect()
    }

    /// Re-sorts to the global's current sort and reorders the model rows to match: no fetch and
    /// no row rebuild, a header click changing the order and not the content.
    pub fn resort<G: DetailGlobal>(&self, g: &G) {
        let (field, dir) = g.sort();
        let order = self.sort_both(|rows| sort_track_list_rows(rows, &field, &dir));
        crate::ui::model_diff::permute_rows_by_id(&g.track_rows(), &order, |r| r.id);
        selection::apply_selection_to_rows(g, self);
    }

    pub fn clear(&self) {
        self.set_id(-1);
        self.release_rows();
        self.filter.lock().clear();
    }

    /// Drops the rows and the selection shadow on a section leave. The id stays, so the
    /// re-entry knows which detail to reopen; that reopen clears the needle.
    pub fn release_rows(&self) {
        self.tracks.lock().clear();
        self.all_tracks.lock().clear();
        self.applied_selection.lock().clear();
    }

    /// Both sets, or the next filter pass rebuilds the row from a canonical copy that never
    /// heard of the edit.
    fn patch_row(&self, id: i64, patch: impl Fn(&mut RsTrackListRow)) {
        for rows in [&self.tracks, &self.all_tracks] {
            if let Some(r) = rows.lock().iter_mut().find(|r| r.id == id) {
                patch(r);
            }
        }
    }
}
