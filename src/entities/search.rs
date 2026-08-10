use serde::{Deserialize, Serialize};

use crate::entities::{album, artist, genre, track};

/// Everything one search query matched, across the four kinds the Search page
/// paints.
///
/// Assembled by hand from four independent queries rather than decoded from a
/// row, which is why it carries no `FromRow` — the sibling aggregate
/// [`BrowseResult`](super::browse::BrowseResult) has the same shape for the
/// same reason. It is the return type of `library::search::search_all`, so it
/// is a boundary value the UI names directly.
#[derive(Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<track::TrackListRow>,
    pub albums: Vec<album::AlbumStats>,
    pub artists: Vec<artist::ArtistStats>,
    pub genres: Vec<genre::GenreStats>,
}
