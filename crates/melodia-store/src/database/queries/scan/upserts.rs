//! FK helpers: upsert an `artist` / `album` / `genre` row by name and
//! return its rowid. Each returns the supplied "unknown" sentinel id (or
//! `None`) for empty names so callers can stay branch-free.

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

/// The credit an album files under: its own tag where it has one, else the track artist's
/// **first** name.
///
/// The whole track credit is the wrong fallback and the reason is the one the album-artist
/// fallback already exists for. A guest on one track is not an album artist, so taking
/// "X feat. Y" here would rename the album after whichever track happened to reach the upsert
/// first and list the album in Y's discography. Same answer as [`album_artist_name_for`], one
/// shape up.
#[must_use]
pub fn album_credit_for(meta: &ExtractedMetadata) -> ArtistCredit {
    if meta.album_artist.is_empty() {
        ArtistCredit::from_name(meta.artist.primary_name())
    } else {
        meta.album_artist.clone()
    }
}

/// The name behind [`album_credit_for`], for the caller that needs the grouping key on its own.
#[must_use]
pub fn album_artist_name_for(meta: &ExtractedMetadata) -> &str {
    match meta.album_artist.primary_name() {
        "" => meta.artist.primary_name(),
        name => name,
    }
}

/// Write a freshly inserted track's artist credit.
///
/// **Position 0 is the row's own `tracks.artist_id`.** Album grouping and the sort indexes still
/// read that FK, so the join table agreeing with it at the head is what stops an artist-scoped
/// query and an album-scoped one disagreeing about the same track. `primary_artist_id` covers the
/// one case the credit can't: a file with no artist tag, which resolves to the sentinel and still
/// gets its row rather than being credited to nobody.
pub async fn insert_track_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    write_credits(tx, "track_artists", "track_id", track_id, credit, primary_artist_id).await
}

/// [`insert_track_credits`] for a track that may already carry one: a re-ingest, or the credit
/// import. The insert paths take the sibling, a rowid the INSERT just minted having nothing to
/// clear, and a DELETE per row being a statement per track of a bulk scan.
pub async fn replace_track_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    clear_credits(tx, "track_artists", "track_id", track_id).await?;
    insert_track_credits(tx, track_id, credit, primary_artist_id).await
}

/// [`replace_track_credits`] for an album, plus the rendered credit `album_stats` displays.
///
/// No insert-only sibling: `upsert_album` reaches this down both arms of its `ON CONFLICT`, so the
/// rows may or may not be there and only the delete can tell.
pub async fn replace_album_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
    clear_credits(tx, "album_artists", "album_id", album_id).await?;
    write_credits(tx, "album_artists", "album_id", album_id, credit, primary_artist_id).await?;

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

/// Drop a parent's whole credit, ahead of writing the replacement.
///
/// Delete-then-insert rather than a diff: a credit is a handful of ordered rows, so a diff would
/// have to reconcile positions anyway and buys nothing but a way to leave the stats triggers out
/// of step.
async fn clear_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &'static str,
    parent_column: &'static str,
    parent_id: i64,
) -> Result<(), AppError> {
    sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE {parent_column} = ?")))
        .bind(parent_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// One row per credited name, in order, upserting the names it hasn't seen.
///
/// Position 0 takes `primary_artist_id` rather than an upsert of its own: every caller resolved
/// that id *from* this credit's first name, so the round trip would ask a question it is holding
/// the answer to. Which is also the invariant `album_credit_for` exists to keep true.
async fn write_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &'static str,
    parent_column: &'static str,
    parent_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
) -> Result<(), AppError> {
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
        let artist_id = if position == 0 {
            primary_artist_id
        } else {
            upsert_artist(tx, &credited.name, primary_artist_id).await?
        };
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
