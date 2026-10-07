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

/// Deletes the folder, its tracks by cascade, and whatever those tracks leave behind.
///
/// The prune and the thumbnail refresh share the delete's transaction because nothing else would
/// run them: only a scan that changed something does, and an album left without tracks or a
/// playlist still showing one of their covers keeps that cover referenced, so the artwork sweep
/// never retires it.
pub async fn delete_folder(db: &DbPool, id: i64) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    sqlx::query("DELETE FROM folders WHERE id = ?").bind(id).execute(&mut *tx).await?;
    crate::database::queries::scan::prune_orphans(&mut tx).await?;
    crate::database::queries::playlist::refresh_automatic_thumbnails(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Hands the tracks of the folders in `ids` to `parent_id`, then deletes those folders.
///
/// The tracks move before the rows go, so the cascade has nothing left to take: what a parent
/// supersedes keeps its ratings, play counts, favourites and playlist entries. Neither the search
/// index nor the stats triggers watch `folder_id`, so the move rewrites nothing else.
pub async fn absorb_folders(db: &DbPool, parent_id: i64, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = db.write().begin().await?;
    // One bind of each budget goes to the parent's id.
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("UPDATE tracks SET folder_id = ? WHERE folder_id IN ({placeholders})");
        let mut repoint = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(parent_id);
        for id in chunk {
            repoint = repoint.bind(id);
        }
        repoint.execute(&mut *tx).await?;

        let sql = format!("DELETE FROM folders WHERE id IN ({placeholders})");
        let mut delete = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for id in chunk {
            delete = delete.bind(id);
        }
        delete.execute(&mut *tx).await?;
    }
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
