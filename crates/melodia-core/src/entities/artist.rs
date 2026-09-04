use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub musicbrainz_id: Option<String>,
    pub image_path: Option<String>,
}

/// View-backed struct with computed stats (from `artist_stats` view)
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct ArtistStats {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub musicbrainz_id: Option<String>,
    pub image_path: Option<String>,
    pub track_count: i32,
    pub album_count: i32,
    pub total_duration_ms: i64,
}

/// Lightweight struct for artists with favorited tracks.
///
/// `Hash` is what the Favorites grid compares against its last applied set —
/// derived rather than hand-listed so a new field can't silently drop out of
/// the comparison and leave a card stale.
#[derive(Clone, Debug, PartialEq, Hash, FromRow, Serialize, Deserialize)]
pub struct FavoriteArtist {
    pub id: i64,
    pub name: String,
    pub image_path: Option<String>,
    pub favorite_count: i32,
}
