//! Genre library API — thin wrappers over the `queries::genre` /
//! `queries::track` layer, mirroring `library/albums.rs`. Each function
//! takes the pool and returns `Result<_, AppError>`; the UI bridge
//! (`melodia-views`' `ui/genres/`) does the in-memory sorting / filtering / chunking.

use melodia_core::entities::{genre, track};
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Every genre with at least one track, name-sorted (the `genre_stats`
/// view's default order). The Genres grid re-sorts in memory for the
/// Track-count / Duration sort options, so this is fetched once and
/// cached UI-side.
pub async fn get_genres(db: &DbPool) -> Result<Vec<genre::GenreStats>, AppError> {
    queries::genre::get_all_genres(db).await
}

/// Stats for a single genre, or `AppError::NotFound` if the id is gone
/// (e.g. the last track tagged with this genre was removed between grid
/// render and click).
pub async fn get_genre_detail(db: &DbPool, id: i64) -> Result<genre::GenreStats, AppError> {
    queries::genre::get_genre_by_id(db, id).await
}

/// Every track tagged with the given genre, in the list-view projection
/// the shared `TrackList` component renders. Default order is the natural
/// `sort_key` (title) — the UI re-sorts in memory for the visible sort
/// header.
pub async fn get_genre_tracks(
    db: &DbPool,
    genre_id: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_tracks_by_genre_for_list(db, genre_id).await
}
