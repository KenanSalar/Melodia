//! The Favorite Artists order. Most Played has none: its SQL rank is the
//! tab's whole meaning.

use crate::entities::artist::FavoriteArtist;
use crate::services::settings::{SortDir, ViewSort};
use crate::ui::favorites::FavoritesUi;

/// Order the Favorite Artists cache in place.
///
/// The **cache**, not the filtered copy `apply::build_filtered_grids` builds,
/// because [`FavoritesUi::first_screenful_paths`] reads the cache directly to
/// decide which covers to prewarm — sorting downstream would warm whichever
/// artists SQL happened to return first while the grid painted a different
/// prefix. Filtering preserves order, so one sort here serves both.
///
/// `favorite_count` breaks ties by name. The SQL it replaces broke them not at
/// all, so artists on the same count could swap places between refreshes.
///
/// Mirrors `ui::artists::grid::sort_artist_indices`, down to reversing rather
/// than branching the comparator.
pub(super) fn sort_artists(artists: &mut [FavoriteArtist], field: &str, dir: SortDir) {
    match field {
        "name" => artists.sort_by_cached_key(|a| a.name.to_lowercase()),
        _ => artists.sort_by_cached_key(|a| (a.favorite_count, a.name.to_lowercase())),
    }
    if matches!(dir, SortDir::Desc) {
        artists.reverse();
    }
}

/// Re-order the cached Favorite Artists to the active sort. Cheap, in-memory,
/// callable from either thread.
pub(super) fn sort_cached_artists(fav_ui: &FavoritesUi) {
    // Clone the sort out in its own statement — taking the second lock while
    // the first guard is still live would nest them for no reason.
    let ViewSort { field, dir } = fav_ui.state().artist_sort.lock().clone();
    sort_artists(&mut fav_ui.state().fav_artists.lock(), &field, dir);
}

/// Set the Favorite Artists sort and re-order the cache to match.
///
/// One call rather than two so no path can move the shadow without moving the
/// rows the prewarm reads — which would be invisible until the covers came up
/// against the wrong cards.
pub fn set_artist_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().artist_sort.lock() = ViewSort { field, dir };
    sort_cached_artists(fav_ui);
}
