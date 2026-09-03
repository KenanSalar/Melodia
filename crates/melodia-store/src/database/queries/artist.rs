use crate::database::DbPool;
use melodia_core::entities::artist;
use melodia_core::error::AppError;

pub async fn get_all_artists(db: &DbPool) -> Result<Vec<artist::ArtistStats>, AppError> {
    let artists =
        sqlx::query_as::<_, artist::ArtistStats>("SELECT * FROM artist_stats ORDER BY name ASC")
            .fetch_all(db.read())
            .await?;
    Ok(artists)
}

pub async fn get_artist_by_id(db: &DbPool, id: i64) -> Result<artist::ArtistStats, AppError> {
    sqlx::query_as::<_, artist::ArtistStats>("SELECT * FROM artist_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Artist", id))
}

pub async fn get_artists_without_images(db: &DbPool) -> Result<Vec<artist::Artist>, AppError> {
    let artists = sqlx::query_as::<_, artist::Artist>(
        "SELECT * FROM artists WHERE image_path IS NULL OR image_path = ''",
    )
    .fetch_all(db.read())
    .await?;
    Ok(artists)
}

pub async fn update_artist_image_path(
    db: &DbPool,
    artist_id: i64,
    image_path: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE artists SET image_path = ? WHERE id = ?")
        .bind(image_path)
        .bind(artist_id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Artists that have at least one favorited track, with favorite count.
///
/// Unordered — a caller owes its own ordering. The one there is,
/// `ui::favorites::grids::refresh_grids`, re-sorts the whole result through
/// `sort_artists` on every fetch, so an `ORDER BY` here would only be overwritten.
pub async fn get_favorite_artists(db: &DbPool) -> Result<Vec<artist::FavoriteArtist>, AppError> {
    let artists = sqlx::query_as::<_, artist::FavoriteArtist>(
        "SELECT ar.id, ar.name, ar.image_path, \
                COUNT(t.id) AS favorite_count \
         FROM artists ar \
         JOIN tracks t ON t.artist_id = ar.id AND t.is_favorite = TRUE \
         GROUP BY ar.id",
    )
    .fetch_all(db.read())
    .await?;
    Ok(artists)
}

#[cfg(test)]
#[path = "tests/artist_tests.rs"]
mod tests;
