//! The reads the scan and the watcher batch make.

use std::collections::HashMap;

use rusqlite::Transaction;

use crate::database::DbPool;
use crate::database::sql;
use melodia_core::entities::scan::FileStamp;
use melodia_core::error::AppError;

pub(crate) fn track_exists_by_path(
    tx: &Transaction<'_>,
    file_path: &str,
) -> Result<bool, AppError> {
    let exists: Option<i64> =
        sql::fetch_optional(tx, "SELECT 1 FROM tracks WHERE file_path = ? LIMIT 1", (file_path,))?;
    Ok(exists.is_some())
}

/// The library folder holding `file_path`, by longest prefix, or `None` outside every one. An
/// import's folder is passed by, so a file below a directory something was once imported from is
/// filed under its library folder rather than with that import.
///
/// **The separator is bound, never spelled**: a Windows folder is `C:\Music`, and a hardcoded `/`
/// answers `None` for every file there, which callers read as outside the library. `/` is
/// compared too, Win32 accepting it and playlists and URIs carrying it. A root folder already ends
/// in a separator, so it is trimmed before one is compared. Only the boundary separator is asked,
/// so two shapes still miss: a path spelled with `/` throughout against a `\` folder below a root,
/// and a case difference, `SQLite` comparing bytes.
pub(crate) fn find_folder_for_path(
    tx: &Transaction<'_>,
    file_path: &str,
) -> Result<Option<i64>, AppError> {
    sql::fetch_optional(
        tx,
        "WITH enabled AS (SELECT id, RTRIM(path, '/' || ?2) AS dir FROM folders WHERE is_enabled) \
         SELECT id FROM enabled \
         WHERE SUBSTR(?1, 1, LENGTH(dir) + 1) IN (dir || '/', dir || ?2) \
         ORDER BY LENGTH(dir) DESC LIMIT 1",
        (file_path, std::path::MAIN_SEPARATOR_STR),
    )
}

/// Folds each of a folder's rows into `init` through `step`, with the columns the incremental-scan
/// filter needs, one row at a time, so the folder's paths are never all held at once. Read through
/// the read pool, before the scan's writer transaction opens.
pub async fn fold_folder_tracks<A: Send + 'static>(
    db: &DbPool,
    folder_id: i64,
    init: A,
    step: impl FnMut(&mut A, String, FileStamp) + Send + 'static,
) -> Result<A, AppError> {
    let rows = "SELECT file_path, file_size, date_modified FROM tracks WHERE folder_id = ?";
    fold_stamps(db, rows, folder_id, init, step).await
}

/// [`fold_folder_tracks`] for the folder's cue sheets.
pub async fn fold_folder_sheets<A: Send + 'static>(
    db: &DbPool,
    folder_id: i64,
    init: A,
    step: impl FnMut(&mut A, String, FileStamp) + Send + 'static,
) -> Result<A, AppError> {
    let rows = "SELECT path, file_size, date_modified FROM cue_sheets WHERE folder_id = ?";
    fold_stamps(db, rows, folder_id, init, step).await
}

async fn fold_stamps<A: Send + 'static>(
    db: &DbPool,
    rows: &'static str,
    folder_id: i64,
    init: A,
    mut step: impl FnMut(&mut A, String, FileStamp) + Send + 'static,
) -> Result<A, AppError> {
    db.read(move |conn| {
        let mut acc = init;
        sql::fetch_each(
            conn,
            rows,
            (folder_id,),
            |(path, file_size, date_modified): (String, Option<i64>, Option<String>)| {
                step(&mut acc, path, FileStamp { file_size, date_modified });
            },
        )?;
        Ok(acc)
    })
    .await
}

/// Each image a cue sheet in the folder claims, mapped to that sheet's path.
pub async fn sheet_claims(
    db: &DbPool,
    folder_id: i64,
) -> Result<HashMap<String, String>, AppError> {
    db.read(move |conn| {
        let rows: Vec<(String, String)> = sql::fetch_all(
            conn,
            "SELECT DISTINCT tracks.file_path, cue_sheets.path
             FROM cue_sheets JOIN tracks ON tracks.cue_sheet_id = cue_sheets.id
             WHERE cue_sheets.folder_id = ?",
            (folder_id,),
        )?;
        Ok(rows.into_iter().collect())
    })
    .await
}
