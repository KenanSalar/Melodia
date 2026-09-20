//! Whole-row `SELECT *` fetches, kept for unit-test fixtures only: assertions need a full `Track`,
//! where production reads the narrower projections beside this file.

use std::collections::HashMap;

use sqlx::AssertSqlSafe;

use super::list::TRACK_LIST_ORDER;
use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::track;
use melodia_core::error::AppError;

pub async fn get_all_tracks(db: &DbPool) -> Result<Vec<track::Track>, AppError> {
    let sql = format!("SELECT * FROM tracks ORDER BY {TRACK_LIST_ORDER}");
    let tracks = sqlx::query_as::<_, track::Track>(AssertSqlSafe(sql)).fetch_all(db.read()).await?;
    Ok(tracks)
}

pub async fn get_tracks_by_album(
    db: &DbPool,
    album_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(
        "SELECT * FROM tracks WHERE album_id = ? ORDER BY disc_number ASC, track_number ASC",
    )
    .bind(album_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

pub async fn get_tracks_by_artist(
    db: &DbPool,
    artist_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(AssertSqlSafe(format!(
        "SELECT * FROM tracks \
         WHERE id IN (SELECT track_id FROM track_artists WHERE artist_id = ?) \
         ORDER BY {TRACK_LIST_ORDER}"
    )))
    .bind(artist_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

pub async fn get_tracks_by_genre(
    db: &DbPool,
    genre_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(AssertSqlSafe(format!(
        "SELECT * FROM tracks \
         WHERE id IN (SELECT track_id FROM track_genres WHERE genre_id = ?) \
         ORDER BY {TRACK_LIST_ORDER}"
    )))
    .bind(genre_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

pub async fn get_track_by_id(db: &DbPool, id: i64) -> Result<track::Track, AppError> {
    sqlx::query_as::<_, track::Track>("SELECT * FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Track", id))
}

/// Fetch tracks by IDs, preserving the input order.
pub async fn get_tracks_by_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<track::Track>, AppError> {
    let tracks: Vec<track::Track> = chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT * FROM tracks WHERE id IN ({placeholders})")
    })
    .await?;

    // Re-order results to match the input ID order
    let mut track_map: HashMap<i64, track::Track> = HashMap::with_capacity(tracks.len());
    track_map.extend(tracks.into_iter().map(|t| (t.id, t)));

    Ok(ids.iter().filter_map(|id| track_map.remove(id)).collect())
}

/// Find groups of tracks that share the same `file_hash` (duplicates).
/// Returns a Vec of groups, where each group is a Vec of tracks with the same hash.
///
/// One query joins each track to the duplicate-hash CTE; rows arrive
/// pre-sorted by `(file_hash, date_added)` so we can group consecutive
/// equal hashes in Rust without a `HashMap`.
pub async fn get_duplicate_tracks(db: &DbPool) -> Result<Vec<Vec<track::Track>>, AppError> {
    let rows: Vec<track::Track> = sqlx::query_as(
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

    let mut groups: Vec<Vec<track::Track>> = Vec::new();
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
