use crate::entities::{album, artist, genre, track};

/// Everything one search matched, assembled from four queries rather than decoded from a row,
/// hence no `FromRow`.
#[derive(Clone)]
pub struct SearchResults {
    pub tracks: Vec<track::TrackListRow>,
    pub albums: Vec<album::AlbumStats>,
    pub artists: Vec<artist::ArtistStats>,
    pub genres: Vec<genre::GenreStats>,
}
