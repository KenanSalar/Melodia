use std::collections::{HashMap, HashSet};

use sqlx::AssertSqlSafe;

use crate::database::queries::search_name;
use crate::database::queries::track::columns;
use crate::database::{DbPool, select_in_id_order};
use melodia_core::entities::playlist::{CutItem, PlaylistItemsCut};
use melodia_core::entities::undo::{UNDO_HOLD_CAP, UndoCapture};
use melodia_core::entities::{playlist, track};
use melodia_core::error::AppError;
use melodia_core::utils::block_move;

pub async fn create_playlist(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
) -> Result<playlist::Playlist, AppError> {
    create_playlist_on(db.write(), name, description).await
}

/// [`create_playlist`] against any executor, so [`create_playlist_with_tracks`] can run it inside
/// its transaction without a second copy of the statement.
async fn create_playlist_on<'e, E>(
    executor: E,
    name: &str,
    description: Option<&str>,
) -> Result<playlist::Playlist, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let now = melodia_core::utils::now_rfc3339();

    let result = sqlx::query_as::<_, playlist::Playlist>(
        "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at, search_name)
         VALUES (?, ?, NULL, FALSE, NULL, ?, ?, ?)
         RETURNING *"
    )
    .bind(name)
    .bind(description)
    .bind(&now)
    .bind(&now)
    .bind(search_name::pack(&[name]))
    .fetch_one(executor)
    .await?;
    Ok(result)
}

/// Creates a playlist already holding `track_ids` in order, and returns its id.
///
/// One transaction, so an insert that fails leaves no empty playlist behind.
pub async fn create_playlist_with_tracks(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
    track_ids: &[i64],
) -> Result<i64, AppError> {
    let mut tx = db.write().begin().await?;
    let playlist_id = create_playlist_on(&mut *tx, name, description).await?.id;

    let now = melodia_core::utils::now_rfc3339();
    let inserted = insert_items_tx(&mut tx, playlist_id, &contiguous(track_ids, 0, &now)?).await?;
    // Only a repeated track leaves a gap: the unique index keeps its first slot and drops the rest.
    if inserted < u64::try_from(track_ids.len()).unwrap_or(u64::MAX) {
        renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
    }
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &now).await?;

    tx.commit().await?;
    Ok(playlist_id)
}

/// Creates a smart playlist, whose membership [`crate::database::queries::smart_playlist`] resolves
/// live from `criteria_json` rather than from `playlist_items`.
pub async fn create_smart_playlist(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
    criteria_json: &str,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();

    let result = sqlx::query_as::<_, playlist::Playlist>(
        "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at, search_name)
         VALUES (?, ?, NULL, TRUE, ?, ?, ?, ?)
         RETURNING *"
    )
    .bind(name)
    .bind(description)
    .bind(criteria_json)
    .bind(&now)
    .bind(&now)
    .bind(search_name::pack(&[name]))
    .fetch_one(db.write())
    .await?;
    Ok(result)
}

/// Replaces a smart playlist's rule set; its name and description go through [`update_playlist`].
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

/// The Playlists grid's cards for `ids`, in the order asked for. A smart playlist's count is the
/// stored `0`, which the caller replaces with its live one.
pub async fn get_playlists_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<playlist::PlaylistStats>, AppError> {
    select_in_id_order(db, "SELECT * FROM playlist_stats", ids, |p: &playlist::PlaylistStats| p.id)
        .await
}

pub async fn get_playlist_by_id(db: &DbPool, id: i64) -> Result<playlist::PlaylistStats, AppError> {
    sqlx::query_as::<_, playlist::PlaylistStats>("SELECT * FROM playlist_stats WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))
}

/// `(id, is_smart, smart_criteria)` for each of `ids` still there, off the base table rather than
/// `playlist_stats`: the card menu asks it of a whole selection only to split the smart ones off.
pub async fn smart_criteria_for_playlists(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, bool, Option<String>)>, AppError> {
    crate::database::chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, is_smart, smart_criteria FROM playlists WHERE id IN ({placeholders})")
    })
    .await
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
        // custom_thumbnail = TRUE records the cleared cover as the user's choice, so a later add,
        // removal or reorder leaves the placeholder rather than filling it from the first track.
        let result = sqlx::query_as::<_, playlist::Playlist>(
            "UPDATE playlists SET name = ?, description = ?,
                thumbnail_path = NULL,
                custom_thumbnail = TRUE, updated_at = ?, search_name = ?
            WHERE id = ? RETURNING *",
        )
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(search_name::pack(&[name]))
        .bind(id)
        .fetch_optional(db.write())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))?;
        Ok(result)
    } else {
        let result = sqlx::query_as::<_, playlist::Playlist>(
            "UPDATE playlists SET name = ?, description = ?, updated_at = ?, search_name = ? \
             WHERE id = ? RETURNING *",
        )
        .bind(name)
        .bind(description)
        .bind(&now)
        .bind(search_name::pack(&[name]))
        .bind(id)
        .fetch_optional(db.write())
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", id))?;
        Ok(result)
    }
}

/// Deletes playlists, their items cascading. Chunked rather than a statement per id, since the
/// card menu hands it a whole selection.
pub async fn delete_playlists(db: &DbPool, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("DELETE FROM playlists WHERE id IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(db.write()).await?;
    }
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

/// A playlist's M3U export rows, in playlist order.
pub async fn get_playlist_tracks_for_export(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::PlaylistExportRow>, AppError> {
    let cols = columns::PLAYLIST_EXPORT.prefixed("t");
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
    if track_ids.is_empty() {
        return Ok(());
    }

    let mut tx = db.write().begin().await?;

    let max_pos: Option<i32> = sqlx::query_scalar::<_, Option<i32>>(
        "SELECT MAX(position) FROM playlist_items WHERE playlist_id = ?",
    )
    .bind(playlist_id)
    .fetch_one(&mut *tx)
    .await?;

    let start_position = max_pos.unwrap_or(-1) + 1;
    let now = melodia_core::utils::now_rfc3339();
    insert_items_tx(&mut tx, playlist_id, &contiguous(track_ids, start_position, &now)?).await?;

    // Closes the gaps ignored duplicates leave.
    renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &now).await?;

    tx.commit().await?;
    Ok(())
}

/// A `playlist_items` row to insert.
pub(crate) struct NewItem<'a> {
    pub(crate) track_id: i64,
    pub(crate) position: i32,
    pub(crate) added_at: &'a str,
}

/// `track_ids` as one run of rows from `start_position` on, all added at `added_at`.
fn contiguous<'a>(
    track_ids: &[i64],
    start_position: i32,
    added_at: &'a str,
) -> Result<Vec<NewItem<'a>>, AppError> {
    track_ids
        .iter()
        .enumerate()
        .map(|(offset, &track_id)| {
            let position = i32::try_from(offset)
                .ok()
                .and_then(|offset| start_position.checked_add(offset))
                .ok_or_else(|| AppError::validation_msg("playlist position overflows i32"))?;
            Ok(NewItem { track_id, position, added_at })
        })
        .collect()
}

/// Inserts `items`, skipping any track the playlist already holds, and returns how many rows
/// landed.
pub(crate) async fn insert_items_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
    items: &[NewItem<'_>],
) -> Result<u64, AppError> {
    const COLS_PER_ROW: usize = 4;
    const CHUNK_SIZE: usize = crate::database::MAX_BINDS_PER_STATEMENT / COLS_PER_ROW;

    let mut inserted: u64 = 0;
    for chunk in items.chunks(CHUNK_SIZE) {
        let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, track_id, position, added_at) ",
        );
        query_builder.push_values(chunk, |mut b, item| {
            b.push_bind(playlist_id)
                .push_bind(item.track_id)
                .push_bind(item.position)
                .push_bind(item.added_at);
        });
        let result = query_builder.build().persistent(false).execute(&mut **tx).await?;
        inserted += result.rows_affected();
    }
    Ok(inserted)
}

/// Removes `track_ids` from a playlist, answering what puts them back, or only how many went where
/// there were more than [`UNDO_HOLD_CAP`]: those are removed as they always were, nothing kept.
pub async fn remove_tracks_from_playlist_batch(
    db: &DbPool,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<UndoCapture<PlaylistItemsCut>, AppError> {
    if track_ids.is_empty() {
        return Ok(UndoCapture::Nothing);
    }

    let mut tx = db.write().begin().await?;

    if track_ids.len() > UNDO_HOLD_CAP {
        let removed = delete_items_tx(&mut tx, playlist_id, track_ids).await?;
        if removed == 0 {
            return Ok(UndoCapture::Nothing);
        }
        renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
        let now = melodia_core::utils::now_rfc3339();
        update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &now).await?;
        tx.commit().await?;
        return Ok(UndoCapture::OverCap { rows: removed });
    }

    let rows = cut_rows_tx(&mut tx, playlist_id, track_ids).await?;
    if rows.is_empty() {
        return Ok(UndoCapture::Nothing);
    }
    let updated_before: String =
        sqlx::query_scalar("SELECT updated_at FROM playlists WHERE id = ?")
            .bind(playlist_id)
            .fetch_one(&mut *tx)
            .await?;

    delete_items_tx(&mut tx, playlist_id, track_ids).await?;
    renumber_playlist_positions_tx(&mut tx, playlist_id).await?;
    let stamped_at = melodia_core::utils::now_rfc3339();
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &stamped_at).await?;
    tx.commit().await?;

    Ok(UndoCapture::Held(PlaylistItemsCut { playlist_id, rows, updated_before, stamped_at }))
}

/// The rows `track_ids` hold in the playlist, ascending, each with its index in the playlist's
/// order: the positions themselves can carry gaps under a hard-deleted track.
async fn cut_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<Vec<CutItem>, AppError> {
    let mut rows = Vec::with_capacity(track_ids.len());
    // One bind of each statement is the `playlist_id`.
    for chunk in track_ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "SELECT track_id, row, added_at FROM (
                SELECT track_id, added_at, ROW_NUMBER() OVER (ORDER BY position, id) - 1 AS row
                FROM playlist_items WHERE playlist_id = ?
            ) WHERE track_id IN ({placeholders})"
        );
        let mut query = sqlx::query_as::<_, (i64, i64, String)>(AssertSqlSafe(sql))
            .persistent(false)
            .bind(playlist_id);
        for &id in chunk {
            query = query.bind(id);
        }
        for (track_id, row, added_at) in query.fetch_all(&mut **tx).await? {
            let row = usize::try_from(row).unwrap_or(usize::MAX);
            rows.push(CutItem { track_id, row, added_at });
        }
    }
    rows.sort_unstable_by_key(|item| item.row);
    Ok(rows)
}

/// Deletes `track_ids`' rows from a playlist and returns how many went.
async fn delete_items_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<usize, AppError> {
    let mut removed: u64 = 0;
    // One bind of each statement is the `playlist_id`.
    for chunk in track_ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "DELETE FROM playlist_items WHERE playlist_id = ? AND track_id IN ({placeholders})"
        );
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(playlist_id);
        for &id in chunk {
            query = query.bind(id);
        }
        removed += query.execute(&mut **tx).await?.rows_affected();
    }
    Ok(usize::try_from(removed).unwrap_or(usize::MAX))
}

/// Moves `track_ids` together to `to`, counted once they have left, in the order the playlist
/// holds them. A track sits in a playlist once (`idx_playlist_items_unique`), so the ids say which
/// rows move even where the list a drag started on has fallen behind the database.
///
/// An automatic cover is the first track's with one, so a move can hand it to another track, and
/// it is refreshed here as an add or a removal refreshes it.
pub async fn move_playlist_tracks(
    db: &DbPool,
    playlist_id: i64,
    track_ids: &[i64],
    to: usize,
) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;

    let mut items: Vec<(i64, i64, i32)> = sqlx::query_as(
        "SELECT id, track_id, position FROM playlist_items WHERE playlist_id = ?
         ORDER BY position, id",
    )
    .bind(playlist_id)
    .fetch_all(&mut *tx)
    .await?;

    let moving: HashSet<i64> = track_ids.iter().copied().collect();
    let rows: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, (_, track_id, _))| moving.contains(track_id))
        .map(|(row, _)| row)
        .collect();
    if rows.len() != moving.len() {
        return Err(AppError::not_found_msg("Track not in the playlist"));
    }
    if to + rows.len() > items.len() {
        return Err(AppError::not_found_msg("Invalid position index"));
    }
    if !block_move::move_block(&mut items, &rows, to) {
        return Ok(());
    }

    // Only the rows whose index moved, so a drag writes the span it crossed rather than the whole
    // playlist. A gap left under a hard-deleted track makes every row after it differ, and the
    // rewrite closes it.
    let changed: Vec<(i64, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, &(id, _, position))| {
            let index = i32::try_from(index).unwrap_or(i32::MAX);
            (position != index).then_some((id, index))
        })
        .collect();
    batch_update_positions_pairs(&mut tx, &changed).await?;
    let now = melodia_core::utils::now_rfc3339();
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &now).await?;

    tx.commit().await?;
    Ok(())
}

/// Closes the gaps in a playlist's positions.
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

/// Writes `(id, position)` pairs in chunks, which is safe because the positions are absolute and
/// each chunk touches its own ids.
pub(crate) async fn batch_update_positions_pairs(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    pairs: &[(i64, i32)],
) -> Result<(), AppError> {
    const CHUNK_SIZE: usize =
        crate::database::MAX_BINDS_PER_STATEMENT / crate::database::CASE_BINDS_PER_ROW;

    if pairs.is_empty() {
        return Ok(());
    }

    for chunk in pairs.chunks(CHUNK_SIZE) {
        let sql = format!(
            "UPDATE playlist_items SET position = {} WHERE id IN ({})",
            crate::database::case_by_id(chunk.len()),
            crate::database::placeholders(chunk.len()),
        );

        let mut query = sqlx::query(AssertSqlSafe(sql));
        for &(id, pos) in chunk {
            query = query.bind(id).bind(pos);
        }
        for &(id, _) in chunk {
            query = query.bind(id);
        }
        query.persistent(false).execute(&mut **tx).await?;
    }
    Ok(())
}

/// How many of `track_ids` each playlist already holds, for the Add-to-Playlist picker's greyed
/// rows and "2 of 4 already added" badges. Summed across chunks, a playlist turning up in several.
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

/// An automatic playlist's thumbnail: the first track in it that has a cover.
macro_rules! first_track_cover {
    () => {
        "(SELECT t.artwork_path FROM playlist_items pi
          JOIN tracks t ON pi.track_id = t.id
          WHERE pi.playlist_id = playlists.id AND t.artwork_path IS NOT NULL AND t.artwork_path != ''
          ORDER BY pi.position ASC LIMIT 1)"
    };
}

/// Stamps an add, removal or reorder with `stamp`, and points an automatic cover at the first track
/// with one. A cover the user picked or cleared stays, but the edit still lifts the playlist in the
/// grid. The stamp is the caller's, so a removal can record the one it wrote and an undo can put
/// the older one back.
pub(crate) async fn update_playlist_thumbnail_and_timestamp_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    playlist_id: i64,
    stamp: &str,
) -> Result<(), AppError> {
    sqlx::query(concat!(
        "UPDATE playlists SET thumbnail_path = CASE WHEN custom_thumbnail = FALSE THEN ",
        first_track_cover!(),
        " ELSE thumbnail_path END, updated_at = ? WHERE id = ?"
    ))
    .bind(stamp)
    .bind(playlist_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Points every automatic playlist's thumbnail back at its first track's cover.
///
/// For what reaches a playlist without an edit to it: a track deleted out from under it, or a
/// cover cleared because its file went missing. `updated_at` stays put: the user changed nothing.
pub(crate) async fn refresh_automatic_thumbnails(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(concat!(
        "UPDATE playlists SET thumbnail_path = ",
        first_track_cover!(),
        " WHERE custom_thumbnail = FALSE AND thumbnail_path IS NOT ",
        first_track_cover!()
    ))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/playlist_tests.rs"]
mod tests;
