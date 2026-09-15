//! Narrow projections by id, id lookups by path or hash, and the work lists the one-shot sweeps
//! page through.

use std::collections::HashMap;

use sqlx::AssertSqlSafe;

use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::track;
use melodia_core::error::AppError;

/// Technical-metadata projection for the full-screen Now Playing view's chip row — reads only the
/// columns `TrackMetaRow` consumes, against `get_track_by_id`'s full `SELECT *`. Returns `None`
/// for a missing id; the caller renders empty chips.
pub async fn get_track_meta(db: &DbPool, id: i64) -> Result<Option<track::TrackMeta>, AppError> {
    let cols = track::track_meta_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE id = ? LIMIT 1");
    let row: Option<track::TrackMeta> = sqlx::query_as::<_, track::TrackMeta>(AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(db.read())
        .await?;
    Ok(row)
}

/// Single-id fetch of the columns a scrobble needs, for the detector's per-track-start enrichment.
/// Sibling of `get_track_meta`; returns `None` for a missing id, and the detector then skips it.
pub async fn get_scrobble_row(
    db: &DbPool,
    id: i64,
) -> Result<Option<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE id = ? LIMIT 1");
    let row: Option<track::ScrobbleRow> =
        sqlx::query_as::<_, track::ScrobbleRow>(AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(db.read())
            .await?;
    Ok(row)
}

/// Every favorited track's scrobble projection — the bulk source for the retroactive love
/// backfill, so connecting a service syncs existing favorites without re-toggling each heart.
/// Carries the `MusicBrainz` ids `ScrobbleRow` needs for the `ListenBrainz` love path.
pub async fn get_favorite_scrobble_rows(db: &DbPool) -> Result<Vec<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    let sql = format!("SELECT {cols} FROM tracks WHERE is_favorite = TRUE");
    let rows =
        sqlx::query_as::<_, track::ScrobbleRow>(AssertSqlSafe(sql)).fetch_all(db.read()).await?;
    Ok(rows)
}

/// Bulk sibling of [`get_scrobble_row`]: the scrobble projection for an arbitrary id set in one
/// chunked `IN (…)` query, so the favorite→love sync enriches a multi-selection without a per-id
/// round-trip. Love order is irrelevant, so rows come back in query order.
pub async fn get_scrobble_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::ScrobbleRow>, AppError> {
    let cols = track::scrobble_row_columns();
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})")
    })
    .await
}

/// Fetch just the `file_path` column for a single track id — the leanest projection, for the "Open
/// Containing Folder" action. Returns `None` for a missing id.
pub async fn get_track_file_path(db: &DbPool, id: i64) -> Result<Option<String>, AppError> {
    let path: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM tracks WHERE id = ? LIMIT 1")
            .bind(id)
            .fetch_optional(db.read())
            .await?;
    Ok(path)
}

/// How many tracks the library holds. The diagnostics bundle reports it as library shape — a bug
/// that only shows up at scale is a different bug.
pub async fn count_tracks(db: &DbPool) -> Result<i64, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    Ok(count)
}

/// Fetch `TrackSummary` projections by IDs, preserving the input order. Reads only the columns the
/// queue / now-playing / playback paths consume, against `get_tracks_by_ids`' full row — a caller
/// that would immediately collect into `TrackSummary` wants this instead.
pub async fn get_track_summaries_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TrackSummary>, AppError> {
    let cols = track::track_summary_columns();
    let summaries: Vec<track::TrackSummary> = chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})")
    })
    .await?;

    let mut map: HashMap<i64, track::TrackSummary> = HashMap::with_capacity(summaries.len());
    map.extend(summaries.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Fetch `TagEditRow` projections by IDs for the Edit-Track-Information dialog, preserving the
/// input order. Reads the editable **single-valued** tag columns plus the read-only technical ones
/// the Summary tab shows, and joins nothing.
///
/// The multi-valued fields are deliberately absent: the dialog reads artists, genres and role
/// credits as rows, through `get_track_{credits,role_credits,genres}_by_ids`. Widening this
/// projection to carry one of their rendered columns instead is the regression — an editor
/// populated from `tracks.genre` splits `Chanson, Francaise` into two genres and saving makes that
/// permanent.
pub async fn get_tag_edit_rows_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TagEditRow>, AppError> {
    let cols = track::track_tag_edit_columns();
    let rows: Vec<track::TagEditRow> = chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})")
    })
    .await?;

    let mut map: HashMap<i64, track::TagEditRow> = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Fetch `(id, file_path)` pairs by IDs, preserving the input order. The tag writer marks each
/// `file_path` in the self-write suppression set before rewriting it, so a stable order keeps the
/// marking deterministic.
pub async fn get_track_paths_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, String)>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, file_path FROM tracks WHERE id IN ({placeholders})")
    })
    .await?;

    let mut map: HashMap<i64, String> = HashMap::with_capacity(rows.len());
    map.extend(rows);

    Ok(ids.iter().filter_map(|id| map.remove(id).map(|path| (*id, path))).collect())
}

/// Fetch `TrackLinks` projections by IDs, preserving the input order. The queue sheet renders from
/// `TrackSummary`, which carries no foreign keys, so its context menu resolves them here rather
/// than widening the projection the player and `queue.json` share.
pub async fn get_track_links_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<track::TrackLinks>, AppError> {
    let cols = track::track_links_columns();
    let links: Vec<track::TrackLinks> = chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT {cols} FROM tracks WHERE id IN ({placeholders})")
    })
    .await?;

    let mut map: HashMap<i64, track::TrackLinks> = HashMap::with_capacity(links.len());
    map.extend(links.into_iter().map(|l| (l.id, l)));

    Ok(ids.iter().filter_map(|id| map.remove(id)).collect())
}

/// Look up track IDs by file paths. Returns a map from `file_path` → track ID.
pub async fn get_track_ids_by_paths(
    db: &DbPool,
    paths: &[String],
) -> Result<HashMap<String, i64>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(db.read(), paths, |placeholders| {
        format!("SELECT id, file_path FROM tracks WHERE file_path IN ({placeholders})")
    })
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, path)| (path, id)));
    Ok(map)
}

/// Look up track IDs by BLAKE3 `file_hash`, through the partial index `idx_tracks_file_hash`.
/// Hashes with no match are absent from the map. Where two tracks share a hash — true content
/// duplicates — one arbitrary id wins, acceptable for playlist re-matching since either copy
/// plays identical audio.
pub async fn get_track_ids_by_hashes(
    db: &DbPool,
    hashes: &[String],
) -> Result<HashMap<String, i64>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(db.read(), hashes, |placeholders| {
        format!(
            "SELECT id, file_hash FROM tracks \
                 WHERE file_hash IS NOT NULL AND file_hash IN ({placeholders})"
        )
    })
    .await?;

    let mut map = HashMap::with_capacity(rows.len());
    map.extend(rows.into_iter().map(|(id, hash)| (hash, id)));
    Ok(map)
}

/// Get all file paths of tracks that have no `file_hash` (for retroactive hashing).
pub async fn get_unhashed_track_paths(db: &DbPool) -> Result<Vec<(i64, String)>, AppError> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, file_path FROM tracks WHERE file_hash IS NULL")
            .fetch_all(db.read())
            .await?;
    Ok(rows)
}

/// One page of unrated track paths, by ascending id and starting past `after_id`. The work-list
/// for the one-shot rating import.
///
/// `rating = 0` is the only marker there is: the column is `NOT NULL DEFAULT 0`, so an untouched
/// row and a deliberately cleared one look identical. That is why the sweep this feeds runs once
/// and records the fact in `settings.json` rather than re-deriving it from the table.
///
/// Paged by keyset rather than `OFFSET`, and rated in whole pages rather than in one pass, because
/// on the first run after ratings shipped the predicate selects the entire library: a `String` per
/// track, resident at once. Keyset is also the only form that stays correct here, since the import
/// writes ratings back as it goes, so rows leave `rating = 0` between pages and an offset would
/// step over their neighbours.
pub async fn get_unrated_track_paths_after(
    db: &DbPool,
    after_id: i64,
    limit: i64,
) -> Result<Vec<(i64, String)>, AppError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, file_path FROM tracks WHERE rating = 0 AND id > ? ORDER BY id LIMIT ?",
    )
    .bind(after_id)
    .bind(limit)
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
