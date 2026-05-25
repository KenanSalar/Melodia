//! Track-row write side: insert, metadata refresh, location-only update
//! (for moved/renamed files), bulk delete, and the album-artwork roll-up
//! that runs at the tail of every scan batch.

use crate::error::AppError;

use super::ResolvedIds;
use super::sort_key::to_natural_sort_key;

/// Update a track's artwork if it is currently missing.
pub async fn update_track_artwork_if_missing(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    artwork_path: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE tracks SET artwork_path = ?
         WHERE file_path = ? AND (artwork_path IS NULL OR artwork_path = '')"
    )
    .bind(artwork_path)
    .bind(file_path)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Bind the canonical 34-column block shared by `insert_track` and
/// `update_track_metadata`: 28 metadata fields (`file_hash` through
/// `artwork_path`), 4 foreign-key ids (`album_id`, `artist_id`, `genre_id`,
/// `folder_id`), then `date_modified` and `sort_key`. Both call sites place
/// these in the same order in their respective SQL so this single helper
/// covers schema changes in one place.
fn bind_track_columns<'q>(
    q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    meta: &'q crate::media::metadata::ExtractedMetadata,
    ids: &'q ResolvedIds,
    sort_key: &'q str,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    q.bind(&meta.file_hash)
        .bind(&meta.title)
        .bind(&meta.artist)
        .bind(&meta.album_artist)
        .bind(&meta.album)
        .bind(&meta.genre)
        .bind(meta.track_number)
        .bind(meta.disc_number)
        .bind(meta.year)
        .bind(&meta.composer)
        .bind(&meta.comment)
        .bind(meta.bpm)
        .bind(&meta.musicbrainz_track_id)
        .bind(&meta.musicbrainz_release_id)
        .bind(&meta.label)
        .bind(meta.original_year)
        .bind(meta.replaygain_track_gain)
        .bind(meta.replaygain_track_peak)
        .bind(meta.replaygain_album_gain)
        .bind(meta.replaygain_album_peak)
        .bind(meta.duration_ms)
        .bind(Some(meta.file_size))
        .bind(&meta.codec)
        .bind(meta.bitrate)
        .bind(meta.channels)
        .bind(meta.sample_rate)
        .bind(meta.bit_depth)
        .bind(&meta.artwork_path)
        .bind(ids.album_id)
        .bind(ids.artist_id)
        .bind(ids.genre_id)
        .bind(ids.folder_id)
        .bind(&meta.date_modified)
        .bind(sort_key)
}

/// Insert a new track into the database. Returns the new track's `id` so the
/// caller can collect inserted IDs without a follow-up `SELECT … WHERE
/// file_path IN (…)` round-trip.
/// `now` should be a pre-computed RFC3339 timestamp so all tracks in a batch share the same `date_added`.
pub async fn insert_track(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    file_name: &str,
    meta: &crate::media::metadata::ExtractedMetadata,
    ids: &ResolvedIds,
    now: &str,
) -> Result<i64, AppError> {
    let sort_key = to_natural_sort_key(&meta.title);

    // Column order: file_path, file_name, then the 34-column shared block
    // bound by `bind_track_columns`, then the playback defaults and date_added.
    let q = sqlx::query(
        "INSERT INTO tracks (
            file_path, file_name,
            file_hash, title, artist, album_artist, album, genre, track_number, disc_number, year, composer, comment,
            bpm, musicbrainz_track_id, musicbrainz_release_id, label, original_year,
            replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak,
            duration_ms, file_size, codec, bitrate, channels, sample_rate, bit_depth,
            artwork_path,
            album_id, artist_id, genre_id, folder_id,
            date_modified, sort_key,
            play_count, skip_count, rating, is_favorite, last_played, last_position,
            date_added
        ) VALUES (
            ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?,
            ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?,
            ?,
            ?, ?, ?, ?,
            ?, ?,
            ?, ?, ?, ?, ?, ?,
            ?
        )",
    )
    .bind(file_path)
    .bind(file_name);

    let q = bind_track_columns(q, meta, ids, &sort_key);

    let result = q
        .bind(0i32) // play_count
        .bind(0i32) // skip_count
        .bind(0i32) // rating
        .bind(false) // is_favorite
        .bind(None::<String>) // last_played
        .bind(0i64) // last_position
        .bind(now) // date_added
        .execute(&mut **tx)
        .await?;
    Ok(result.last_insert_rowid())
}

/// Update a track's location when it has been moved or renamed.
/// Preserves all playback state (`play_count`, rating, `is_favorite`, etc.).
pub async fn update_track_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    new_path: &str,
    new_file_name: &str,
    new_folder_id: i64,
    date_modified: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE tracks SET file_path = ?, file_name = ?, folder_id = ?, date_modified = ?
         WHERE id = ?",
    )
    .bind(new_path)
    .bind(new_file_name)
    .bind(new_folder_id)
    .bind(date_modified)
    .bind(track_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Update all metadata columns for an existing track that has changed on disk.
/// Preserves playback state (`play_count`, rating, `is_favorite`, `last_played`, `last_position`).
pub async fn update_track_metadata(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    meta: &crate::media::metadata::ExtractedMetadata,
    ids: &ResolvedIds,
) -> Result<(), AppError> {
    let sort_key = to_natural_sort_key(&meta.title);
    let q = sqlx::query(
        "UPDATE tracks SET
            file_hash = ?, title = ?, artist = ?, album_artist = ?, album = ?,
            genre = ?, track_number = ?, disc_number = ?, year = ?,
            composer = ?, comment = ?,
            bpm = ?, musicbrainz_track_id = ?, musicbrainz_release_id = ?,
            label = ?, original_year = ?,
            replaygain_track_gain = ?, replaygain_track_peak = ?,
            replaygain_album_gain = ?, replaygain_album_peak = ?,
            duration_ms = ?, file_size = ?, codec = ?, bitrate = ?,
            channels = ?, sample_rate = ?, bit_depth = ?,
            artwork_path = COALESCE(?, artwork_path),
            album_id = ?, artist_id = ?, genre_id = ?, folder_id = ?,
            date_modified = ?, sort_key = ?
         WHERE file_path = ?",
    );
    let q = bind_track_columns(q, meta, ids, &sort_key);
    q.bind(file_path).execute(&mut **tx).await?;
    Ok(())
}

/// Update albums that are missing artwork by pulling from their tracks.
/// Uses a CTE to calculate first artwork per album in a single pass,
/// avoiding a correlated subquery per album.
pub async fn update_album_artwork_from_tracks(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(
        "WITH first_art AS (
            SELECT album_id, artwork_path,
                   ROW_NUMBER() OVER (PARTITION BY album_id ORDER BY id) AS rn
            FROM tracks
            WHERE album_id IS NOT NULL
              AND artwork_path IS NOT NULL
              AND artwork_path != ''
        )
        UPDATE albums SET artwork_path = (
            SELECT artwork_path FROM first_art
            WHERE first_art.album_id = albums.id AND rn = 1
        )
        WHERE (artwork_path IS NULL OR artwork_path = '')
          AND id IN (SELECT album_id FROM first_art WHERE rn = 1)"
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete a single track by file path.
/// The `tracks_stats_delete` trigger fires automatically, keeping stats correct.
pub async fn delete_track_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM tracks WHERE file_path = ?")
        .bind(file_path)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Batch-delete tracks by file paths, respecting `SQLite`'s 999-parameter bind limit.
/// Returns the total number of rows deleted.
pub async fn delete_tracks_by_paths_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_paths: &[String],
) -> Result<u64, AppError> {
    if file_paths.is_empty() {
        return Ok(0);
    }

    let mut total_deleted: u64 = 0;
    for chunk in file_paths.chunks(crate::database::SQLITE_BIND_LIMIT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("DELETE FROM tracks WHERE file_path IN ({placeholders})");
        let mut query = sqlx::query(&sql);
        for path in chunk {
            query = query.bind(path);
        }
        let result = query.persistent(false).execute(&mut **tx).await?;
        total_deleted += result.rows_affected();
    }
    Ok(total_deleted)
}
