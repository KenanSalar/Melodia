//! Songs tab as a [`PagedTrackList`]: the sort and filter shadows its loads run under, and when a
//! load may still land.
//!
//! The Tracks view's shape over the favourites: a [`TrackRows`] holding the ids in the tab's sort
//! and filter, both answered in SQL, and the rows near the viewport. Its model is kept across a
//! tab switch, being a keyset and a few hundred rows, and handed back on the section leave.

use std::sync::Arc;

use slint::Weak;

use super::{FavoritesTab, FavoritesUi};
use crate::ui::paged_rows::track_rows::{self, ListLoads, ListQuery, PagedTrackList, TrackRows};
use melodia_app::services::settings::ViewSort;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::Favorites;

const TRACKS_LABEL: &str = "Favorites.tracks";

impl PagedTrackList for FavoritesUi {
    type Global = Favorites<'static>;
    type Facts = ();

    const LABEL: &'static str = TRACKS_LABEL;

    fn loads(&self) -> &ListLoads {
        &self.songs
    }

    fn global(&self) -> &Weak<Favorites<'static>> {
        &self.global
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        &self.cover_thumbs
    }

    fn query(&self) -> Option<ListQuery> {
        let sort = current_sort(self);
        Some(ListQuery {
            scope: TrackListScope::Favorites,
            order: TrackOrder::new(&sort.field, sort.dir),
            needle: current_filter(self),
        })
    }

    /// Songs mounted on a page still on screen. A leave has already emptied the model, so a load
    /// landing behind it would undo that teardown behind a view nobody can see, and a tab pick
    /// leaves the list to the pick that brings it back.
    fn still_wanted(&self, _: &ListQuery) -> bool {
        self.section_active() && self.active_tab() == FavoritesTab::Songs
    }

    /// The caller consumed the dirty flag before asking, so a load that shows nothing re-arms it
    /// for the pick that brings Songs back.
    fn load_dropped(&self) {
        self.mark_songs_dirty();
    }
}

/// The active sort. Slint mirrors it in `Favorites.sort-*`, but this cache is the source of truth
/// — those properties are written from Rust, never back.
fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter needle, folded by [`set_filter`] and ready to hand to a
/// `row_match` predicate.
pub fn current_filter(fav_ui: &FavoritesUi) -> Needle {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers follow it with [`requery_tracks`] and a `set_view_sort`
/// persist, so the next launch lands on the same order.
pub fn set_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().sort.lock() = ViewSort { field, dir };
}

/// Update the cached filter needle, so the live-refresh subscriber can reload on a background
/// thread without reading the Slint global. Folded on the way in, so every reader shares one.
pub fn set_filter(fav_ui: &FavoritesUi, filter: &str) {
    *fav_ui.state().filter.lock() = fold_needle(filter);
}

/// Reload the ids after a header click or a keystroke ([`track_rows::requery`]).
///
/// Nothing is loaded while another tab is mounted: entering Songs clears the shared filter, and a
/// Songs tab left filtered is marked dirty on its way out, so the pick that brings it back
/// refreshes it anyway.
pub fn requery_tracks(state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    if fav_ui.active_tab() != FavoritesTab::Songs {
        return;
    }
    track_rows::requery(state, fav_ui);
}

/// The Songs tab's ids in display order, for `play-row` and the hero's Shuffle.
pub fn displayed_ids(g: &Favorites<'_>) -> Vec<i64> {
    TrackRows::ids_of(&g.get_tracks(), TRACKS_LABEL)
}

/// Empty `Favorites.tracks`, its ids and cached rows with it, for the section leave.
pub fn release_tracks(g: &Favorites<'_>) {
    let rows = g.get_tracks();
    if let Some(model) = TrackRows::installed(&rows, TRACKS_LABEL) {
        model.clear();
    }
}
