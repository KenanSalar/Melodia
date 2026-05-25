use crate::database::DbPool;
use crate::entities::genre;
use crate::error::AppError;

pub async fn get_all_genres(db: &DbPool) -> Result<Vec<genre::GenreStats>, AppError> {
    let genres = sqlx::query_as::<_, genre::GenreStats>(
        "SELECT * FROM genre_stats ORDER BY name ASC"
    )
    .fetch_all(db.read())
    .await?;
    Ok(genres)
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
