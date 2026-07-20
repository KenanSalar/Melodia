//! FK helpers: upsert an `artist` / `album` / `genre` row by name and
//! return its rowid. Each returns the supplied "unknown" sentinel id (or
//! `None`) for empty names so callers can stay branch-free.

use crate::error::AppError;

/// Find or create an artist by name, returning the artist ID.
/// Uses the provided `unknown_artist_id` for empty artist names.
pub async fn upsert_artist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    unknown_artist_id: i64,
) -> Result<i64, AppError> {
    if name.is_empty() {
        return Ok(unknown_artist_id);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO artists (name) VALUES (?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id"
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Find or create an album by name and artist, returning the album ID.
/// Returns None if the album name is empty.
pub async fn upsert_album(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    artist_id: i64,
    year: Option<i32>,
) -> Result<Option<i64>, AppError> {
    if name.is_empty() {
        return Ok(None);
    }
    let id = sqlx::query_scalar::<_, i64>(
        // `year = COALESCE(excluded.year, albums.year)` updates the stored year on
        // re-ingest (e.g. a tag edit) but preserves it when the new value is NULL.
        // `excluded.year` / `albums.year` reference already-present columns, so no
        // extra bind — the (name, artist_id, year) bind order is unchanged.
        "INSERT INTO albums (name, artist_id, year) VALUES (?, ?, ?)
         ON CONFLICT(name, artist_id) DO UPDATE SET
             name = excluded.name,
             year = COALESCE(excluded.year, albums.year)
         RETURNING id"
    )
    .bind(name)
    .bind(artist_id)
    .bind(year)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(id))
}

/// Find or create a genre by name, returning the genre ID.
/// Returns None if the genre name is empty.
pub async fn upsert_genre(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<Option<i64>, AppError> {
    if name.is_empty() {
        return Ok(None);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO genres (name) VALUES (?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id"
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(id))
}
