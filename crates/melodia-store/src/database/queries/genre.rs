use crate::database::{DbPool, select_in_id_order};
use melodia_core::entities::genre;
use melodia_core::error::AppError;

/// The Genres grid's cards for `ids`, in the order asked for.
pub async fn get_genres_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<genre::GenreStats>, AppError> {
    select_in_id_order(db, "SELECT * FROM genre_stats", ids, |g: &genre::GenreStats| g.id).await
}

pub async fn get_genre_by_id(db: &DbPool, id: i64) -> Result<genre::GenreStats, AppError> {
    sqlx::query_as::<_, genre::GenreStats>("SELECT * FROM genre_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Genre", id))
}

#[cfg(test)]
#[path = "tests/genre_tests.rs"]
mod tests;
