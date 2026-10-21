//! Album library API — thin wrappers over the `queries::album` /
//! `queries::track` layer, mirroring `library/tracks.rs`. Each function
//! takes the pool and returns `Result<_, AppError>`. The grid's ids and cards
//! are `library::keyset`'s.

use melodia_core::entities::album;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Stats for a single album, or `AppError::NotFound` if the id is gone
/// (e.g. the album's folder was removed between grid render and click).
pub async fn get_album_detail(db: &DbPool, id: i64) -> Result<album::AlbumStats, AppError> {
    queries::album::get_album_by_id(db, id).await
}
