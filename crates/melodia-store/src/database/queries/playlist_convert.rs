//! Turning a smart playlist into a regular one holding what its rules match, and back.

use super::playlist::{contiguous, insert_items_tx, update_playlist_thumbnail_and_timestamp_tx};
use super::smart_playlist::member_ids_tx;
use crate::database::DbPool;
use melodia_core::entities::playlist::DroppedRules;
use melodia_core::entities::smart_criteria::{Draw, SmartCriteria};
use melodia_core::error::AppError;

/// Makes smart playlist `id` a regular one holding what its rules match now, in its page's order
/// under `draw`, and answers the rules it dropped: `None` where it is gone or already regular. It
/// keeps its name, description, a picked cover and `updated_at`, so the card keeps its place. An
/// automatic cover becomes the first item's: the card's own, unless a random cap is picked under a
/// `draw` other than the card's default.
///
/// The membership is read inside the write that flips `is_smart`: a scan's delete, serialised on
/// the one write connection, commits before it, so the rules miss the track, or after, so the
/// cascade takes the item.
pub async fn convert_to_regular(
    db: &DbPool,
    id: i64,
    draw: Draw,
) -> Result<Option<DroppedRules>, AppError> {
    let mut tx = db.write().begin().await?;
    let row: Option<(bool, Option<String>, String, String, String)> = sqlx::query_as(
        "SELECT is_smart, smart_criteria, name, created_at, updated_at FROM playlists WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((true, smart_criteria, name, created_at, updated_at)) = row else {
        return Ok(None);
    };

    let criteria = SmartCriteria::from_json_opt(smart_criteria.as_deref());
    let member_ids = member_ids_tx(&mut tx, &criteria, draw).await?;
    sqlx::query("UPDATE playlists SET is_smart = FALSE, smart_criteria = NULL WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let now = melodia_core::utils::now_rfc3339();
    insert_items_tx(&mut tx, id, &contiguous(&member_ids, 0, &now)?).await?;
    update_playlist_thumbnail_and_timestamp_tx(&mut tx, id, &updated_at).await?;

    tx.commit().await?;
    Ok(Some(DroppedRules { playlist_id: id, created_at, name, smart_criteria }))
}

/// Makes a converted playlist smart again under the rules `dropped` holds. Its items go, any added
/// since included, a smart playlist reading none; its name, description, a cover picked since and
/// `updated_at` stay as they are now.
///
/// A playlist deleted since fails with [`AppError::not_found`], even once its id is reused.
pub async fn restore_rules(db: &DbPool, dropped: &DroppedRules) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    // A smart playlist's automatic cover is worked out on read, so the stored one goes.
    let restored = sqlx::query(
        "UPDATE playlists SET is_smart = TRUE, smart_criteria = ?,
            thumbnail_path = CASE WHEN custom_thumbnail THEN thumbnail_path ELSE NULL END
         WHERE id = ? AND created_at = ?",
    )
    .bind(&dropped.smart_criteria)
    .bind(dropped.playlist_id)
    .bind(&dropped.created_at)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if restored == 0 {
        return Err(AppError::not_found("Playlist", dropped.playlist_id));
    }
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?")
        .bind(dropped.playlist_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}
