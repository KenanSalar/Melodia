//! Turning a smart playlist into a regular one holding what its rules match, and back.

use rusqlite::Transaction;

use super::playlist::{contiguous, insert_items_tx, update_playlist_thumbnail_and_timestamp_tx};
use super::smart_playlist::member_ids_tx;
use crate::database::DbPool;
use crate::database::sql;
use melodia_core::entities::playlist::{DroppedRules, PlaylistIdentity};
use melodia_core::entities::smart_criteria::{Draw, SmartCriteria};
use melodia_core::error::AppError;

/// Makes smart playlist `id` a regular one holding what its rules match now, in its page's order
/// under `draw`, and answers the rules it dropped: `None` where it is gone or already regular. It
/// keeps `updated_at`, so the card keeps its place; an automatic cover becomes the first item's.
///
/// The membership is read inside the write that flips `is_smart`: a scan's delete, serialised on
/// the one write connection, commits before it, so the rules miss the track, or after, so the
/// cascade takes the item.
pub async fn convert_to_regular(
    db: &DbPool,
    id: i64,
    draw: Draw,
) -> Result<Option<DroppedRules>, AppError> {
    db.transaction(move |tx| fill_from_rules(tx, id, draw)).await
}

fn fill_from_rules(
    tx: &Transaction<'_>,
    id: i64,
    draw: Draw,
) -> Result<Option<DroppedRules>, AppError> {
    let row: Option<(bool, Option<String>, String, String, String)> = sql::fetch_optional(
        tx,
        "SELECT is_smart, smart_criteria, name, created_at, updated_at FROM playlists WHERE id = ?",
        (id,),
    )?;
    let Some((true, smart_criteria, name, created_at, updated_at)) = row else {
        return Ok(None);
    };

    let criteria = SmartCriteria::from_json_opt(smart_criteria.as_deref());
    let member_ids = member_ids_tx(tx, &criteria, draw)?;
    sql::execute(
        tx,
        "UPDATE playlists SET is_smart = FALSE, smart_criteria = NULL WHERE id = ?",
        (id,),
    )?;
    let now = melodia_core::utils::now_rfc3339();
    insert_items_tx(tx, id, &contiguous(&member_ids, 0, &now)?)?;
    update_playlist_thumbnail_and_timestamp_tx(tx, id, &updated_at)?;

    let playlist = PlaylistIdentity { id, created_at };
    Ok(Some(DroppedRules { playlist, name, smart_criteria }))
}

/// Makes a converted playlist smart again under the rules `dropped` holds. Its items go, any added
/// since included, a smart playlist reading none; a cover picked since and `updated_at` stay.
///
/// A playlist deleted since fails with [`AppError::not_found`], even once its id is reused.
pub async fn restore_rules(db: &DbPool, dropped: DroppedRules) -> Result<(), AppError> {
    db.transaction(move |tx| {
        let playlist = &dropped.playlist;
        // A smart playlist's automatic cover is worked out on read, so the stored one goes.
        let restored = sql::execute(
            tx,
            "UPDATE playlists SET is_smart = TRUE, smart_criteria = ?,
                thumbnail_path = CASE WHEN custom_thumbnail THEN thumbnail_path ELSE NULL END
             WHERE id = ? AND created_at = ?",
            (&dropped.smart_criteria, playlist.id, &playlist.created_at),
        )?;
        if restored == 0 {
            return Err(AppError::not_found("Playlist", playlist.id));
        }
        sql::execute(tx, "DELETE FROM playlist_items WHERE playlist_id = ?", (playlist.id,))?;
        Ok(())
    })
    .await
}
