//! What the view suites build their lists out of.
//!
//! `#[cfg(test)] pub(crate)`, as `melodia-app`'s `state::fixtures` is: nothing past this crate's
//! own tests reads it.

use melodia_core::entities::track::TrackListRow;
use melodia_core::utils::needle::fold_needle;

use crate::ui::track_list_cache::{TrackListCache, VisibleRows};

/// A track titled `title` with every other field empty, for a test to fill in the ones it reads.
pub(crate) fn track_row(id: i64, title: &str) -> TrackListRow {
    TrackListRow {
        id,
        title: title.to_owned(),
        artist: None,
        album_artist: None,
        album: None,
        genre: None,
        track_number: None,
        disc_number: None,
        year: None,
        duration_ms: 0,
        artwork_path: None,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        sort_key: None,
    }
}

/// Four tracks whose title order, 2 4 3 1, is neither their id order nor its reverse. `"ha"`
/// matches 2 and 3.
pub(crate) fn four_tracks() -> Vec<TrackListRow> {
    [(1, "delta"), (2, "alpha"), (3, "charlie"), (4, "bravo")]
        .into_iter()
        .map(|(id, title)| TrackListRow {
            sort_key: Some(title.to_owned()),
            ..track_row(id, title)
        })
        .collect()
}

/// What a list showing `cache` under `needle` holds.
pub(crate) fn visible_rows(cache: &TrackListCache, needle: &str) -> VisibleRows {
    VisibleRows::new(cache.snapshot(), &fold_needle(needle))
}
