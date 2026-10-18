//! What a list holding only ids reads: its ordered ids for a scope, the rows behind a page of
//! them, and what its hero states about the whole scope.

use melodia_core::entities::track::{MostPlayedFavorite, TrackListRow};
use melodia_core::entities::track_list::{ScopeFacts, TrackListScope, TrackOrder};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;
use melodia_store::database::{DbPool, queries};

/// Every track in `scope` that `needle` matches, as ids in `order`.
pub async fn track_ids(
    db: &DbPool,
    scope: TrackListScope,
    order: TrackOrder,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    queries::track::track_ids(db, scope, order, needle).await
}

/// The rows behind `ids`, in the order asked. An id whose row has gone since the ids were read is
/// dropped, so a short page is a library that changed under the list, not an error.
pub async fn rows_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<TrackListRow>, AppError> {
    queries::track::get_track_list_rows_by_ids(db, ids).await
}

/// How many tracks `scope` holds, how long they run and how many artists and albums they span.
pub async fn facts(db: &DbPool, scope: TrackListScope) -> Result<ScopeFacts, AppError> {
    queries::track::scope_facts(db, scope).await
}

/// Every genre line `scope`'s tracks carry, with how many carry it, the commonest first.
pub async fn genre_lines(
    db: &DbPool,
    scope: TrackListScope,
) -> Result<Vec<(String, i64)>, AppError> {
    queries::track::genre_lines(db, scope).await
}

/// The library-wide Most Played ranking that `needle` matches, as `(id, play_count)` pairs.
pub async fn most_played_ids(db: &DbPool, needle: &Needle) -> Result<Vec<(i64, i32)>, AppError> {
    queries::track::most_played_ids(db, needle).await
}

/// [`most_played_ids`] over the favorites alone.
pub async fn most_played_favorite_ids(
    db: &DbPool,
    needle: &Needle,
) -> Result<Vec<(i64, i32)>, AppError> {
    queries::track::most_played_favorite_ids(db, needle).await
}

/// The Most Played cards behind `ids`, in the order asked, dropping any whose row has gone.
pub async fn most_played_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<MostPlayedFavorite>, AppError> {
    queries::track::get_most_played_by_ids(db, ids).await
}
