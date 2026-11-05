use rusqlite::Transaction;

use crate::database::DbPool;
use crate::database::queries::{playlist, scan};
use crate::database::sql::{self, Sql};
use melodia_core::entities::folder;
use melodia_core::error::AppError;

pub async fn insert_folder(
    db: &DbPool,
    path: &str,
    is_enabled: bool,
) -> Result<folder::Folder, AppError> {
    let path = path.to_owned();
    let now = melodia_core::utils::now_rfc3339();
    db.write(move |conn| {
        sql::fetch_one(
            conn,
            "INSERT INTO folders (path, is_enabled, added_at)
             VALUES (?, ?, ?)
             RETURNING *",
            (path, is_enabled, now),
        )
    })
    .await
}

pub async fn get_all_folders(db: &DbPool) -> Result<Vec<folder::Folder>, AppError> {
    db.read(|conn| sql::fetch_all(conn, "SELECT * FROM folders", ())).await
}

pub async fn get_folder_by_id(db: &DbPool, id: i64) -> Result<folder::Folder, AppError> {
    db.read(move |conn| sql::fetch_optional(conn, "SELECT * FROM folders WHERE id = ?", (id,)))
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
    db.transaction(move |tx| {
        sql::execute(tx, "DELETE FROM folders WHERE id = ?", (id,))?;
        scan::prune_orphans(tx)?;
        playlist::refresh_automatic_thumbnails(tx)
    })
    .await
}

/// Hands the tracks and kept-out files of the folders in `ids` to `parent_id`, then deletes those
/// folders.
///
/// Both move before the rows go, so the cascade has nothing left to take: what a parent
/// supersedes keeps its ratings, play counts, favourites and playlist entries, and a kept-out file
/// stays out. Neither the search index nor the stats triggers watch `folder_id`, so the move
/// rewrites nothing else.
pub async fn absorb_folders(db: &DbPool, parent_id: i64, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut statements = Vec::new();
    // One bind of each budget goes to the parent's id.
    for chunk in ids.chunks(crate::database::MAX_BINDS_PER_STATEMENT - 1) {
        let placeholders = crate::database::placeholders(chunk.len());
        for table in ["tracks", "excluded_files"] {
            let mut repoint = Sql::trusted(format!(
                "UPDATE {table} SET folder_id = ? WHERE folder_id IN ({placeholders})"
            ));
            repoint.bind(parent_id);
            for &id in chunk {
                repoint.bind(id);
            }
            statements.push(repoint);
        }

        let mut delete = Sql::trusted(format!("DELETE FROM folders WHERE id IN ({placeholders})"));
        for &id in chunk {
            delete.bind(id);
        }
        statements.push(delete);
    }
    db.transaction(move |tx| {
        for statement in statements {
            statement.execute(tx)?;
        }
        Ok(())
    })
    .await
}

/// Finds or creates a folder by path, returning its id. A created one is disabled, so it is neither
/// watched nor listed as a library folder; an existing one is left as it is.
pub(crate) fn upsert_folder(tx: &Transaction<'_>, path: &str) -> Result<i64, AppError> {
    let now = melodia_core::utils::now_rfc3339();
    sql::fetch_one(
        tx,
        "INSERT INTO folders (path, is_enabled, added_at)
         VALUES (?, FALSE, ?)
         ON CONFLICT(path) DO UPDATE SET path = excluded.path
         RETURNING id",
        (path, now),
    )
}

pub async fn update_folder_last_scanned(
    db: &DbPool,
    id: i64,
    timestamp: String,
) -> Result<(), AppError> {
    db.write(move |conn| {
        sql::execute(conn, "UPDATE folders SET last_scanned = ? WHERE id = ?", (timestamp, id))
    })
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/folder_tests.rs"]
mod tests;
