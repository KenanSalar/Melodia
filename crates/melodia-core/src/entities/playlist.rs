use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow)]
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
#[derive(Clone, Debug, PartialEq, FromRow)]
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

/// What one removal from a playlist took, enough to put each row back where it stood.
#[derive(Debug, PartialEq)]
pub struct PlaylistItemsCut {
    pub playlist_id: i64,
    /// Ascending by `row`.
    pub rows: Vec<CutItem>,
    pub updated_before: String,
    /// The removal's own `updated_at`, which a restore puts back only while it is still there.
    pub stamped_at: String,
}

#[derive(Debug, PartialEq)]
pub struct CutItem {
    pub track_id: i64,
    /// Its index in the playlist's order at the moment it was cut, not its raw position, which can
    /// carry gaps.
    pub row: usize,
    pub added_at: String,
}
