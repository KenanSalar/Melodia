use melodia_core::error::AppError;

// ── Trigger SQL constants ───────────────────────────────────────────────────
// Must stay in sync with the stats triggers in
// migrations/20260514000000_initial_schema.sql.

const CREATE_TRACKS_STATS_INSERT: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

UPDATE genres
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.genre_id;

END
";

const CREATE_TRACKS_STATS_DELETE: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_delete AFTER DELETE ON tracks BEGIN
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE genres
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.genre_id;

END
";

const CREATE_TRACKS_STATS_UPDATE: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_update AFTER
UPDATE OF artist_id,
album_id,
genre_id,
duration_ms ON tracks BEGIN
-- Decrement old parent stats
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.artist_id;

UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE genres
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.genre_id;

-- Increment new parent stats
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

UPDATE genres
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.genre_id;

END
";

/// Drop the three stats-maintenance triggers so bulk inserts skip per-row
/// overhead. Call [`enable_stats_triggers`] and [`recalculate_all_stats`]
/// after the bulk operation completes.
///
/// Safe inside a transaction — `SQLite` DDL is transactional, so a rollback
/// restores the triggers automatically.
pub async fn disable_stats_triggers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query("DROP TRIGGER IF EXISTS tracks_stats_insert").execute(&mut **tx).await?;
    sqlx::query("DROP TRIGGER IF EXISTS tracks_stats_delete").execute(&mut **tx).await?;
    sqlx::query("DROP TRIGGER IF EXISTS tracks_stats_update").execute(&mut **tx).await?;
    Ok(())
}

/// Recreate the three stats-maintenance triggers dropped by
/// [`disable_stats_triggers`].
pub async fn enable_stats_triggers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(CREATE_TRACKS_STATS_INSERT).execute(&mut **tx).await?;
    sqlx::query(CREATE_TRACKS_STATS_DELETE).execute(&mut **tx).await?;
    sqlx::query(CREATE_TRACKS_STATS_UPDATE).execute(&mut **tx).await?;
    Ok(())
}

/// Recalculate all denormalized stats (`track_count`, `total_duration_ms`,
/// `album_count`) on artists, albums, and genres from scratch.
///
/// Run this after a bulk ingest with triggers disabled.
pub async fn recalculate_all_stats(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    // Artists: track_count, total_duration_ms, album_count
    sqlx::query(
        "UPDATE artists SET
            track_count = COALESCE((SELECT COUNT(*) FROM tracks WHERE artist_id = artists.id), 0),
            total_duration_ms = COALESCE((SELECT SUM(duration_ms) FROM tracks WHERE artist_id = artists.id), 0),
            album_count = COALESCE((SELECT COUNT(*) FROM albums WHERE artist_id = artists.id), 0)"
    )
    .execute(&mut **tx)
    .await?;

    // Albums: track_count, total_duration_ms
    sqlx::query(
        "UPDATE albums SET
            track_count = COALESCE((SELECT COUNT(*) FROM tracks WHERE album_id = albums.id), 0),
            total_duration_ms = COALESCE((SELECT SUM(duration_ms) FROM tracks WHERE album_id = albums.id), 0)"
    )
    .execute(&mut **tx)
    .await?;

    // Genres: track_count, total_duration_ms
    sqlx::query(
        "UPDATE genres SET
            track_count = COALESCE((SELECT COUNT(*) FROM tracks WHERE genre_id = genres.id), 0),
            total_duration_ms = COALESCE((SELECT SUM(duration_ms) FROM tracks WHERE genre_id = genres.id), 0)"
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/stats_tests.rs"]
mod tests;
