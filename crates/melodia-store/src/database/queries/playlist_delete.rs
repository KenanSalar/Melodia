//! Deleting playlists, either for good or holding what an undo puts back. Each delete is one
//! transaction, a playlist's items cascading with it.

use std::collections::HashMap;

use rusqlite::Transaction;

use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::playlist::{CutItem, Playlist, PlaylistSnapshot};
use melodia_core::error::AppError;

/// Deletes playlists for good, answering the names of those that went. Chunked, since the card
/// menu hands it a whole selection.
pub async fn delete_playlists(db: &DbPool, ids: Vec<i64>) -> Result<Vec<String>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db.transaction(move |tx| delete_tx(tx, &ids)).await
}

/// Deletes playlists, answering what puts each back, or nothing where none of `ids` was there.
/// It holds every item it takes, so its caller checks [`item_count`] against `UNDO_HOLD_CAP`
/// first.
pub async fn delete_playlists_held(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<PlaylistSnapshot>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db.transaction(move |tx| {
        let snapshots = snapshots_tx(tx, &ids)?;
        if !snapshots.is_empty() {
            delete_tx(tx, &ids)?;
        }
        Ok(snapshots)
    })
    .await
}

/// How many items deleting `ids` would take, which is what the undo cap is checked on.
pub async fn item_count(db: &DbPool, ids: Vec<i64>) -> Result<usize, AppError> {
    let per_chunk: Vec<i64> = db
        .read(move |conn| {
            chunked_in_query(conn, &ids, |placeholders| {
                format!("SELECT COUNT(*) FROM playlist_items WHERE playlist_id IN ({placeholders})")
            })
        })
        .await?;
    let total: i64 = per_chunk.iter().sum();
    Ok(usize::try_from(total).unwrap_or(usize::MAX))
}

/// The covers picked for `ids` rather than taken from a track, the files only those rows name.
pub async fn custom_covers(db: &DbPool, ids: &[i64]) -> Result<Vec<String>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |placeholders| {
            format!(
                "SELECT thumbnail_path FROM playlists
                 WHERE custom_thumbnail = TRUE AND thumbnail_path IS NOT NULL
                   AND id IN ({placeholders})"
            )
        })
    })
    .await
}

/// Every row of `ids` still there, with its items in the playlist's order.
fn snapshots_tx(tx: &Transaction<'_>, ids: &[i64]) -> Result<Vec<PlaylistSnapshot>, AppError> {
    let playlists: Vec<Playlist> = chunked_in_query(tx, ids, |placeholders| {
        format!(
            "SELECT id, name, description, thumbnail_path, is_smart, smart_criteria, created_at,
                    updated_at, custom_thumbnail
             FROM playlists WHERE id IN ({placeholders})"
        )
    })?;

    // Chunked by playlist, so each one's items come from one ordered statement.
    let rows: Vec<(i64, i64, String)> = chunked_in_query(tx, ids, |placeholders| {
        format!(
            "SELECT playlist_id, track_id, added_at FROM playlist_items
             WHERE playlist_id IN ({placeholders}) ORDER BY playlist_id, position, id"
        )
    })?;
    let mut items: HashMap<i64, Vec<CutItem>> = HashMap::new();
    for (playlist_id, track_id, added_at) in rows {
        let held = items.entry(playlist_id).or_default();
        held.push(CutItem { track_id, row: held.len(), added_at });
    }

    Ok(playlists
        .into_iter()
        .map(|playlist| {
            let items = items.remove(&playlist.id).unwrap_or_default();
            PlaylistSnapshot { playlist, items }
        })
        .collect())
}

/// Deletes `ids` and answers the names that went.
fn delete_tx(tx: &Transaction<'_>, ids: &[i64]) -> Result<Vec<String>, AppError> {
    chunked_in_query(tx, ids, |placeholders| {
        format!("DELETE FROM playlists WHERE id IN ({placeholders}) RETURNING name")
    })
}
