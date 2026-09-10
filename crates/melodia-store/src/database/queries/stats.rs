use melodia_core::error::AppError;

// ── Trigger SQL constants ───────────────────────────────────────────────────
// Must stay in sync with the stats triggers in
// migrations/20260911000000_tag_coverage.sql, which is where they were last rewritten.
//
// The artist and genre arms live on the three join tables rather than on `tracks`: both sets of
// counts mean "credited on" now, and a track carries a list rather than one name. What a `tracks`
// row can still move by itself is the duration, hence the two duration arms left in the update
// trigger.

const CREATE_TRACKS_STATS_INSERT: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

END
";

const CREATE_TRACKS_STATS_DELETE: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_delete AFTER DELETE ON tracks BEGIN
UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

END
";

const CREATE_TRACKS_STATS_UPDATE: &str = r"
CREATE TRIGGER IF NOT EXISTS tracks_stats_update AFTER
UPDATE OF album_id,
duration_ms ON tracks BEGIN
UPDATE artists
SET
    total_duration_ms = MAX(
        total_duration_ms + new.duration_ms - old.duration_ms,
        0
    )
WHERE
    id IN (
        SELECT
            artist_id
        FROM
            track_artists
        WHERE
            track_id = new.id
    );

UPDATE genres
SET
    total_duration_ms = MAX(
        total_duration_ms + new.duration_ms - old.duration_ms,
        0
    )
WHERE
    id IN (
        SELECT
            genre_id
        FROM
            track_genres
        WHERE
            track_id = new.id
    );

UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

END
";

const CREATE_TRACK_ARTISTS_STATS_INSERT: &str = r"
CREATE TRIGGER IF NOT EXISTS track_artists_stats_insert AFTER INSERT ON track_artists BEGIN
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + COALESCE(
        (
            SELECT
                duration_ms
            FROM
                tracks
            WHERE
                id = new.track_id
        ),
        0
    )
WHERE
    id = new.artist_id;

END
";

const CREATE_TRACK_ARTISTS_STATS_DELETE: &str = r"
CREATE TRIGGER IF NOT EXISTS track_artists_stats_delete AFTER DELETE ON track_artists BEGIN
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(
        total_duration_ms - COALESCE(
            (
                SELECT
                    duration_ms
                FROM
                    tracks
                WHERE
                    id = old.track_id
            ),
            0
        ),
        0
    )
WHERE
    id = old.artist_id;

END
";

const CREATE_TRACK_GENRES_STATS_INSERT: &str = r"
CREATE TRIGGER IF NOT EXISTS track_genres_stats_insert AFTER INSERT ON track_genres BEGIN
UPDATE genres
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + COALESCE(
        (
            SELECT
                duration_ms
            FROM
                tracks
            WHERE
                id = new.track_id
        ),
        0
    )
WHERE
    id = new.genre_id;

END
";

const CREATE_TRACK_GENRES_STATS_DELETE: &str = r"
CREATE TRIGGER IF NOT EXISTS track_genres_stats_delete AFTER DELETE ON track_genres BEGIN
UPDATE genres
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(
        total_duration_ms - COALESCE(
            (
                SELECT
                    duration_ms
                FROM
                    tracks
                WHERE
                    id = old.track_id
            ),
            0
        ),
        0
    )
WHERE
    id = old.genre_id;

END
";

const CREATE_ALBUM_ARTISTS_STATS_INSERT: &str = r"
CREATE TRIGGER IF NOT EXISTS album_artists_stats_insert AFTER INSERT ON album_artists BEGIN
UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            album_artists
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

END
";

const CREATE_ALBUM_ARTISTS_STATS_DELETE: &str = r"
CREATE TRIGGER IF NOT EXISTS album_artists_stats_delete AFTER DELETE ON album_artists BEGIN
UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            album_artists
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

END
";

/// Every stats-maintenance trigger: the name to drop it by, and the SQL to put it back. One array
/// rather than two, so a trigger cannot be added to half of the pair and silently survive a bulk
/// scan it was meant to sit out.
const STATS_TRIGGERS: [(&str, &str); 9] = [
    ("tracks_stats_insert", CREATE_TRACKS_STATS_INSERT),
    ("tracks_stats_delete", CREATE_TRACKS_STATS_DELETE),
    ("tracks_stats_update", CREATE_TRACKS_STATS_UPDATE),
    ("track_artists_stats_insert", CREATE_TRACK_ARTISTS_STATS_INSERT),
    ("track_artists_stats_delete", CREATE_TRACK_ARTISTS_STATS_DELETE),
    ("track_genres_stats_insert", CREATE_TRACK_GENRES_STATS_INSERT),
    ("track_genres_stats_delete", CREATE_TRACK_GENRES_STATS_DELETE),
    ("album_artists_stats_insert", CREATE_ALBUM_ARTISTS_STATS_INSERT),
    ("album_artists_stats_delete", CREATE_ALBUM_ARTISTS_STATS_DELETE),
];

/// Drop the stats-maintenance triggers so bulk inserts skip per-row overhead. Call
/// [`enable_stats_triggers`] and [`recalculate_all_stats`] after the bulk operation completes.
///
/// **`tracks_credits_cleanup` and `albums_credits_cleanup` are deliberately not in that set.**
/// They stand in for the `ON DELETE CASCADE` the credit tables don't have, so dropping them
/// wouldn't cost a stat, it would make a track undeletable.
///
/// Safe inside a transaction — `SQLite` DDL is transactional, so a rollback
/// restores the triggers automatically.
pub async fn disable_stats_triggers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    for (name, _) in STATS_TRIGGERS {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TRIGGER IF EXISTS {name}")))
            .persistent(false)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Recreate the stats-maintenance triggers dropped by [`disable_stats_triggers`].
pub async fn enable_stats_triggers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    for (_, sql) in STATS_TRIGGERS {
        sqlx::query(sql).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Recalculate all denormalized stats (`track_count`, `total_duration_ms`,
/// `album_count`) on artists, albums, and genres from scratch.
///
/// Run this after a bulk ingest with triggers disabled.
pub async fn recalculate_all_stats(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    // Artists: all three off the credit tables, which is where "how many" now lives.
    sqlx::query(
        "UPDATE artists SET
            track_count = COALESCE((SELECT COUNT(*) FROM track_artists WHERE artist_id = artists.id), 0),
            total_duration_ms = COALESCE((SELECT SUM(t.duration_ms) FROM track_artists ta JOIN tracks t ON t.id = ta.track_id WHERE ta.artist_id = artists.id), 0),
            album_count = COALESCE((SELECT COUNT(*) FROM album_artists WHERE artist_id = artists.id), 0)"
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

    // Genres: both off `track_genres`, which is where "how many" now lives.
    sqlx::query(
        "UPDATE genres SET
            track_count = COALESCE((SELECT COUNT(*) FROM track_genres WHERE genre_id = genres.id), 0),
            total_duration_ms = COALESCE((SELECT SUM(t.duration_ms) FROM track_genres tg JOIN tracks t ON t.id = tg.track_id WHERE tg.genre_id = genres.id), 0)"
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/stats_tests.rs"]
mod tests;
