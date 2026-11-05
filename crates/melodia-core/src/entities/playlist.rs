#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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

/// A playlist as an undo finds it again, `created_at` telling it from one that reused its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistIdentity {
    pub id: i64,
    pub created_at: String,
}

/// What one removal from a playlist took, enough to put each row back where it stood.
#[derive(Debug, PartialEq)]
pub struct PlaylistItemsCut {
    pub playlist: PlaylistIdentity,
    /// Ascending by `row`.
    pub rows: Vec<CutItem>,
    pub updated_before: String,
    /// The removal's own `updated_at`, which a restore puts back only while it is still there.
    pub stamped_at: String,
}

/// The rules a convert to a regular playlist cleared, enough to make it smart again.
#[derive(Debug, PartialEq)]
pub struct DroppedRules {
    pub playlist: PlaylistIdentity,
    pub name: String,
    /// As stored, so the undo writes back exactly what the convert cleared.
    pub smart_criteria: Option<String>,
}

/// A deleted playlist as a restore writes it back: the row under its own id, and every item.
#[derive(Debug, PartialEq)]
pub struct PlaylistSnapshot {
    pub playlist: Playlist,
    /// Ascending, each `row` its index in the playlist.
    pub items: Vec<CutItem>,
}

#[derive(Debug, PartialEq)]
pub struct CutItem {
    pub track_id: i64,
    /// Its index in the playlist's order at the moment it was cut, not its raw position, which can
    /// carry gaps.
    pub row: usize,
    pub added_at: String,
}
