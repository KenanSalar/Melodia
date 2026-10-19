//! What each card grid sorts by, in the terms the store's entity keysets take.
//!
//! Each parses the field token its grid's sort row spells, and anything it doesn't recognise
//! falls through to the grid's default arm, as the in-memory sorts these replaced read it.

/// The Albums grid's sort row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumSort {
    Name,
    /// A missing year sorts as `0`, then by name.
    Year,
    /// The artist line the card shows, then by name.
    Artist,
}

impl AlbumSort {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "year" => Self::Year,
            "artist" => Self::Artist,
            _ => Self::Name,
        }
    }
}

/// The Artists grid's sort row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtistSort {
    Name,
    TrackCount,
    AlbumCount,
}

impl ArtistSort {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "track_count" => Self::TrackCount,
            "album_count" => Self::AlbumCount,
            _ => Self::Name,
        }
    }
}

/// The Genres grid's sort row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenreSort {
    Name,
    TrackCount,
    Duration,
}

impl GenreSort {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "track_count" => Self::TrackCount,
            "duration" => Self::Duration,
            _ => Self::Name,
        }
    }
}

/// Favorites' Favorite Artists sort row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FavoriteArtistSort {
    /// How many of the artist's tracks are favourites, then by name.
    FavoriteCount,
    Name,
}

impl FavoriteArtistSort {
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "name" => Self::Name,
            _ => Self::FavoriteCount,
        }
    }
}
