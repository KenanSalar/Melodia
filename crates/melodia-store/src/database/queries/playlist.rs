use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rusqlite::{Connection, Transaction};

use crate::database::queries::search_name;
use crate::database::queries::track::{columns, missing_ids_on};
use crate::database::sql::{self, Sql};
use crate::database::{
    CASE_BINDS_PER_ROW, DbPool, MAX_BINDS_PER_STATEMENT, case_by_id, chunked_in_query,
    chunked_in_statements, placeholders, select_in_id_order, values_placeholders,
};
use melodia_core::entities::playlist::{CutItem, PlaylistIdentity, PlaylistItemsCut};
use melodia_core::entities::undo::{UNDO_HOLD_CAP, UndoCapture};
use melodia_core::entities::{playlist, track};
use melodia_core::error::AppError;
use melodia_core::utils::block_move;

pub async fn create_playlist(
    db: &DbPool,
    name: &str,
    description: Option<&str>,
) -> Result<playlist::Playlist, AppError> {
    let name = name.to_owned();
    let description = description.map(str::to_owned);
    db.write(move |conn| create_playlist_on(conn, &name, description.as_deref())).await
}

/// [`create_playlist`] on a connection the caller holds, so [`create_playlist_with_tracks`] can run
/// it inside its transaction without a second copy of the statement.
fn create_playlist_on(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    sql::fetch_one(
        conn,
        "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at, search_name)
         VALUES (?, ?, NULL, FALSE, NULL, ?, ?, ?)
         RETURNING *",
        (name, description, &now, &now, search_name::pack(&[name])),
    )
}

/// Creates a playlist already holding `track_ids` in order, less any the library lost since they
/// were read, and returns its id.
///
/// One transaction, so an insert that fails leaves no empty playlist behind.
pub async fn create_playlist_with_tracks(
    db: &DbPool,
    name: String,
    description: Option<String>,
    track_ids: Vec<i64>,
) -> Result<i64, AppError> {
    db.transaction(move |tx| {
        let present = present_in_order(tx, &track_ids)?;
        let playlist_id = create_playlist_on(tx, &name, description.as_deref())?.id;

        let now = melodia_core::utils::now_rfc3339();
        let landed = insert_items_tx(tx, playlist_id, &contiguous(&present, 0, &now)?)?;
        // Only a repeated track leaves a gap, the unique index keeping its first slot.
        if landed.len() < present.len() {
            renumber_playlist_positions_tx(tx, playlist_id)?;
        }
        update_playlist_thumbnail_and_timestamp_tx(tx, playlist_id, &now)?;
        Ok(playlist_id)
    })
    .await
}

/// Creates a smart playlist, whose membership [`crate::database::queries::smart_playlist`] resolves
/// live from `criteria_json` rather than from `playlist_items`.
pub async fn create_smart_playlist(
    db: &DbPool,
    name: String,
    description: Option<String>,
    criteria_json: String,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    let search_name = search_name::pack(&[&name]);
    db.write(move |conn| {
        sql::fetch_one(
            conn,
            "INSERT INTO playlists (name, description, thumbnail_path, is_smart, smart_criteria, created_at, updated_at, search_name)
             VALUES (?, ?, NULL, TRUE, ?, ?, ?, ?)
             RETURNING *",
            (name, description, criteria_json, &now, &now, search_name),
        )
    })
    .await
}

/// Replaces a smart playlist's rule set; its name and description go through [`update_playlist`].
pub async fn update_smart_criteria(
    db: &DbPool,
    id: i64,
    criteria_json: String,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    db.write(move |conn| {
        sql::fetch_optional(
            conn,
            "UPDATE playlists SET smart_criteria = ?, updated_at = ? WHERE id = ? RETURNING *",
            (criteria_json, now, id),
        )
    })
    .await?
    .ok_or_else(|| AppError::not_found("Playlist", id))
}

pub async fn get_all_playlists(db: &DbPool) -> Result<Vec<playlist::PlaylistStats>, AppError> {
    db.read(|conn| {
        sql::fetch_all(conn, "SELECT * FROM playlist_stats ORDER BY updated_at DESC", ())
    })
    .await
}

/// The Playlists grid's cards for `ids`, in the order asked for. A smart playlist's count is the
/// stored `0`, which the caller replaces with its live one.
pub async fn get_playlists_by_ids(
    db: &DbPool,
    ids: Vec<i64>,
) -> Result<Vec<playlist::PlaylistStats>, AppError> {
    db.read(move |conn| {
        select_in_id_order(
            conn,
            "SELECT * FROM playlist_stats",
            &ids,
            |p: &playlist::PlaylistStats| p.id,
        )
    })
    .await
}

pub async fn get_playlist_by_id(db: &DbPool, id: i64) -> Result<playlist::PlaylistStats, AppError> {
    db.read(move |conn| {
        sql::fetch_optional(conn, "SELECT * FROM playlist_stats WHERE id = ?", (id,))
    })
    .await?
    .ok_or_else(|| AppError::not_found("Playlist", id))
}

/// `(id, is_smart, smart_criteria)` for each of `ids` still there, off the base table rather than
/// `playlist_stats`: the card menu asks it of a whole selection only to split the smart ones off.
pub async fn smart_criteria_for_playlists(
    db: &DbPool,
    ids: &[i64],
) -> Result<Vec<(i64, bool, Option<String>)>, AppError> {
    let ids = ids.to_vec();
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |placeholders| {
            format!(
                "SELECT id, is_smart, smart_criteria FROM playlists WHERE id IN ({placeholders})"
            )
        })
    })
    .await
}

pub async fn update_playlist(
    db: &DbPool,
    id: i64,
    name: String,
    description: Option<String>,
    clear_thumbnail: bool,
) -> Result<playlist::Playlist, AppError> {
    let statement = if clear_thumbnail {
        // custom_thumbnail = TRUE records the cleared cover as the user's choice, so a later add,
        // removal or reorder leaves the placeholder rather than filling it from the first track.
        "UPDATE playlists SET name = ?, description = ?, thumbnail_path = NULL,
                custom_thumbnail = TRUE, updated_at = ?, search_name = ?
         WHERE id = ? RETURNING *"
    } else {
        "UPDATE playlists SET name = ?, description = ?, updated_at = ?, search_name = ?
         WHERE id = ? RETURNING *"
    };
    let now = melodia_core::utils::now_rfc3339();
    let search_name = search_name::pack(&[&name]);
    db.write(move |conn| {
        sql::fetch_optional(conn, statement, (name, description, now, search_name, id))
    })
    .await?
    .ok_or_else(|| AppError::not_found("Playlist", id))
}

#[cfg(test)]
pub async fn get_playlist_tracks(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::Track>, AppError> {
    db.read(move |conn| {
        sql::fetch_all(
            conn,
            "SELECT t.* FROM tracks t
             JOIN playlist_items pi ON pi.track_id = t.id
             WHERE pi.playlist_id = ?
             ORDER BY pi.position ASC",
            (playlist_id,),
        )
    })
    .await
}

static EXPORT_ROWS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} FROM tracks t \
         JOIN playlist_items pi ON pi.track_id = t.id \
         WHERE pi.playlist_id = ? \
         ORDER BY pi.position ASC",
        columns::PLAYLIST_EXPORT.prefixed("t")
    )
});

/// A playlist's M3U export rows, in playlist order.
pub async fn get_playlist_tracks_for_export(
    db: &DbPool,
    playlist_id: i64,
) -> Result<Vec<track::PlaylistExportRow>, AppError> {
    db.read(move |conn| sql::fetch_all(conn, EXPORT_ROWS.as_str(), (playlist_id,))).await
}

/// Appends `track_ids` to a playlist, skipping any it already holds or the library lost since they
/// were read, and returns the tracks that landed, in no set order.
pub async fn add_tracks_to_playlist(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<Vec<i64>, AppError> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    db.transaction(move |tx| append_items(tx, playlist_id, &track_ids)).await
}

fn append_items(
    tx: &Transaction<'_>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<Vec<i64>, AppError> {
    let present = present_in_order(tx, track_ids)?;

    let max_pos: Option<i32> = sql::fetch_one(
        tx,
        "SELECT MAX(position) FROM playlist_items WHERE playlist_id = ?",
        (playlist_id,),
    )?;

    let start_position = max_pos.unwrap_or(-1) + 1;
    let now = melodia_core::utils::now_rfc3339();
    let landed = insert_items_tx(tx, playlist_id, &contiguous(&present, start_position, &now)?)?;
    // A playlist that took nothing keeps its `updated_at`, so its card keeps its place.
    if landed.is_empty() {
        return Ok(landed);
    }

    // Closes the gaps ignored duplicates leave.
    renumber_playlist_positions_tx(tx, playlist_id)?;
    update_playlist_thumbnail_and_timestamp_tx(tx, playlist_id, &now)?;
    Ok(landed)
}

/// A `playlist_items` row to insert.
pub(crate) struct NewItem<'a> {
    pub(crate) track_id: i64,
    pub(crate) position: i32,
    pub(crate) added_at: &'a str,
}

/// `track_ids` as one run of rows from `start_position` on, all added at `added_at`.
pub(crate) fn contiguous<'a>(
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

/// Inserts `items`, skipping any track the playlist already holds, and returns the tracks that
/// landed, in no set order. Each item names a track present inside `tx`: a scan hard-deletes a
/// track whose file went, and an insert naming one fails its foreign key, which `OR IGNORE`
/// doesn't cover. Ids read before `tx` opened go through [`missing_ids_on`] first.
pub(crate) fn insert_items_tx(
    tx: &Transaction<'_>,
    playlist_id: i64,
    items: &[NewItem<'_>],
) -> Result<Vec<i64>, AppError> {
    const COLS_PER_ROW: usize = 4;
    const CHUNK_SIZE: usize = MAX_BINDS_PER_STATEMENT / COLS_PER_ROW;

    let mut landed = Vec::with_capacity(items.len());
    for chunk in items.chunks(CHUNK_SIZE) {
        let mut sql = Sql::trusted(format!(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, track_id, position, added_at) \
             VALUES {} RETURNING track_id",
            values_placeholders(chunk.len(), COLS_PER_ROW)
        ));
        for item in chunk {
            sql.bind(playlist_id).bind(item.track_id).bind(item.position).bind(item.added_at);
        }
        landed.extend(sql.fetch_all::<i64>(tx)?);
    }
    Ok(landed)
}

/// `track_ids` in their order, less any the library no longer holds.
fn present_in_order(tx: &Transaction<'_>, track_ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let missing = missing_ids_on(tx, track_ids)?;
    Ok(track_ids.iter().copied().filter(|id| !missing.contains(id)).collect())
}

/// Removes `track_ids` from a playlist, answering what puts them back, or only how many went where
/// there were more than [`UNDO_HOLD_CAP`]: those are removed for good, nothing kept.
pub async fn remove_tracks_from_playlist_batch(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<UndoCapture<PlaylistItemsCut>, AppError> {
    if track_ids.is_empty() {
        return Ok(UndoCapture::Nothing);
    }
    if track_ids.len() > UNDO_HOLD_CAP {
        let rows = remove_tracks_from_playlist_for_good(db, playlist_id, track_ids).await?;
        return Ok(if rows == 0 { UndoCapture::Nothing } else { UndoCapture::OverCap { rows } });
    }
    db.transaction(move |tx| cut_items(tx, playlist_id, &track_ids)).await
}

fn cut_items(
    tx: &Transaction<'_>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<UndoCapture<PlaylistItemsCut>, AppError> {
    let rows = cut_rows_tx(tx, playlist_id, track_ids)?;
    if rows.is_empty() {
        return Ok(UndoCapture::Nothing);
    }
    let (created_at, updated_before): (String, String) = sql::fetch_one(
        tx,
        "SELECT created_at, updated_at FROM playlists WHERE id = ?",
        (playlist_id,),
    )?;

    delete_items_tx(tx, playlist_id, track_ids)?;
    renumber_playlist_positions_tx(tx, playlist_id)?;
    let stamped_at = melodia_core::utils::now_rfc3339();
    update_playlist_thumbnail_and_timestamp_tx(tx, playlist_id, &stamped_at)?;

    let playlist = PlaylistIdentity { id: playlist_id, created_at };
    Ok(UndoCapture::Held(PlaylistItemsCut { playlist, rows, updated_before, stamped_at }))
}

/// Removes `track_ids` from a playlist keeping nothing, and answers how many rows went. Removing
/// none renumbers and stamps nothing.
pub async fn remove_tracks_from_playlist_for_good(
    db: &DbPool,
    playlist_id: i64,
    track_ids: Vec<i64>,
) -> Result<usize, AppError> {
    if track_ids.is_empty() {
        return Ok(0);
    }
    db.transaction(move |tx| {
        let removed = delete_items_tx(tx, playlist_id, &track_ids)?;
        if removed == 0 {
            return Ok(0);
        }
        renumber_playlist_positions_tx(tx, playlist_id)?;
        let now = melodia_core::utils::now_rfc3339();
        update_playlist_thumbnail_and_timestamp_tx(tx, playlist_id, &now)?;
        Ok(removed)
    })
    .await
}

/// The rows `track_ids` hold in the playlist, ascending, each with its index in the playlist's
/// order: the positions themselves can carry gaps under a hard-deleted track.
fn cut_rows_tx(
    tx: &Transaction<'_>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<Vec<CutItem>, AppError> {
    let mut rows = Vec::with_capacity(track_ids.len());
    let statements = chunked_in_statements(playlist_id, track_ids, |placeholders| {
        format!(
            "SELECT track_id, row, added_at FROM (
                SELECT track_id, added_at, ROW_NUMBER() OVER (ORDER BY position, id) - 1 AS row
                FROM playlist_items WHERE playlist_id = ?
            ) WHERE track_id IN ({placeholders})"
        )
    });
    for sql in statements {
        let found: Vec<(i64, i64, String)> = sql.fetch_all(tx)?;
        for (track_id, row, added_at) in found {
            let row = usize::try_from(row).unwrap_or(usize::MAX);
            rows.push(CutItem { track_id, row, added_at });
        }
    }
    rows.sort_unstable_by_key(|item| item.row);
    Ok(rows)
}

/// Deletes `track_ids`' rows from a playlist and returns how many went.
fn delete_items_tx(
    tx: &Transaction<'_>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<usize, AppError> {
    let statements = chunked_in_statements(playlist_id, track_ids, |placeholders| {
        format!("DELETE FROM playlist_items WHERE playlist_id = ? AND track_id IN ({placeholders})")
    });
    let mut removed: u64 = 0;
    for sql in statements {
        removed += sql.execute(tx)?;
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
    track_ids: Vec<i64>,
    to: usize,
) -> Result<(), AppError> {
    db.transaction(move |tx| move_items(tx, playlist_id, &track_ids, to)).await
}

fn move_items(
    tx: &Transaction<'_>,
    playlist_id: i64,
    track_ids: &[i64],
    to: usize,
) -> Result<(), AppError> {
    let mut items: Vec<(i64, i64, i32)> = sql::fetch_all(
        tx,
        "SELECT id, track_id, position FROM playlist_items WHERE playlist_id = ?
         ORDER BY position, id",
        (playlist_id,),
    )?;

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
    batch_update_positions_pairs(tx, &changed)?;
    let now = melodia_core::utils::now_rfc3339();
    update_playlist_thumbnail_and_timestamp_tx(tx, playlist_id, &now)
}

/// Closes the gaps in a playlist's positions, writing only the rows that move.
pub(crate) fn renumber_playlist_positions_tx(
    tx: &Transaction<'_>,
    playlist_id: i64,
) -> Result<(), AppError> {
    sql::execute(
        tx,
        "WITH numbered AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY position, id) - 1 AS new_pos
            FROM playlist_items WHERE playlist_id = ?
        )
        UPDATE playlist_items SET position = numbered.new_pos
        FROM numbered
        WHERE numbered.id = playlist_items.id AND playlist_items.position <> numbered.new_pos",
        (playlist_id,),
    )?;
    Ok(())
}

/// Writes `(id, position)` pairs in chunks, which is safe because the positions are absolute and
/// each chunk touches its own ids.
pub(crate) fn batch_update_positions_pairs(
    tx: &Transaction<'_>,
    pairs: &[(i64, i32)],
) -> Result<(), AppError> {
    const CHUNK_SIZE: usize = MAX_BINDS_PER_STATEMENT / CASE_BINDS_PER_ROW;

    if pairs.is_empty() {
        return Ok(());
    }

    for chunk in pairs.chunks(CHUNK_SIZE) {
        let mut sql = Sql::trusted(format!(
            "UPDATE playlist_items SET position = {} WHERE id IN ({})",
            case_by_id(chunk.len()),
            placeholders(chunk.len()),
        ));
        for &(id, pos) in chunk {
            sql.bind(id).bind(pos);
        }
        for &(id, _) in chunk {
            sql.bind(id);
        }
        sql.execute(tx)?;
    }
    Ok(())
}

/// How many of `track_ids` each playlist already holds, for the Add-to-Playlist picker's greyed
/// rows and "2 of 4 already added" badges. Summed across chunks, a playlist turning up in several.
pub async fn count_tracks_in_playlists_for_selection(
    db: &DbPool,
    track_ids: Vec<i64>,
) -> Result<HashMap<i64, i64>, AppError> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<(i64, i64)> = db
        .read(move |conn| {
            chunked_in_query(conn, &track_ids, |placeholders| {
                format!(
                    "SELECT playlist_id, COUNT(*) FROM playlist_items \
                     WHERE track_id IN ({placeholders}) \
                     GROUP BY playlist_id"
                )
            })
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
    path: String,
) -> Result<playlist::Playlist, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    db.write(move |conn| {
        sql::fetch_optional(
            conn,
            "UPDATE playlists SET thumbnail_path = ?, custom_thumbnail = TRUE, updated_at = ?
             WHERE id = ? RETURNING *",
            (path, now, playlist_id),
        )
    })
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
pub(crate) fn update_playlist_thumbnail_and_timestamp_tx(
    tx: &Transaction<'_>,
    playlist_id: i64,
    stamp: &str,
) -> Result<(), AppError> {
    sql::execute(
        tx,
        concat!(
            "UPDATE playlists SET thumbnail_path = CASE WHEN custom_thumbnail = FALSE THEN ",
            first_track_cover!(),
            " ELSE thumbnail_path END, updated_at = ? WHERE id = ?"
        ),
        (stamp, playlist_id),
    )?;
    Ok(())
}

/// Points every automatic playlist's thumbnail back at its first track's cover.
///
/// For what reaches a playlist without an edit to it: a track deleted out from under it, or a
/// cover cleared because its file went missing. `updated_at` stays put: the user changed nothing.
pub(crate) fn refresh_automatic_thumbnails(tx: &Transaction<'_>) -> Result<(), AppError> {
    sql::execute(
        tx,
        concat!(
            "UPDATE playlists SET thumbnail_path = ",
            first_track_cover!(),
            " WHERE custom_thumbnail = FALSE AND thumbnail_path IS NOT ",
            first_track_cover!()
        ),
        (),
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/playlist_tests.rs"]
mod tests;
