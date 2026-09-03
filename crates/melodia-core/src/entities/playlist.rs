use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub thumbnail_path: Option<String>,
    pub is_smart: bool,
    pub smart_criteria: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub custom_thumbnail: bool,
}

/// View-backed struct with computed stats (from `playlist_stats` view)
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct PlaylistStats {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub thumbnail_path: Option<String>,
    pub is_smart: bool,
    pub smart_criteria: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub custom_thumbnail: bool,
    pub track_count: i32,
    pub total_duration_ms: i64,
}
