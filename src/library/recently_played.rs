//! Recently-Played library API — thin async wrappers over the
//! `queries::track` recency/most-played queries. Mirrors `library::favorites`.
//!
//! Read-only over data written elsewhere: `tasks::play_count_flusher` stamps
//! `last_played` and bumps `stats_changed_tx`, so the lifecycle re-fetches on that
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

/// The most-played tracks across the whole library, for the "Most Played" tab.
///
/// Uncapped: a virtualized grid has nothing to gain by truncating, and the `[1, 100]`
/// clamp it used to take was the right shape for the ten-card carousel it replaced,
/// not a ceiling to scroll into. `favorites::get_most_played_favorites` makes the same
/// call over a set that is a strict subset of its own Songs tab, where this one is
/// everything ever played — the query's own doc comment says what that costs per
/// `stats_changed` tick.
pub async fn get_most_played(state: &AppState) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    queries::track::get_most_played(&state.db).await
}
