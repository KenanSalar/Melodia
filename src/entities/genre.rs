use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct GenreStats {
    pub id: i64,
    pub name: String,
    pub track_count: i32,
    pub total_duration_ms: i64,
}
