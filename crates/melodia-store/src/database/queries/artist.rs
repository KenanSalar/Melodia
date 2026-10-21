use crate::database::{
    DbPool, IDS_PER_SET, chunked_in_query, in_id_order, select_in_id_order, set_where_id_in,
};
use melodia_core::entities::artist;
use melodia_core::error::AppError;

/// The Artists grid's cards for `ids`, in the order asked for.
pub async fn get_artists_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<artist::ArtistStats>, AppError> {
    select_in_id_order(db, "SELECT * FROM artist_stats", ids, |a: &artist::ArtistStats| a.id).await
}

pub async fn get_artist_by_id(db: &DbPool, id: i64) -> Result<artist::ArtistStats, AppError> {
    sqlx::query_as::<_, artist::ArtistStats>("SELECT * FROM artist_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Artist", id))
}

/// The artist a track or album with no artist tag is filed under, inserted at a fixed id by the
/// initial schema.
///
/// Never pruned, and never given an image: it names no one, so a directory search for its name
/// answers with a stranger's photo, drawn under every untagged track.
pub const UNKNOWN_ARTIST_ID: i64 = 1;

/// The artists an image fetch should look up, the [`UNKNOWN_ARTIST_ID`] placeholder excluded.
///
/// An artist whose last search found nothing at `missed_before` or later (Unix seconds) sits it
/// out until that miss is older.
pub async fn get_artists_without_images(
    db: &DbPool,
    missed_before: i64,
) -> Result<Vec<artist::Artist>, AppError> {
    let artists = sqlx::query_as::<_, artist::Artist>(
        "SELECT * FROM artists WHERE (image_path IS NULL OR image_path = '') AND id <> ? \
         AND (image_search_missed_at IS NULL OR image_search_missed_at < ?)",
    )
    .bind(UNKNOWN_ARTIST_ID)
    .bind(missed_before)
    .fetch_all(db.read())
    .await?;
    Ok(artists)
}

/// Records that the image search for each of `ids` found nothing, at `at` (Unix seconds).
pub async fn mark_image_search_missed(db: &DbPool, ids: &[i64], at: i64) -> Result<(), AppError> {
    for chunk in ids.chunks(IDS_PER_SET) {
        set_where_id_in("UPDATE artists SET image_search_missed_at = ?", at, chunk)
            .execute(db.write())
            .await?;
    }
    Ok(())
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

/// Favorites' Favorite Artists cards for `ids`, each with how many of its tracks are favourites,
/// in the order asked for. An id that has lost its last favourite since the keyset was read
/// returns nothing.
pub async fn get_favorite_artists_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<artist::FavoriteArtist>, AppError> {
    let rows = chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT ar.id, ar.name, ar.image_path, COUNT(DISTINCT t.id) AS favorite_count \
             FROM artists ar \
             JOIN track_artists ta ON ta.artist_id = ar.id \
             JOIN tracks t ON t.id = ta.track_id AND t.is_favorite = TRUE \
             WHERE ar.id IN ({placeholders}) \
             GROUP BY ar.id"
        )
    })
    .await?;
    Ok(in_id_order(rows, ids, |a: &artist::FavoriteArtist| a.id))
}

#[cfg(test)]
#[path = "tests/artist_tests.rs"]
mod tests;
