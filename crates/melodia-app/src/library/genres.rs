//! Genre library API — thin wrappers over the `queries::genre` /
//! `queries::track` layer, mirroring `library/albums.rs`. Each function
//! takes the pool and returns `Result<_, AppError>`. The grid's ids and cards
//! are `library::keyset`'s.

use melodia_core::entities::genre;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Stats for a single genre, or `AppError::NotFound` if the id is gone
/// (e.g. the last track tagged with this genre was removed between grid
/// render and click).
pub async fn get_genre_detail(db: &DbPool, id: i64) -> Result<genre::GenreStats, AppError> {
    queries::genre::get_genre_by_id(db, id).await
}
