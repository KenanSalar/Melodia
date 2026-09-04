use std::collections::HashMap;

use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use melodia_core::entities::{playlist, playlist_item, track};
use melodia_core::error::AppError;

pub async fn create_playlist(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    let result = sqlx::query_as::<_, playlist::Playlist>(
        "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at)
         VALUES (?, ?, NULL, FALSE, NULL, ?, ?)
         RETURNING *"
    )
    .bind(name)
    .bind(description)
    .bind(&now)
    .bind(&now)
    .fetch_one(db.write())
    .await?;
    Ok(result)
}

/// Create a smart (dynamic) playlist. Identical to [`create_playlist`] except
/// `is_smart = TRUE` and `smart_criteria` holds the serialized
/// [`melodia_core::entities::smart_criteria::SmartCriteria`] JSON. Smart playlists
/// never get `playlist_items` rows — their membership is resolved live from the
/// criteria (see [`crate::database::queries::smart_playlist`]).
pub async fn create_smart_playlist(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
    criteria_json: &str,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    let result = sqlx::query_as::<_, playlist::Playlist>(
        "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at)
         VALUES (?, ?, NULL, TRUE, ?, ?, ?)
         RETURNING *"
    )
    .bind(name)
    .bind(description)
    .bind(criteria_json)
    .bind(&now)
    .bind(&now)
    .fetch_one(db.write())
    .await?;
    Ok(result)
}

/// Replace a smart playlist's rule set. Only `smart_criteria` (+ `updated_at`)
/// change; name/description go through [`update_playlist`] like a regular
/// playlist's.
pub async fn update_smart_criteria(
    db: &DbPool,
    id: i64,
    criteria_json: &str,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    sqlx::query_as::<_, playlist::Playlist>(
        "UPDATE playlists SET smart_criteria = ?, updated_at = ? WHERE id = ? RETURNING *",
    )
    .bind(criteria_json)
    .bind(&now)
    .bind(id)
    .fetch_optional(db.write())
    .await?
    .ok_or_else(|| AppError::not_found("Playlist", id))
}

pub async fn get_all_playlists(db: &DbPool) -> Result<Vec<playlist::PlaylistStats>, AppError> {
    let playlists = sqlx::query_as::<_, playlist::PlaylistStats>(
        "SELECT * FROM playlist_stats ORDER BY updated_at DESC",
    )
    .fetch_all(db.read())
    .await?;
    Ok(playlists)
}

pub async fn get_playlist_by_id(db: &DbPool, id: i64) -> Result<playlist::PlaylistStats, AppError> {
    sqlx::query_as::<_, playlist::PlaylistStats>("SELECT * FROM playlist_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))
}

pub async fn update_playlist(
    db: &DbPool,
    id: i64,
    name: &str,
    description: Option<&str>,
    clear_thumbnail: bool,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    if clear_thumbnail {
        // Clear thumbnail entirely so the placeholder icon is shown.
        // custom_thumbnail = TRUE marks this as an explicit user choice so that
        // subsequent add/remove track operations do not auto-repopulate the
        // thumbnail from the first track's artwork in
        // update_playlist_thumbnail_and_timestamp_tx().
        let result = sqlx::query_as::<_, playlist::Playlist>(
            "UPDATE playlists SET name = ?, description = ?,
                thumbnail_path = NULL,
                custom_thumbnail = TRUE, updated_at = ?
            WHERE id = ? RETURNING *",
        )
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(id)
        .fetch_optional(db.write())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))?;
        Ok(result)
    } else {
        let result = sqlx::query_as::<_, playlist::Playlist>(
            "UPDATE playlists SET name = ?, description = ?, updated_at = ? WHERE id = ? RETURNING *"
        )
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(id)
        .fetch_optional(db.write())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))?;
        Ok(result)
    }
}

pub async fn delete_playlist(db: &DbPool, id: i64) -> Result<(), AppError> {
    // Cascade will handle playlist_items
    sqlx::query("DELETE FROM playlists WHERE id = ?").bind(id).execute(db.write()).await?;
    Ok(())
}

#[cfg(test)]
pub async fn get_playlist_tracks(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    let tracks = sqlx::query_as::<_, track::Track>(
        "SELECT t.* FROM tracks t
         JOIN playlist_items pi ON pi.track_id = t.id
         WHERE pi.playlist_id = ?
         ORDER BY pi.position ASC",
    )
    .bind(playlist_id)
    .fetch_all(db.read())
    .await?;
    Ok(tracks)
}

/// Lightweight version of `get_playlist_tracks` for list views.
pub async fn get_playlist_tracks_for_list(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns_prefixed("t");
    let sql = format!(
        "SELECT {cols} FROM tracks t \
         JOIN playlist_items pi ON pi.track_id = t.id \
         WHERE pi.playlist_id = ? \
         ORDER BY pi.position ASC"
    );
    let tracks = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(playlist_id)
        .fetch_all(db.read())
        .await?;
    Ok(tracks)
}

/// Slim projection for M3U export — `file_path`, `file_hash`, title, artist,
/// `duration_ms` in playlist order. Far cheaper than `get_playlist_tracks`
/// (full `Track`) which the export path doesn't need.
pub async fn get_playlist_tracks_for_export(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::PlaylistExportRow>, AppError> {
    let cols = track::playlist_export_columns_prefixed("t");
    let sql = format!(
        "SELECT {cols} FROM tracks t \
         JOIN playlist_items pi ON pi.track_id = t.id \
         WHERE pi.playlist_id = ? \
         ORDER BY pi.position ASC"
    );
    let tracks = sqlx::query_as::<_, track::PlaylistExportRow>(AssertSqlSafe(sql))
        .bind(playlist_id)
        .fetch_all(db.read())
        .await?;
    Ok(tracks)
}

pub async fn add_tracks_to_playlist(
    db: &DbPool,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), AppError> {
    // Batch INSERT — 4 columns per row, chunked within SQLite's bind limit
    const COLS_PER_ROW: usize = 4;
    const CHUNK_SIZE: usize = crate::database::SQLITE_BIND_LIMIT / COLS_PER_ROW;

    if track_ids.is_empty() {
        return Ok(());
    }

    let mut tx = db.write().begin().await?;

    // Get current max position within the transaction
    let max_pos: Option<i32> = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(position) FROM playlist_items WHERE playlist_id = ?",
    )
    .bind(playlist_id)
    .fetch_one(&mut *tx)
    .await?;

    let now = melodia_core::utils::now_rfc3339();
    let start_position = max_pos.unwrap_or(-1) + 1;

    for (chunk_idx, chunk) in track_ids.chunks(CHUNK_SIZE).enumerate() {
        let chunk_offset = start_position
            .checked_add(i32::try_from(chunk_idx * CHUNK_SIZE).map_err(|_| {
                AppError::Validation("playlist chunk offset overflows i32".to_owned())
            })?)
            .ok_or_else(|| AppError::Validation("playlist position overflows i32".to_owned()))?;
        let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, track_id, position, added_at) ",
        );
        query_builder.push_values(chunk.iter().enumerate(), |mut b, (i, track_id)| {
            let position = chunk_offset.saturating_add(i32::try_from(i).unwrap_or(i32::MAX));
            b.push_bind(playlist_id).push_bind(*track_id).push_bind(position).push_bind(&now);
        });
        query_builder.build().persistent(false).execute(&mut *tx).await?;
    }

    // Renumber positions to close gaps from ignored duplicates
    renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id).await?;

    tx.commit().await?;
    Ok(())
}

pub async fn remove_tracks_from_playlist_batch(
    db: &DbPool,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), AppError> {
    if track_ids.is_empty() {
        return Ok(());
    }

    let mut tx = db.write().begin().await?;

    // Chunk to stay within the SQLite bind limit — one slot is the
    // `playlist_id`, the rest are the `track_id` IN-list.
    for chunk in track_ids.chunks(crate::database::SQLITE_BIND_LIMIT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "DELETE FROM playlist_items WHERE playlist_id = ? AND track_id IN ({placeholders})"
        );
        let mut query = sqlx::query(AssertSqlSafe(sql)).bind(playlist_id);
        for &id in chunk {
            query = query.bind(id);
        }
        query.execute(&mut *tx).await?;
    }

    renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn reorder_playlist_track(
    db: &DbPool,
    playlist_id: i64,
    from: i32,
    to: i32,
) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;

    let items = sqlx::query_as::<_, playlist_item::PlaylistItem>(
        "SELECT * FROM playlist_items WHERE playlist_id = ? ORDER BY position ASC",
    )
    .bind(playlist_id)
    .fetch_all(&mut *tx)
    .await?;

    let from_idx = usize::try_from(from)
        .map_err(|_| AppError::Validation("negative `from` index".to_owned()))?;
    let to_idx =
        usize::try_from(to).map_err(|_| AppError::Validation("negative `to` index".to_owned()))?;

    if from_idx >= items.len() || to_idx >= items.len() {
        return Err(AppError::NotFound("Invalid position index".to_owned()));
    }

    let mut ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let moved = ids.remove(from_idx);
    ids.insert(to_idx, moved);

    // Batch UPDATE with CASE expression — single query instead of N
    batch_update_positions(&mut tx, &ids).await?;

    tx.commit().await?;
    Ok(())
}

/// Renumber positions within an existing transaction (used by `add_tracks_to_playlist`).
/// Uses `ROW_NUMBER()` window function for O(N log N) instead of correlated subquery O(N²).
async fn renumber_playlist_positions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "WITH numbered AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY position, id) - 1 AS new_pos
            FROM playlist_items WHERE playlist_id = ?1
        )
        UPDATE playlist_items SET position = (
            SELECT new_pos FROM numbered WHERE numbered.id = playlist_items.id
        ) WHERE playlist_id = ?1",
    )
    .bind(playlist_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Batch UPDATE positions using a CASE expression — single query instead of N.
/// `ids` contains item IDs in their desired position order (index = new position).
async fn batch_update_positions(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }

    let pairs: Vec<(i64, i32)> = ids
        .iter()
        .enumerate()
        .map(|(pos, &id)| (id, i32::try_from(pos).unwrap_or(i32::MAX)))
        .collect();
    batch_update_positions_pairs(tx, &pairs).await
}

/// Batch UPDATE positions from (id, position) pairs using a CASE expression.
async fn batch_update_positions_pairs(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    pairs: &[(i64, i32)],
) -> Result<(), AppError> {
    if pairs.is_empty() {
        return Ok(());
    }

    let mut sql = String::from("UPDATE playlist_items SET position = CASE id ");
    for _ in pairs {
        sql.push_str("WHEN ? THEN ? ");
    }
    sql.push_str("END WHERE id IN (");
    sql.push_str(&crate::database::placeholders(pairs.len()));
    sql.push(')');

    let mut query = sqlx::query(AssertSqlSafe(sql));
    for &(id, pos) in pairs {
        query = query.bind(id).bind(pos);
    }
    for &(id, _) in pairs {
        query = query.bind(id);
    }
    query.persistent(false).execute(&mut **tx).await?;
    Ok(())
}

/// For each playlist that contains at least one of `track_ids`, returns
/// the number of distinct selection ids already in it. Backs the Add-to-
/// Playlist picker dialog so full-overlap rows can be greyed out and
/// partial-overlap rows can show a `2 of 4 already added` badge.
///
/// Chunked through `chunked_in_query` to stay inside `SQLITE_BIND_LIMIT`;
/// counts are summed across chunks because a playlist may appear in more
/// than one chunk's result set.
pub async fn count_tracks_in_playlists_for_selection(
    db: &DbPool,
    track_ids: &[i64],
) -> Result<HashMap<i64, i64>, AppError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<(i64, i64)> =
        crate::database::chunked_in_query(db.read(), track_ids, |placeholders| {
            format!(
                "SELECT playlist_id, COUNT(*) FROM playlist_items \
                 WHERE track_id IN ({placeholders}) \
                 GROUP BY playlist_id"
            )
        })
        .await?;

    let mut map: HashMap<i64, i64> = HashMap::with_capacity(rows.len());
    for (pl_id, count) in rows {
        *map.entry(pl_id).or_insert(0) += count;
    }
    Ok(map)
}

/// Returns up to `limit` distinct artwork paths from tracks in the given
/// playlist, in playlist order.
pub async fn get_playlist_artwork_paths(
    db: &DbPool,
    playlist_id: i64,
    limit: i64,
) -> Result<Vec<String>, AppError> {
    let paths: Vec<(String,)> = sqlx::query_as(
        "SELECT t.artwork_path FROM tracks t
         JOIN playlist_items pi ON pi.track_id = t.id
         WHERE pi.playlist_id = ? AND t.artwork_path IS NOT NULL AND t.artwork_path != ''
         GROUP BY t.artwork_path
         ORDER BY MIN(pi.position) ASC
         LIMIT ?",
    )
    .bind(playlist_id)
    .bind(limit)
    .fetch_all(db.read())
    .await?;
    Ok(paths.into_iter().map(|(p,)| p).collect())
}

/// Sets a custom thumbnail path on the playlist.
pub async fn set_playlist_custom_thumbnail(
    db: &DbPool,
    playlist_id: i64,
    path: &str,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    sqlx::query_as::<_, playlist::Playlist>(
        "UPDATE playlists SET thumbnail_path = ?, custom_thumbnail = TRUE, updated_at = ?
         WHERE id = ? RETURNING *",
    )
    .bind(path)
    .bind(&now)
    .bind(playlist_id)
    .fetch_optional(db.write())
    .await?
    .ok_or_else(|| AppError::not_found("Playlist", playlist_id))
}

/// Updates thumbnail and timestamp within an existing transaction.
async fn update_playlist_thumbnail_and_timestamp_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
) -> Result<(), AppError> {
    let now = melodia_core::utils::now_rfc3339();

    sqlx::query(
        "UPDATE playlists SET
            thumbnail_path = (
                SELECT t.artwork_path FROM playlist_items pi
                JOIN tracks t ON pi.track_id = t.id
                WHERE pi.playlist_id = playlists.id AND t.artwork_path IS NOT NULL AND t.artwork_path != ''
                ORDER BY pi.position ASC LIMIT 1
            ),
            updated_at = ?
        WHERE id = ? AND custom_thumbnail = FALSE"
    )
    .bind(&now)
    .bind(playlist_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/playlist_tests.rs"]
mod tests;
