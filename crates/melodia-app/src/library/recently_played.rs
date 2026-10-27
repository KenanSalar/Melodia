//! Recently Played, read-only over the `last_played` `tasks::play_count_flusher` stamps. The Most
//! Played tab is `library::keyset`'s.

use melodia_core::entities::track;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Membership is fixed to this set, the view filtering and re-sorting it in memory.
pub const RECENTLY_PLAYED_LIMIT: i64 = 200;

/// The most-recently-played tracks (newest first), list-view projection.
pub async fn get_recently_played(db: &DbPool) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_recently_played(db, RECENTLY_PLAYED_LIMIT).await
}
