//! FK helpers: upsert an `artist` / `album` / `genre` row by name and
//! return its rowid. Each returns the supplied "unknown" sentinel id (or
//! `None`) for empty names so callers can stay branch-free.

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::error::AppError;

/// Rewrite a track's artist credit: every name upserted, the ordered rows replaced.
///
/// **Position 0 is the row's own `tracks.artist_id`.** Album grouping and the sort indexes still
/// read that FK, so the join table agreeing with it at the head is what stops an artist-scoped
/// query and an album-scoped one disagreeing about the same track. `primary_artist_id` covers the
/// one case the credit can't: a file with no artist tag, which resolves to the sentinel and still
/// gets its row rather than being credited to nobody.
pub async fn replace_track_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    replace_credits(tx, "track_artists", "track_id", track_id, credit, primary_artist_id).await
}

/// [`replace_track_credits`] for an album, plus the rendered credit `album_stats` displays.
pub async fn replace_album_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    replace_credits(tx, "album_artists", "album_id", album_id, credit, primary_artist_id).await?;

    // NULL rather than the one name it would repeat, so `album_stats` falls back to the artist row
    // and a single-artist album keeps rendering from one place.
    let rendered = (credit.artists().len() > 1).then(|| credit.line().unwrap_or_default());
    sqlx::query("UPDATE albums SET artist_credit = ? WHERE id = ?")
        .bind(rendered)
        .bind(album_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Delete-then-insert rather than a diff: a credit is a handful of ordered rows, so a diff would
/// have to reconcile positions anyway and buys nothing but a way to leave the stats triggers out
/// of step.
async fn replace_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &'static str,
    parent_column: &'static str,
    parent_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE {parent_column} = ?")))
        .bind(parent_id)
        .execute(&mut **tx)
        .await?;

    let insert = format!(
        "INSERT INTO {table} ({parent_column}, artist_id, position, join_phrase)
         VALUES (?, ?, ?, ?)"
    );

    if credit.is_empty() {
        sqlx::query(sqlx::AssertSqlSafe(insert.as_str()))
            .bind(parent_id)
            .bind(primary_artist_id)
            .bind(0_i64)
            .bind("")
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }

    for (position, credited) in credit.artists().iter().enumerate() {
        let artist_id = upsert_artist(tx, &credited.name, primary_artist_id).await?;
        sqlx::query(sqlx::AssertSqlSafe(insert.as_str()))
            .bind(parent_id)
            .bind(artist_id)
            .bind(i64::try_from(position).unwrap_or(i64::MAX))
            .bind(&credited.join_phrase)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

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
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Find or create an album by name and artist, returning the album ID.
/// Returns None if the album name is empty.
///
/// `artist_id` is the album's grouping key, the primary name behind `credit`, and the two are
/// separate arguments because the caller has already resolved and cached the id. The credit rows
/// are written here rather than by the caller for the reason [`replace_track_credits`] gives: an
/// album row without them files under nobody.
pub async fn upsert_album(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    artist_id: i64,
    credit: &ArtistCredit,
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
         RETURNING id",
    )
    .bind(name)
    .bind(artist_id)
    .bind(year)
    .fetch_one(&mut **tx)
    .await?;
    replace_album_credits(tx, id, credit, artist_id).await?;
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
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(id))
}
