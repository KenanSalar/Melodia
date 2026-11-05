use crate::database::{DbPool, select_in_id_order, sql};
use melodia_core::entities::genre;
use melodia_core::error::AppError;

/// The Genres grid's cards for `ids`, in the order asked for.
pub async fn get_genres_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<genre::GenreStats>, AppError> {
    db.read(move |conn| {
        select_in_id_order(conn, "SELECT * FROM genre_stats", &ids, |g: &genre::GenreStats| g.id)
    })
    .await
}

pub async fn get_genre_by_id(db: &DbPool, id: i64) -> Result<genre::GenreStats, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, "SELECT * FROM genre_stats WHERE id = ?", (id,)))
        .await?
        .ok_or_else(|| AppError::not_found("Genre", id))
}

#[cfg(test)]
#[path = "tests/genre_tests.rs"]
mod tests;
