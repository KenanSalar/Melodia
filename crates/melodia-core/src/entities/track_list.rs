//! What a track list holds and how it is ordered, in the terms the store's id queries take.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

impl SortDir {
    /// The lowercase token the Slint `sort-dir` properties use.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        }
    }

    /// Parse a Slint `sort-dir` token; anything other than `"desc"` is `Asc`.
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        if token == "desc" { SortDir::Desc } else { SortDir::Asc }
    }
}

/// Which tracks a list holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackListScope {
    /// My Library's Songs tab: every track.
    Library,
    /// Favorites' Songs tab.
    Favorites,
    Album(i64),
    /// Every track the artist is credited on, not only those naming it first.
    Artist(i64),
    /// Every track carrying the genre, not only those naming it first.
    Genre(i64),
    /// A manual playlist's items, or a smart playlist's current membership.
    Playlist(i64),
}

/// The column a track list sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSortField {
    /// The natural order, by the `sort_key` the scan derives from the title.
    Title,
    TrackNumber,
    Artist,
    Album,
    Genre,
    Year,
    Length,
    /// A playlist's own order. Any other list has none, and sorts by [`Self::Title`] instead.
    Position,
}

impl TrackSortField {
    /// Parse the token a header cell or a saved view spells. Anything unrecognised is the
    /// natural order, as `ui::track_sort` reads it.
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "track_number" => Self::TrackNumber,
            "artist" => Self::Artist,
            "album" => Self::Album,
            "genre" => Self::Genre,
            "year" => Self::Year,
            "length" => Self::Length,
            "position" => Self::Position,
            _ => Self::Title,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TrackOrder {
    pub field: TrackSortField,
    pub dir: SortDir,
}
