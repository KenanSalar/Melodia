//! Putting back what removals from a playlist took.

use std::collections::HashSet;

use sqlx::AssertSqlSafe;

use super::playlist::{
    NewItem, batch_update_positions_pairs, insert_items_tx,
    update_playlist_thumbnail_and_timestamp_tx,
};
use crate::database::DbPool;
use melodia_core::entities::playlist::{CutItem, PlaylistItemsCut};
use melodia_core::error::AppError;
use melodia_core::utils::reinsert::reinsert;

/// A row of the playlist as the restore rebuilds it.
enum Slot<'a> {
    Kept { id: i64, position: i32 },
    Back(&'a CutItem),
}

/// Puts back the rows `cuts` took from one playlist, `cuts` in the order they were made, each row
/// at the index it left with its own date added. Rows edited in since stay, and a restored row
/// lands at its old index clamped to the list. One whose track is gone or back in the playlist
/// stays out.
///
/// The playlist going too fails it, the rows having nowhere to go.
pub async fn restore_playlist_items(
    db: &DbPool,
    cuts: &[PlaylistItemsCut],
) -> Result<(), AppError> {
    let Some(playlist_id) = cuts.first().map(|cut| cut.playlist_id) else {
        return Ok(());
    };
    let mut tx = db.write().begin().await?;

    let live_stamp: String = sqlx::query_scalar("SELECT updated_at FROM playlists WHERE id = ?")
        .bind(playlist_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::not_found("Playlist", playlist_id))?;
    let survivors: Vec<(i64, i64, i32)> = sqlx::query_as(
        "SELECT id, track_id, position FROM playlist_items WHERE playlist_id = ?
         ORDER BY position, id",
    )
    .bind(playlist_id)
    .fetch_all(&mut *tx)
    .await?;

    let held_ids: Vec<i64> =
        cuts.iter().flat_map(|cut| &cut.rows).map(|item| item.track_id).collect();
    let in_library = tracks_still_there(&mut tx, &held_ids).await?;
    let listed: HashSet<i64> = survivors.iter().map(|&(_, track_id, _)| track_id).collect();

    // Newest first, each cut's indices being the playlist's at the moment it was made. A row that
    // stays out keeps its slot until every cut has merged, or each row after it lands one early.
    let kept: Vec<Slot<'_>> =
        survivors.iter().map(|&(id, _, position)| Slot::Kept { id, position }).collect();
    let merged = cuts.iter().rev().fold(kept, |list, cut| {
        reinsert(list, cut.rows.iter().map(|item| (item.row, Slot::Back(item))))
    });
    let restored = merged.into_iter().filter(|slot| match slot {
        Slot::Kept { .. } => true,
        Slot::Back(item) => in_library.contains(&item.track_id) && !listed.contains(&item.track_id),
    });

    let mut moved = Vec::new();
    let mut back = Vec::new();
    for (index, slot) in restored.enumerate() {
        let position = i32::try_from(index).unwrap_or(i32::MAX);
        match slot {
            Slot::Kept { id, position: was } if was != position => moved.push((id, position)),
            Slot::Kept { .. } => {}
            Slot::Back(item) => {
                back.push(NewItem { track_id: item.track_id, position, added_at: &item.added_at });
            }
        }
    }
    batch_update_positions_pairs(&mut tx, &moved).await?;
    insert_items_tx(&mut tx, playlist_id, &back).await?;
    let stamp = restored_stamp(&live_stamp, cuts);
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, playlist_id, &stamp).await?;

    tx.commit().await?;
    Ok(())
}

/// Steps back through the cuts, newest first, while `updated_at` still reads as each one left it,
/// so the card returns to its grid slot and an edit made between two cuts keeps its own stamp. A
/// playlist touched since the last cut is stamped now, the restore being an edit like any other.
fn restored_stamp(live: &str, cuts: &[PlaylistItemsCut]) -> String {
    let mut stamp: Option<&str> = None;
    for cut in cuts.iter().rev() {
        if stamp.unwrap_or(live) != cut.stamped_at {
            break;
        }
        stamp = Some(&cut.updated_before);
    }
    stamp.map_or_else(melodia_core::utils::now_rfc3339, str::to_owned)
}

/// Which of `track_ids` the library still holds. A scan hard-deletes a track whose file went, and
/// an insert naming one fails its foreign key, which `OR IGNORE` doesn't cover.
async fn tracks_still_there(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_ids: &[i64],
) -> Result<HashSet<i64>, AppError> {
    let mut present = HashSet::with_capacity(track_ids.len());
    for chunk in track_ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("SELECT id FROM tracks WHERE id IN ({placeholders})");
        let mut query = sqlx::query_scalar::<_, i64>(AssertSqlSafe(sql)).persistent(false);
        for &id in chunk {
            query = query.bind(id);
        }
        present.extend(query.fetch_all(&mut **tx).await?);
    }
    Ok(present)
}
