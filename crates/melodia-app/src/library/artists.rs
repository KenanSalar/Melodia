//! Artist reads. The grid's ids and cards are `library::keyset`'s.

use melodia_core::entities::{album, artist};
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Stats for a single artist, or `AppError::NotFound` if the id is gone.
pub async fn get_artist_detail(db: &DbPool, id: i64) -> Result<artist::ArtistStats, AppError> {
    queries::artist::get_artist_by_id(db, id).await
}

/// In the Albums grid's `AlbumStats` shape, for the Artist Detail header's Albums section.
pub async fn get_artist_albums(
    db: &DbPool,
    artist_id: i64,
) -> Result<Vec<album::AlbumStats>, AppError> {
    queries::album::get_albums_by_artist(db, artist_id).await
}
