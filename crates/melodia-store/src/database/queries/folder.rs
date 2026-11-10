use rusqlite::{Connection, Transaction};

use crate::database::queries::{playlist, scan};
use crate::database::{DbPool, chunked_in_execute, chunked_in_statements, sql};
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

pub(crate) fn paths_on(conn: &Connection) -> Result<Vec<String>, AppError> {
    sql::fetch_all(conn, "SELECT path FROM folders", ())
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

/// Hands the tracks, cue sheets and kept-out files of the folders in `ids` to `parent_id`, then
/// deletes those folders.
///
/// All three move before the rows go, so the cascade has nothing left to take: what a parent
/// supersedes keeps its ratings, play counts, favourites and playlist entries, a sheet keeps the
/// tracks that cascade from it, and a kept-out file stays out. Neither the search index nor the
/// stats triggers watch `folder_id`, so the move rewrites nothing else.
pub async fn absorb_folders(db: &DbPool, parent_id: i64, ids: &[i64]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Ok(());
    }
    let ids = ids.to_vec();
    db.transaction(move |tx| {
        for table in ["tracks", "cue_sheets", "excluded_files"] {
            let repoints = chunked_in_statements(parent_id, &ids, |placeholders| {
                format!("UPDATE {table} SET folder_id = ? WHERE folder_id IN ({placeholders})")
            });
            for repoint in repoints {
                repoint.execute(tx)?;
            }
        }
        chunked_in_execute(tx, &ids, |placeholders| {
            format!("DELETE FROM folders WHERE id IN ({placeholders})")
        })?;
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
