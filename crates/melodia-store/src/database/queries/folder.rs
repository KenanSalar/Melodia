use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use melodia_core::entities::folder;
use melodia_core::error::AppError;

pub async fn insert_folder(
    db: &DbPool,
    path: &str,
    is_enabled: bool,
) -> Result<folder::Folder, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    let row = sqlx::query_as::<_, folder::Folder>(
        "INSERT INTO folders (path, is_enabled, added_at)
         VALUES (?, ?, ?)
         RETURNING *",
    )
    .bind(path)
    .bind(is_enabled)
    .bind(&now)
    .fetch_one(db.write())
    .await?;
    Ok(row)
}

pub async fn get_all_folders(db: &DbPool) -> Result<Vec<folder::Folder>, AppError> {
    let folders =
        sqlx::query_as::<_, folder::Folder>("SELECT * FROM folders").fetch_all(db.read()).await?;
    Ok(folders)
}

pub async fn get_folder_by_id(db: &DbPool, id: i64) -> Result<folder::Folder, AppError> {
    sqlx::query_as::<_, folder::Folder>("SELECT * FROM folders WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Folder", id))
}

/// Deletes the folders, their tracks by cascade, and whatever those tracks leave behind.
///
/// The prune shares the delete's transaction because nothing else would run it: only a scan that
/// changed something prunes, and an album left without tracks keeps its cover referenced, so the
/// artwork sweep never retires it. Chunks at [`MAX_BINDS_PER_STATEMENT`] so a long id list stays
/// inside one statement's budget.
///
/// [`MAX_BINDS_PER_STATEMENT`]: crate::database::MAX_BINDS_PER_STATEMENT
pub async fn delete_folders(db: &DbPool, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = db.write().begin().await?;
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("DELETE FROM folders WHERE id IN ({placeholders})");
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for id in chunk {
            q = q.bind(id);
        }
        q.persistent(false).execute(&mut *tx).await?;
    }
    crate::database::queries::scan::prune_orphans(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Get or create a folder by path, returning the folder ID.
/// Created folders have `is_enabled = FALSE` so they are not watched or shown as library folders.
/// If the folder already exists, its existing ID is returned without modification.
pub async fn upsert_folder(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &str,
) -> Result<i64, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO folders (path, is_enabled, added_at)
         VALUES (?, FALSE, ?)
         ON CONFLICT(path) DO UPDATE SET path = excluded.path
         RETURNING id",
    )
    .bind(path)
    .bind(&now)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn update_folder_last_scanned(
    db: &DbPool,
    id: i64,
    timestamp: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE folders SET last_scanned = ? WHERE id = ?")
        .bind(timestamp)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/folder_tests.rs"]
mod tests;
