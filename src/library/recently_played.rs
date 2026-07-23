//! Recently-Played library API — thin async wrappers over the
//! `queries::track` recency/most-played queries. Mirrors `library::favorites`.
//!
//! The view is read-only over data written elsewhere: `last_played` is stamped
//! by the play-count flusher (`tasks::play_count_flusher`) which also bumps
//! `stats_changed_tx`, so the Recently-Played lifecycle re-fetches on that
//! signal exactly like Favorites.

use crate::database::queries;
use crate::entities::track;
use crate::error::AppError;
use crate::state::AppState;

/// Number of most-recently-played tracks the view holds. Membership is fixed to
/// this set — the view filters/re-sorts it in memory rather than re-querying.
pub const RECENTLY_PLAYED_LIMIT: i64 = 200;

/// The most-recently-played tracks (newest first), list-view projection.
pub async fn get_recently_played(state: &AppState) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_recently_played(&state.db, RECENTLY_PLAYED_LIMIT).await
}

/// Top-N most-played tracks across the whole library (for the "Most Played"
/// strip). Clamps `limit` to a sane band like `favorites::get_most_played_favorites`.
pub async fn get_most_played(
    state: &AppState,
    limit: i64,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    let limit = limit.clamp(1, 100);
    queries::track::get_most_played(&state.db, limit).await
}
