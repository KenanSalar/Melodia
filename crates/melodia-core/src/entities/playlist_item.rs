use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub id: i64,
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: i32,
    pub added_at: String,
}
