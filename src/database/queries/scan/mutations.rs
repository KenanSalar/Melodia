//! Track-row write side: insert (single + multi-row), metadata refresh,
//! location-only update (for moved/renamed files), bulk delete, and the
//! album-artwork roll-up that runs at the tail of every scan batch.

use std::collections::HashMap;

use crate::database::SQLITE_BIND_LIMIT;
use crate::error::AppError;
use crate::media::metadata::ExtractedMetadata;

use super::ResolvedIds;
use super::sort_key::to_natural_sort_key;

/// Column-name list shared by [`insert_track`]'s single-row INSERT and
/// [`insert_tracks_batch`]'s multi-row form. The bind order in
/// `bind_track_columns` (single-row) and `push_row_values` (multi-row)
/// MUST both match this list — change all three together.
const TRACK_INSERT_COLUMNS: &str = "file_path, file_name,
    file_hash, title, artist, album_artist, album, genre, track_number, disc_number, year, composer, comment,
    bpm, musicbrainz_track_id, musicbrainz_release_id, label, original_year,
    replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak,
    duration_ms, file_size, codec, bitrate, channels, sample_rate, bit_depth,
    artwork_path,
    album_id, artist_id, genre_id, folder_id,
    date_modified, sort_key,
    play_count, skip_count, rating, is_favorite, last_played, last_position,
    date_added";

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
    let sql = format!(
        "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES (
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
    );
    let q = sqlx::query(&sql).bind(file_path).bind(file_name);

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

/// One buffered row for [`insert_tracks_batch`]. `meta` borrows from the
/// scanned-file set the ingest loop iterates; the resolved ids are moved in.
pub struct NewTrackRow<'a> {
    pub file_path: String,
    pub file_name: String,
    pub meta: &'a ExtractedMetadata,
    pub ids: ResolvedIds,
}

/// Rows per multi-row INSERT statement: 37 binds each (`file_path` +
/// `file_name` + the 34-column shared block + `date_added`; the six
/// playback defaults are SQL literals), kept under `SQLite`'s bind cap.
pub const INSERT_CHUNK_ROWS: usize = SQLITE_BIND_LIMIT / 37;

/// Multi-row variant of [`insert_track`] for the scan/import ingest hot
/// path: one `INSERT … VALUES (…), (…), … RETURNING id, file_path` per
/// [`INSERT_CHUNK_ROWS`] rows instead of one statement per row. Returns
/// the new ids **in `rows` order** — `SQLite` documents `RETURNING`
/// output order as unspecified, so ids are mapped back via the returned
/// `file_path` (a UNIQUE column) rather than by position. Callers rely
/// on that ordering: the `DnD` import queues tracks in drop order.
pub async fn insert_tracks_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rows: &[NewTrackRow<'_>],
    now: &str,
) -> Result<Vec<i64>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(INSERT_CHUNK_ROWS) {
        let sort_keys: Vec<String> = chunk
            .iter()
            .map(|r| to_natural_sort_key(&r.meta.title))
            .collect();

        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!(
            "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) "
        ));
        qb.push_values(
            chunk.iter().zip(sort_keys.iter()),
            |mut b, (row, sort_key)| {
                let meta = row.meta;
                // Bind order mirrors `bind_track_columns` — see the
                // TRACK_INSERT_COLUMNS contract above.
                b.push_bind(&row.file_path)
                    .push_bind(&row.file_name)
                    .push_bind(&meta.file_hash)
                    .push_bind(&meta.title)
                    .push_bind(&meta.artist)
                    .push_bind(&meta.album_artist)
                    .push_bind(&meta.album)
                    .push_bind(&meta.genre)
                    .push_bind(meta.track_number)
                    .push_bind(meta.disc_number)
                    .push_bind(meta.year)
                    .push_bind(&meta.composer)
                    .push_bind(&meta.comment)
                    .push_bind(meta.bpm)
                    .push_bind(&meta.musicbrainz_track_id)
                    .push_bind(&meta.musicbrainz_release_id)
                    .push_bind(&meta.label)
                    .push_bind(meta.original_year)
                    .push_bind(meta.replaygain_track_gain)
                    .push_bind(meta.replaygain_track_peak)
                    .push_bind(meta.replaygain_album_gain)
                    .push_bind(meta.replaygain_album_peak)
                    .push_bind(meta.duration_ms)
                    .push_bind(Some(meta.file_size))
                    .push_bind(&meta.codec)
                    .push_bind(meta.bitrate)
                    .push_bind(meta.channels)
                    .push_bind(meta.sample_rate)
                    .push_bind(meta.bit_depth)
                    .push_bind(&meta.artwork_path)
                    .push_bind(row.ids.album_id)
                    .push_bind(row.ids.artist_id)
                    .push_bind(row.ids.genre_id)
                    .push_bind(row.ids.folder_id)
                    .push_bind(&meta.date_modified)
                    .push_bind(sort_key);
                // Playback defaults as SQL literals (play_count,
                // skip_count, rating, is_favorite, last_played,
                // last_position) — keeps the per-row bind count at 37.
                b.push("0")
                    .push("0")
                    .push("0")
                    .push("0")
                    .push("NULL")
                    .push("0")
                    .push_bind(now);
            },
        );
        qb.push(" RETURNING id, file_path");

        let returned: Vec<(i64, String)> = qb
            .build_query_as()
            .persistent(false)
            .fetch_all(&mut **tx)
            .await?;
        let mut by_path: HashMap<String, i64> =
            returned.into_iter().map(|(id, path)| (path, id)).collect();
        for row in chunk {
            let Some(id) = by_path.remove(row.file_path.as_str()) else {
                // Unreachable in practice — every VALUES row produces a
                // RETURNING row — but fail loudly rather than silently
                // desync `inserted_track_ids` from reality.
                return Err(AppError::Scanner(format!(
                    "multi-row insert returned no id for {}",
                    row.file_path
                )));
            };
            out.push(id);
        }
    }
    Ok(out)
}

/// Update a track's location when it has been moved or renamed.
/// Preserves all playback state (`play_count`, rating, `is_favorite`, etc.).
/// Returns `false` when no row with `track_id` exists — callers holding a
/// candidate id resolved *before* the transaction opened must treat that
/// as "the row vanished in-tx" and fall back to inserting.
pub async fn update_track_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    new_path: &str,
    new_file_name: &str,
    new_folder_id: i64,
    date_modified: Option<&str>,
) -> Result<bool, AppError> {
    let result = sqlx::query(
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
    Ok(result.rows_affected() > 0)
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
