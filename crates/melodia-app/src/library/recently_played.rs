//! Recently-Played library API — a thin async wrapper over the `queries::track`
//! recency query. The Most Played tab's ids and cards are `library::keyset`'s.
//!
//! Read-only over data written elsewhere: `tasks::play_count_flusher` stamps
//! `last_played` and bumps `stats_changed`, so the lifecycle re-fetches on that
//! signal exactly like Favorites.

use melodia_core::entities::track;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Number of most-recently-played tracks the view holds. Membership is fixed to
/// this set — the view filters/re-sorts it in memory rather than re-querying.
pub const RECENTLY_PLAYED_LIMIT: i64 = 200;

/// The most-recently-played tracks (newest first), list-view projection.
pub async fn get_recently_played(db: &DbPool) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_recently_played(db, RECENTLY_PLAYED_LIMIT).await
}
