use std::collections::HashMap;

use sqlx::AssertSqlSafe;

use crate::database::{chunked_in_query, DbPool};
use crate::entities::track;
use crate::error::AppError;

/// The `ORDER BY` body every "most played" surface ranks with — the Favorites
/// grid tab and the hero mosaic that has to agree with it.
///
/// `play_count DESC` is the ranking; the three keys after it are what make it
/// *total*. Without them `SQLite` may hand back tied rows in any order, so the
/// grid could re-order between refreshes and the mosaic — a second query over
/// the same rows — had no reason to pick the same four covers. `last_played
/// DESC` breaks a tie toward the one played most recently (the column is
/// RFC-3339 text whose lexical order is chronological, per
/// [`get_recently_played`]; NULLs sort last under `DESC`, so a row whose count
/// predates the timestamp falls to the back of its group). `date_added DESC` is
/// only reachable among never-played favourites, where it keeps the mosaic's
/// fill newest-first. `id ASC` closes the last gap.
const MOST_PLAYED_ORDER: &str = "play_count DESC, last_played DESC, date_added DESC, id ASC";

/// Build the body of an `ORDER BY` clause for the Tracks list (used by
/// `get_all_tracks`, `get_all_tracks_for_list`, and
/// `get_favorite_tracks_for_list`). Returns the clause without the
/// leading `ORDER BY` so callers can compose it into longer queries.
///
/// Centralised so adding a new sort column or tweaking collation lands
/// in one place. `sort_by == Some("track_number")` produces a
/// disc-then-track-then-title ordering; everything else maps to a
/// single column with `COLLATE NOCASE` on the four text columns.
///
/// Returns a `&'static str` — the inputs come from a fixed enum-bounded
/// set, so every distinct output is enumerated below as a static literal.
/// "title" and unrecognised values fall through to `sort_key` (the
/// natural-ordering key built at scan time from title/artist/album).
fn track_list_order_by(sort_by: Option<&str>, sort_dir: Option<&str>) -> &'static str {
    let desc = sort_dir == Some("desc");
    match sort_by {
        Some("track_number") => {
            if desc {
                "COALESCE(NULLIF(disc_number, 0), 1) ASC, \
                 CASE WHEN track_number IS NULL OR track_number = 0 THEN 2147483647 ELSE track_number END DESC, \
                 sort_key COLLATE NOCASE ASC"
            } else {
                "COALESCE(NULLIF(disc_number, 0), 1) ASC, \
                 CASE WHEN track_number IS NULL OR track_number = 0 THEN 2147483647 ELSE track_number END ASC, \
                 sort_key COLLATE NOCASE ASC"
            }
        }
        Some("artist") => if desc { "artist COLLATE NOCASE DESC" } else { "artist COLLATE NOCASE ASC" },
        Some("album") => if desc { "album COLLATE NOCASE DESC" } else { "album COLLATE NOCASE ASC" },
        Some("genre") => if desc { "genre COLLATE NOCASE DESC" } else { "genre COLLATE NOCASE ASC" },
        Some("length") => if desc { "duration_ms DESC" } else { "duration_ms ASC" },
        Some("date_added") => if desc { "date_added DESC" } else { "date_added ASC" },
        Some("play_count") => if desc { "play_count DESC" } else { "play_count ASC" },
        Some("year") => if desc { "year DESC" } else { "year ASC" },
        _ => if desc { "sort_key COLLATE NOCASE DESC" } else { "sort_key COLLATE NOCASE ASC" },
    }
}

// The four `SELECT *` variants below are kept for unit-test fixtures only
// (they return a full `Track`, which assertions need). Production list views
// always use the `_for_list` variants further down which fetch the
// `TrackListRow` projection.
#[cfg(test)]
pub async fn get_all_tracks(
    db: &DbPool,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<track::Track>, AppError> {
    let order_by = track_list_order_by(sort_by.as_deref(), sort_dir.as_deref());
    let sql = format!("SELECT * FROM tracks ORDER BY {order_by}");
    let tracks = sqlx::query_as::<_, track::Track>(AssertSqlSafe(sql))
        .fetch_all(db.read())
        .await?;
    Ok(tracks)
}

#[cfg(test)]
pub async fn get_tracks_by_album(db: &DbPool, album_id: i64) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(
        "SELECT * FROM tracks WHERE album_id = ? ORDER BY disc_number ASC, track_number ASC"
    )
    .bind(album_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

#[cfg(test)]
pub async fn get_tracks_by_artist(db: &DbPool, artist_id: i64) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(
        "SELECT * FROM tracks WHERE artist_id = ? ORDER BY sort_key COLLATE NOCASE ASC"
    )
    .bind(artist_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

#[cfg(test)]
pub async fn get_tracks_by_genre(db: &DbPool, genre_id: i64) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(
        "SELECT * FROM tracks WHERE genre_id = ? ORDER BY sort_key COLLATE NOCASE ASC"
    )
    .bind(genre_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

#[cfg(test)]
pub async fn get_track_by_id(db: &DbPool, id: i64) -> Result<track::Track, AppError> {
    sqlx::query_as::<_, track::Track>("SELECT * FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Track", id))
}

/// Fetch tracks by IDs, preserving the input order.
#[cfg(test)]
pub async fn get_tracks_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<track::Track>, AppError> {
    let tracks: Vec<track::Track> = chunked_in_query(
        db.read(),
        ids,
        |placeholders| format!("SELECT * FROM tracks WHERE id IN ({placeholders})"),
    )
    .await?;

    // Re-order results to match the input ID order
    let mut track_map: HashMap<i64, track::Track> = HashMap::with_capacity(tracks.len());
    track_map.extend(tracks.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| track_map.remove(id)).collect())
}

/// Technical-metadata projection for the full-screen Now Playing view's
/// chip row — reads only the 8 columns `TrackMetaRow` consumes (vs
/// `get_track_by_id`'s full 41-column `SELECT *`). Returns `None` for a
/// missing id; the caller renders empty chips.
pub async fn get_track_meta(
    db: &DbPool,
    id: i64,
) -> Result<Option<track::TrackMeta>, AppError> {
    let cols = track::track_meta_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE id = ? LIMIT 1");
    let row: Option<track::TrackMeta> = sqlx::query_as::<_, track::TrackMeta>(AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(db.read())
        .await?;
    Ok(row)
}

/// Single-id fetch of the columns a scrobble needs, for the detector's
/// per-track-start enrichment. Sibling of `get_track_meta`; returns `None` for
/// a missing id (the detector then skips the scrobble).
pub async fn get_scrobble_row(db: &DbPool, id: i64) -> Result<Option<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE id = ? LIMIT 1");
    let row: Option<track::ScrobbleRow> = sqlx::query_as::<_, track::ScrobbleRow>(AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(db.read())
        .await?;
    Ok(row)
}

/// Every favorited track's scrobble projection — the bulk source for the
/// retroactive love backfill (enable a love toggle / connect a service and
/// existing favorites sync without re-toggling each heart). Carries the
/// `MusicBrainz` ids `ScrobbleRow` needs for the `ListenBrainz` love path.
pub async fn get_favorite_scrobble_rows(db: &DbPool) -> Result<Vec<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE is_favorite = TRUE");
    let rows = sqlx::query_as::<_, track::ScrobbleRow>(AssertSqlSafe(sql))
        .fetch_all(db.read())
        .await?;
    Ok(rows)
}

/// Bulk sibling of [`get_scrobble_row`]: the scrobble projection for an
/// arbitrary id set in one chunked `IN (…)` query. The favorite→love sync uses
/// it to enrich a multi-selection without a per-id round-trip. Love order is
/// irrelevant, so rows come back in query order (no order-preserving re-walk).
pub async fn get_scrobble_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    chunked_in_query(
        db.read(),
        ids,
        |placeholders| format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})"),
    )
    .await
}

/// Fetch just the `file_path` column for a single track id — the
/// leanest possible projection, used by the "Open Containing Folder"
/// context-menu action which only needs the on-disk location. Returns
/// `None` for a missing id.
pub async fn get_track_file_path(db: &DbPool, id: i64) -> Result<Option<String>, AppError> {
    let path: Option<String> = sqlx::query_scalar("SELECT file_path FROM tracks WHERE id = ? LIMIT 1")
        .bind(id)
        .fetch_optional(db.read())
        .await?;
    Ok(path)
}

/// Fetch `TrackSummary` projections by IDs, preserving the input order.
/// Reads only the 12 columns the queue / now-playing / playback paths
/// actually consume — vs `get_tracks_by_ids` which reads all 41. Callers
/// that immediately do `tracks.into_iter().map(|t| t.into()).collect()`
/// into `TrackSummary` should use this instead.
pub async fn get_track_summaries_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TrackSummary>, AppError> {
    let cols = track::track_summary_columns();
    let summaries: Vec<track::TrackSummary> = chunked_in_query(
        db.read(),
        ids,
        |placeholders| format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})"),
    )
    .await?;

    let mut map: HashMap<i64, track::TrackSummary> = HashMap::with_capacity(summaries.len());
    map.extend(summaries.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Fetch `TagEditRow` projections by IDs for the Edit-Track-Information
/// dialog, preserving the input order. Reads the editable tag columns plus
/// the read-only technical columns the Summary tab shows — no joins, since
/// artist/album/genre are stored denormalized on `tracks`.
pub async fn get_tag_edit_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TagEditRow>, AppError> {
    let cols = track::track_tag_edit_columns();
    let rows: Vec<track::TagEditRow> = chunked_in_query(
        db.read(),
        ids,
        |placeholders| format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})"),
    )
    .await?;

    let mut map: HashMap<i64, track::TagEditRow> = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Fetch `(id, file_path)` pairs by IDs, preserving the input order. The tag
/// writer marks each `file_path` in the self-write suppression set before
/// rewriting it, so a stable order keeps the marking deterministic.
pub async fn get_track_paths_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, String)>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(
        db.read(),
        ids,
        |placeholders| format!("SELECT id, file_path FROM tracks WHERE id IN ({placeholders})"),
    )
    .await?;

    let mut map: HashMap<i64, String> = HashMap::with_capacity(rows.len());
    map.extend(rows);

    Ok(ids
        .iter()
        .filter_map(|id| map.remove(id).map(|path| (*id, path)))
        .collect())
}

/// Lightweight version of `get_all_tracks` returning only list-view columns.
pub async fn get_all_tracks_for_list(
    db: &DbPool,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let order_by = track_list_order_by(sort_by.as_deref(), sort_dir.as_deref());
    let sql = format!("SELECT {cols} FROM tracks ORDER BY {order_by}");
    let tracks = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .fetch_all(db.read())
        .await?;
    Ok(tracks)
}

/// Lightweight version of `get_tracks_by_album` for list views.
pub async fn get_tracks_by_album_for_list(db: &DbPool, album_id: i64) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let tracks = sqlx::query_as::<_, track::TrackListRow>(
        AssertSqlSafe(format!("SELECT {cols} FROM tracks WHERE album_id = ? ORDER BY disc_number ASC, track_number ASC"))
    )
    .bind(album_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

/// Lightweight version of `get_tracks_by_artist` for list views.
pub async fn get_tracks_by_artist_for_list(db: &DbPool, artist_id: i64) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let tracks = sqlx::query_as::<_, track::TrackListRow>(
        AssertSqlSafe(format!("SELECT {cols} FROM tracks WHERE artist_id = ? ORDER BY sort_key COLLATE NOCASE ASC"))
    )
    .bind(artist_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

/// Lightweight version of `get_tracks_by_genre` for list views.
pub async fn get_tracks_by_genre_for_list(db: &DbPool, genre_id: i64) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let tracks = sqlx::query_as::<_, track::TrackListRow>(
        AssertSqlSafe(format!("SELECT {cols} FROM tracks WHERE genre_id = ? ORDER BY sort_key COLLATE NOCASE ASC"))
    )
    .bind(genre_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

pub async fn update_play_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    let now = crate::utils::now_rfc3339();
    sqlx::query(
        "UPDATE tracks SET play_count = play_count + 1, last_played = ? WHERE id = ?"
    )
    .bind(now)
    .bind(id)
    .execute(db.write())
    .await?;
    Ok(())
}

pub async fn update_skip_count(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET skip_count = skip_count + 1 WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

pub async fn update_last_position(db: &DbPool, id: i64, position_ms: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET last_position = ? WHERE id = ?")
        .bind(position_ms)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Set `is_favorite` for one or more tracks by ID.
pub async fn set_favorite(db: &DbPool, ids: &[i64], favorite: bool) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `favorite` parameter itself.
    for chunk in ids.chunks(crate::database::SQLITE_BIND_LIMIT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "UPDATE tracks SET is_favorite = ? WHERE id IN ({placeholders})"
        );
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(favorite);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(db.write()).await?;
    }
    Ok(())
}

/// Set the star `rating` (0–5) for one or more tracks by ID. Mirrors
/// [`set_favorite`]: the value is clamped 0–5 by the caller
/// (`library::ratings::set_rating`), chunked to respect the `SQLite` bind
/// limit (one slot reserved for the `rating` bind), and run non-persistently
/// since the placeholder count is dynamic.
pub async fn set_rating(db: &DbPool, ids: &[i64], rating: i32) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `rating` parameter itself.
    for chunk in ids.chunks(crate::database::SQLITE_BIND_LIMIT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET rating = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(rating);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(db.write()).await?;
    }
    Ok(())
}

/// Authoritatively set `artwork_path` for one or more tracks by ID, within a
/// caller-supplied transaction. Unlike `update_track_metadata`'s
/// `artwork_path = COALESCE(?, artwork_path)`, this is a plain overwrite — so
/// `None` genuinely nulls the column (the artwork-Remove case), which a
/// COALESCE can never do. Tx-scoped because the tag-edit orchestrator writes
/// it in the same transaction as the metadata refresh.
pub async fn set_track_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    // Reserve 1 bind slot for the `artwork_path` parameter itself.
    for chunk in ids.chunks(crate::database::SQLITE_BIND_LIMIT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET artwork_path = ? WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql))
            .persistent(false)
            .bind(artwork_path);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Fetch all favorite tracks (lightweight list-view columns) with sorting.
pub async fn get_favorite_tracks_for_list(
    db: &DbPool,
    sort_by: Option<String>,
    sort_dir: Option<String>,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let order_by = track_list_order_by(sort_by.as_deref(), sort_dir.as_deref());
    let sql =
        format!("SELECT {cols} FROM tracks WHERE is_favorite = TRUE ORDER BY {order_by}");
    let tracks = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .fetch_all(db.read())
        .await?;
    Ok(tracks)
}

/// Fetch tracks whose files live directly inside `dir_path` (not in
/// subdirectories). Returns the `TrackListRow` projection (19 columns) — the
/// Browse view renders these through the shared `TrackList` component, so it
/// needs the same column set the Tracks view uses (genre / year / favourite /
/// foreign-key ids), not a narrower browse-only slice.
///
/// Platform invariant: `tracks.file_path` is stored verbatim from
/// `Path::to_string_lossy()` at scan time, which uses the platform's native
/// separator (`/` on Unix, `\` on Windows). The LIKE pattern below matches that
/// — pass `dir_path` in native form too (callers route through
/// `dunce::canonicalize` / `crate::utils::canonicalize_path`).
///
/// Query-plan note: `file_path` has a UNIQUE auto-index, but the `ESCAPE`
/// clause disables `SQLite`'s prefix-`LIKE`→range-scan optimisation
/// entirely (verified with `EXPLAIN QUERY PLAN`: `SCAN tracks USING INDEX
/// sqlite_autoindex_tracks_1`). At realistic library sizes (hundreds–low
/// thousands of tracks) a one-off index scan per *navigation* is sub-ms
/// and not worth a generated `parent_dir` column + index (which would slow
/// every insert — see CLAUDE.md "don't over-index"). Revisit only if a
/// profiler flags this at a much larger library size.
pub async fn get_tracks_in_directory(
    db: &DbPool,
    dir_path: &str,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let escaped = dir_path.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    // Native path separator inside the LIKE pattern. `\` is the ESCAPE
    // character (per the `ESCAPE '\'` clause below), so on Windows the
    // separator must be doubled to denote a literal `\` rather than an
    // escape sequence.
    #[cfg(windows)]
    let sep = "\\\\";
    #[cfg(not(windows))]
    let sep = "/";
    let pattern = format!("{escaped}{sep}%");
    let subdir_pattern = format!("{escaped}{sep}%{sep}%");

    let cols = track::track_list_columns();
    let sql = format!(
        "SELECT {cols} FROM tracks WHERE file_path LIKE ? ESCAPE '\\' AND file_path NOT LIKE ? ESCAPE '\\'"
    );
    let tracks = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(&pattern)
        .bind(&subdir_pattern)
        .fetch_all(db.read())
        .await?;

    Ok(tracks)
}

/// Look up track IDs by file paths. Returns a map from `file_path` → track ID.
pub async fn get_track_ids_by_paths(
    db: &DbPool,
    paths: &[String],
) -> Result<HashMap<String, i64>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(
        db.read(),
        paths,
        |placeholders| format!("SELECT id, file_path FROM tracks WHERE file_path IN ({placeholders})"),
    )
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, path)| (path, id)));
    Ok(map)
}

/// Look up track IDs by BLAKE3 `file_hash`. Returns a map from `file_hash`
/// → track ID. Uses the partial index `idx_tracks_file_hash`. Hashes with
/// no match are simply absent from the map. If two tracks share a hash
/// (true content duplicates), one arbitrary id wins — acceptable for
/// playlist re-matching since either copy plays identical audio.
pub async fn get_track_ids_by_hashes(
    db: &DbPool,
    hashes: &[String],
) -> Result<HashMap<String, i64>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(
        db.read(),
        hashes,
        |placeholders| {
            format!(
                "SELECT id, file_hash FROM tracks \
                 WHERE file_hash IS NOT NULL AND file_hash IN ({placeholders})"
            )
        },
    )
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, hash)| (hash, id)));
    Ok(map)
}

/// Find groups of tracks that share the same `file_hash` (duplicates).
/// Returns a Vec of groups, where each group is a Vec of tracks with the same hash.
///
/// One query joins each track to the duplicate-hash CTE; rows arrive
/// pre-sorted by `(file_hash, date_added)` so we can group consecutive
/// equal hashes in Rust without a `HashMap`.
#[cfg(test)]
pub async fn get_duplicate_tracks(
    db: &DbPool,
) -> Result<Vec<Vec<crate::entities::track::Track>>, AppError> {
    let rows: Vec<crate::entities::track::Track> = sqlx::query_as(
        "WITH dup_hashes AS (
             SELECT file_hash FROM tracks
             WHERE file_hash IS NOT NULL
             GROUP BY file_hash HAVING COUNT(*) > 1
         )
         SELECT t.* FROM tracks t
         JOIN dup_hashes d ON t.file_hash = d.file_hash
         ORDER BY t.file_hash, t.date_added ASC",
    )
    .fetch_all(db.read())
    .await?;

    let mut groups: Vec<Vec<crate::entities::track::Track>> = Vec::new();
    let mut cur_hash: Option<String> = None;
    for t in rows {
        // The CTE filter guarantees file_hash is Some for every row returned.
        let h = t.file_hash.clone().unwrap_or_default();
        match groups.last_mut() {
            Some(group) if cur_hash.as_deref() == Some(h.as_str()) => group.push(t),
            _ => {
                cur_hash = Some(h);
                groups.push(vec![t]);
            }
        }
    }
    Ok(groups)
}

/// Get all file paths of tracks that have no `file_hash` (for retroactive hashing).
pub async fn get_unhashed_track_paths(
    db: &DbPool,
) -> Result<Vec<(i64, String)>, AppError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, file_path FROM tracks WHERE file_hash IS NULL",
    )
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// Tracks with no `MusicBrainz` Recording ID that carry enough metadata to be
/// looked up — the work-list for the auto-tag backfill. Rows without an artist
/// or title can't be resolved, so they're excluded rather than attempted and
/// skipped. Columns: `(id, file_path, artist, title, album)`.
pub async fn get_tracks_missing_mbid(
    db: &DbPool,
) -> Result<Vec<(i64, String, String, String, Option<String>)>, AppError> {
    let rows = sqlx::query_as(
        "SELECT id, file_path, artist, title, album
         FROM tracks
         WHERE (musicbrainz_track_id IS NULL OR musicbrainz_track_id = '')
           AND artist IS NOT NULL AND artist <> ''
           AND title <> ''",
    )
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// Batch-update `file_hash` and `date_modified` for tracks by ID.
/// All chunks share a single transaction to amortize fsync under
/// `synchronous=NORMAL` — fast for retroactive backfills on large libraries.
///
/// Each chunk runs as a single CTE-driven UPDATE so the round-trip count
/// is O(chunks), not O(rows). Chunk size is bounded by `SQLite`'s 999-bind
/// limit divided by 3 columns per row.
pub async fn batch_update_hashes(
    db: &DbPool,
    updates: &[(i64, String, Option<String>)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 3;
    const CHUNK_SIZE: usize = crate::database::SQLITE_BIND_LIMIT / COLS_PER_ROW; // 333

    if updates.is_empty() {
        return Ok(());
    }

    let mut tx = db.write().begin().await?;
    for chunk in updates.chunks(CHUNK_SIZE) {
        let placeholders = std::iter::repeat_n("(?,?,?)", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "WITH v(id, h, m) AS (VALUES {placeholders})
             UPDATE tracks
                SET file_hash = (SELECT h FROM v WHERE v.id = tracks.id),
                    date_modified = (SELECT m FROM v WHERE v.id = tracks.id)
              WHERE id IN (SELECT id FROM v)"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for (id, hash, mtime) in chunk {
            q = q.bind(id).bind(hash).bind(mtime);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Aggregate stats for the Favorites hero header: count, total duration, up to 4 artwork paths.
pub async fn get_favorite_stats(db: &DbPool) -> Result<track::FavoriteStats, AppError> {
    let (count, total_duration_ms): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks WHERE is_favorite = TRUE",
    )
    .fetch_one(db.read())
    .await?;

    // Up to 4 *distinct* artworks for the live-updating Favorites hero mosaic:
    // walk the favourites in `MOST_PLAYED_ORDER` and keep each cover's best
    // slot, which makes the mosaic literally the head of the Most Played tab
    // rather than a second ranking that has to be kept in step with it. A
    // hand-matched `ORDER BY` over `MAX(play_count)` was that second ranking,
    // and it disagreed with the grid on every tie.
    //
    // The rank spans *all* favourites, not just `play_count > 0` as the grid
    // does, so once the played covers run out the remainder pads the mosaic to
    // 4 — the same fill the previous query gave, since unplayed rows sort last
    // under `play_count DESC`. Duplicates are deliberately not synthesised: the
    // consuming `CoverMosaic` renders an empty placeholder for the slots beyond
    // what this returns when `pad-to-four` is set, so a one-album-heavy library
    // shows 1 real cover + 3 placeholders rather than the same artwork 4 times.
    let artwork_paths: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT artwork_path FROM ( \
            SELECT artwork_path, \
                   ROW_NUMBER() OVER (ORDER BY {MOST_PLAYED_ORDER}) AS pos \
              FROM tracks \
             WHERE is_favorite = TRUE AND artwork_path IS NOT NULL AND artwork_path <> '' \
         ) \
         GROUP BY artwork_path \
         ORDER BY MIN(pos) \
         LIMIT 4"
    )))
    .fetch_all(db.read())
    .await?;

    Ok(track::FavoriteStats {
        count,
        total_duration_ms,
        artwork_paths,
    })
}

/// Favorite tracks by play count, most played first (only those with
/// `play_count` > 0). Ordered by [`MOST_PLAYED_ORDER`], which the hero mosaic
/// ranks with too — the two surfaces are the same list, so they resolve a tie
/// the same way or they visibly disagree.
///
/// Returns the whole set rather than a top N — the Most Played tab is a
/// virtualized grid, so it has nothing to gain by truncating. The partial
/// `idx_tracks_play_count` still drives the scan and supplies the leading
/// term; the three tiebreakers cost a sort within each tied group.
pub async fn get_most_played_favorites(
    db: &DbPool,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    // The order's trailing keys are read, not selected — `MostPlayedFavorite`
    // is the card projection and has no slot for a timestamp.
    let rows = sqlx::query_as::<_, track::MostPlayedFavorite>(AssertSqlSafe(format!(
        "SELECT id, title, artist, artwork_path, play_count, duration_ms \
           FROM tracks \
          WHERE is_favorite = TRUE AND play_count > 0 \
          ORDER BY {MOST_PLAYED_ORDER}"
    )))
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// Top N tracks by play count across the whole library (only those with
/// `play_count` > 0). Sibling of [`get_most_played_favorites`] without the
/// `is_favorite` filter — drives the Recently-Played view's "Most Played"
/// strip. Reuses the generic `MostPlayedFavorite` card projection, and the same
/// [`MOST_PLAYED_ORDER`] — the strip re-fetches on every `stats_changed` tick,
/// so an order that left ties to the planner could reshuffle the cards under
/// the user with nothing about the library having moved.
pub async fn get_most_played(
    db: &DbPool,
    limit: i64,
) -> Result<Vec<track::MostPlayedFavorite>, AppError> {
    let rows = sqlx::query_as::<_, track::MostPlayedFavorite>(AssertSqlSafe(format!(
        "SELECT id, title, artist, artwork_path, play_count, duration_ms \
           FROM tracks \
          WHERE play_count > 0 \
          ORDER BY {MOST_PLAYED_ORDER} \
          LIMIT ?"
    )))
    .bind(limit)
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// The N most-recently-played tracks (list-view projection), newest first.
///
/// Ordering rides on `last_played` (RFC-3339 UTC, written by the play-count
/// flusher / `update_play_count`). Lexical `TEXT` order == chronological order,
/// so `ORDER BY last_played DESC` is correct: the fixed-width date/time prefix
/// and constant `+00:00` offset aside, `chrono`'s `to_rfc3339` emits
/// *variable*-width fractional seconds (`AutoSi` → 0/3/6/9 digits), but that is
/// still order-safe — the offset `'+'` (0x2B) sorts below every digit, and
/// `AutoSi` only ever drops trailing-zero fractional groups. Rows without a
/// `last_played` (never played) are excluded — index-backed by the partial
/// `idx_tracks_last_played`.
///
/// Returns the `TrackListRow` projection so the Recently-Played view can render
/// through the shared `TrackList`; `last_played` itself is not selected (it is
/// only the sort key). Membership is the top `limit` by recency — the view
/// re-sorts/filters this set in memory, it never re-queries.
pub async fn get_recently_played(
    db: &DbPool,
    limit: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let sql = format!(
        "SELECT {cols} FROM tracks \
         WHERE last_played IS NOT NULL \
         ORDER BY last_played DESC \
         LIMIT ?"
    );
    let rows = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(db.read())
        .await?;
    Ok(rows)
}

#[cfg(test)]
#[path = "tests/track_tests.rs"]
mod tests;
