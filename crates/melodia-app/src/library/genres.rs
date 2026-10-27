//! Genre reads. The grid's ids and cards are `library::keyset`'s.

use melodia_core::entities::genre;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// `AppError::NotFound` where the genre's last track went between the grid painting and the click.
pub async fn get_genre_detail(db: &DbPool, id: i64) -> Result<genre::GenreStats, AppError> {
    queries::genre::get_genre_by_id(db, id).await
}
