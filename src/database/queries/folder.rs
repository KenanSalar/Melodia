use sqlx::AssertSqlSafe;

use crate::database::DbPool;
use crate::entities::folder;
use crate::error::AppError;

pub async fn insert_folder(db: &DbPool, path: &str, is_enabled: bool) -> Result<folder::Folder, AppError> {
    let now = crate::utils::now_rfc3339();
    let row = sqlx::query_as::<_, folder::Folder>(
        "INSERT INTO folders (path, is_enabled, added_at)
         VALUES (?, ?, ?)
         RETURNING *"
    )
    .bind(path)
    .bind(is_enabled)
    .bind(&now)
    .fetch_one(db.write())
    .await?;
    Ok(row)
}

pub async fn get_all_folders(db: &DbPool) -> Result<Vec<folder::Folder>, AppError> {
    let folders = sqlx::query_as::<_, folder::Folder>("SELECT * FROM folders")
        .fetch_all(db.read())
        .await?;
    Ok(folders)
}

pub async fn get_folder_by_id(db: &DbPool, id: i64) -> Result<folder::Folder, AppError> {
    sqlx::query_as::<_, folder::Folder>("SELECT * FROM folders WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Folder", id))
}

pub async fn delete_folder(db: &DbPool, id: i64) -> Result<(), AppError> {
    // ON DELETE CASCADE on tracks.folder_id handles child deletion
    sqlx::query("DELETE FROM folders WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Batch delete by id list. Used by `add_folder` to remove subfolders covered
/// by a newly-added parent — one round-trip instead of one per child.
/// Chunks at `SQLite`'s bind-variable limit so we never bust 999 placeholders.
pub async fn delete_folders_by_ids(db: &DbPool, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    for chunk in ids.chunks(crate::database::SQLITE_BIND_LIMIT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("DELETE FROM folders WHERE id IN ({placeholders})");
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for id in chunk {
            q = q.bind(id);
        }
        q.persistent(false).execute(db.write()).await?;
    }
    Ok(())
}

/// Get or create a folder by path, returning the folder ID.
/// Created folders have `is_enabled = FALSE` so they are not watched or shown as library folders.
/// If the folder already exists, its existing ID is returned without modification.
pub async fn upsert_folder(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &str,
) -> Result<i64, AppError> {
    let now = crate::utils::now_rfc3339();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO folders (path, is_enabled, added_at)
         VALUES (?, FALSE, ?)
         ON CONFLICT(path) DO UPDATE SET path = excluded.path
         RETURNING id"
    )
    .bind(path)
    .bind(&now)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn update_folder_last_scanned(db: &DbPool, id: i64, timestamp: &str) -> Result<(), AppError> {
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
