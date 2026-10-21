//! What the view suites build their lists out of.
//!
//! `#[cfg(test)] pub(crate)`, as `melodia-app`'s `state::fixtures` is: nothing past this crate's
//! own tests reads it.

use melodia_core::entities::track::TrackListRow;
use melodia_core::utils::needle::fold_needle;
use melodia_ui::TrackListRow as UiTrackListRow;

use crate::ui::paged_rows::{Keyset, ReadStamp};
use crate::ui::tracks::to_slint_track_list_row;
use crate::ui::util::StringPool;
use crate::ui::{row_match, track_sort};

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

/// What a Songs list shows of [`four_tracks`] under `needle`, by title in `dir`, every row seeded
/// so none is ever fetched.
pub(crate) fn track_keyset(needle: &str, dir: &str) -> Keyset<UiTrackListRow> {
    let tracks = four_tracks();
    let needle = fold_needle(needle);
    let shown: Vec<&TrackListRow> = track_sort::compute_track_order(&tracks, "title", dir)
        .into_iter()
        .map(|i| &tracks[i])
        .filter(|track| row_match::track_matches(track, &needle))
        .collect();
    let mut pool = StringPool::default();
    Keyset {
        ids: shown.iter().map(|track| track.id).collect(),
        seed: shown
            .iter()
            .map(|track| (track.id, to_slint_track_list_row(track, &mut pool)))
            .collect(),
        read: ReadStamp::now(),
    }
}
