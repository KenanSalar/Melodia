use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct AlbumStats {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub artist_id: i64,
    pub artist_name: String,
    pub year: Option<i32>,
    pub disc_count: Option<i32>,
    pub is_compilation: bool,
    pub musicbrainz_id: Option<String>,
    pub artwork_path: Option<String>,
    pub track_count: i32,
    pub total_duration_ms: i64,
}

/// Lightweight struct for albums containing favorited tracks.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct FavoriteAlbum {
    pub id: i64,
    pub name: String,
    pub artist_name: String,
    pub artwork_path: Option<String>,
    pub favorite_count: i32,
}
