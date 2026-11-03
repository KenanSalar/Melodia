//! Not Duplicates: the sets of copies the duplicate finder leaves out of its list. Nothing but the
//! finder reads them, so a write here moves no library signal.

use sqlx::AssertSqlSafe;

use crate::database::{DbPool, IDS_PER_SET, placeholders};
use melodia_core::entities::duplicates::DismissalSet;
use melodia_core::error::AppError;

/// Marks `track_ids` as one new set, moving any copy already in another. A copy deleted since the
/// find is skipped rather than failing the foreign key.
pub async fn dismiss(db: &DbPool, track_ids: &[i64]) -> Result<DismissalSet, AppError> {
    let mut tx = db.write().begin().await?;
    let set: DismissalSet =
        sqlx::query_scalar("SELECT COALESCE(MAX(set_id), 0) + 1 FROM duplicate_dismissals")
            .fetch_one(&mut *tx)
            .await?;

    for chunk in track_ids.chunks(IDS_PER_SET) {
        let sql = format!(
            "INSERT OR REPLACE INTO duplicate_dismissals (track_id, set_id) \
             SELECT id, ? FROM tracks WHERE id IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(set);
        for id in chunk {
            query = query.bind(*id);
        }
        query.execute(&mut *tx).await?;
    }

    tx.commit().await?;
    Ok(set)
}

/// Counts a dismissed group as duplicates again.
pub async fn restore(db: &DbPool, set: DismissalSet) -> Result<(), AppError> {
    sqlx::query("DELETE FROM duplicate_dismissals WHERE set_id = ?")
        .bind(set)
        .execute(db.write())
        .await?;
    Ok(())
}
