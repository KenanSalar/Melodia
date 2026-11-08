//! Not Duplicates: the sets of copies the duplicate finder leaves out of its list. Nothing but the
//! finder reads them, so a write here moves no library signal.

use rusqlite::Transaction;

use crate::database::{DbPool, chunked_in_statements, sql};
use melodia_core::entities::duplicates::DismissalSet;
use melodia_core::error::AppError;

/// Marks `track_ids` as one new set, moving any copy already in another. A copy deleted since the
/// find is skipped rather than failing the foreign key.
pub async fn dismiss(db: &DbPool, track_ids: &[i64]) -> Result<DismissalSet, AppError> {
    let track_ids = track_ids.to_vec();
    db.transaction(move |tx| insert_set(tx, &track_ids)).await
}

fn insert_set(tx: &Transaction<'_>, track_ids: &[i64]) -> Result<DismissalSet, AppError> {
    let set: DismissalSet =
        sql::fetch_one(tx, "SELECT COALESCE(MAX(set_id), 0) + 1 FROM duplicate_dismissals", ())?;

    let inserts = chunked_in_statements(set, track_ids, |placeholders| {
        format!(
            "INSERT OR REPLACE INTO duplicate_dismissals (track_id, set_id) \
             SELECT id, ? FROM tracks WHERE id IN ({placeholders})"
        )
    });
    for insert in inserts {
        insert.execute(tx)?;
    }
    Ok(set)
}

/// Counts a dismissed group as duplicates again.
pub async fn restore(db: &DbPool, set: DismissalSet) -> Result<(), AppError> {
    db.write(move |conn| {
        sql::execute(conn, "DELETE FROM duplicate_dismissals WHERE set_id = ?", (i64::from(set),))
    })
    .await?;
    Ok(())
}
