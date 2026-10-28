//! Album reads. The grid's ids and cards are `library::keyset`'s.

use melodia_core::entities::album;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// `AppError::NotFound` where the album's folder went between the grid painting and the click.
pub async fn get_album_detail(db: &DbPool, id: i64) -> Result<album::AlbumStats, AppError> {
    queries::album::get_album_by_id(db, id).await
}
