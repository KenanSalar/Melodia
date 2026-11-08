//! Files the duplicate finder took out of the library, which scans and the watcher leave out.
//!
//! A kept-out file is known by its path, and across a move by its whole-file hash as last read, as
//! a track's row is, so one rewritten and then moved while nothing watched reads as a new file.
//!
//! A path here never has a track row: the merge deletes the row it excludes, an import lifts the
//! exclusion ahead of its insert, and every other ingest path asks here inside its own write
//! transaction, so a walk read before an exclusion committed still can't bring its file back.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, Transaction};

use crate::database::sql::{self, Sql};
use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_execute, chunked_in_query, values_placeholders,
};
use melodia_core::entities::duplicates::KeptOutFile;
use melodia_core::entities::scan::{ExtractedMetadata, FileStamp};
use melodia_core::error::AppError;

/// A spare's file as the merge keeps it out.
pub(crate) struct NewExclusion<'a> {
    pub(crate) path: &'a str,
    pub(crate) file_hash: Option<&'a str>,
    pub(crate) file_size: Option<i64>,
    pub(crate) date_modified: Option<&'a str>,
    pub(crate) folder_id: i64,
}

pub(crate) fn exclude_on(tx: &Transaction<'_>, files: &[NewExclusion<'_>]) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 6;
    let excluded_at = melodia_core::utils::now_rfc3339();
    for chunk in files.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let mut sql = Sql::vouched(format!(
            "INSERT INTO excluded_files \
             (path, file_hash, file_size, date_modified, folder_id, excluded_at) VALUES {}",
            values_placeholders(chunk.len(), COLS_PER_ROW)
        ));
        for file in chunk {
            sql.bind(file.path)
                .bind(file.file_hash)
                .bind(file.file_size)
                .bind(file.date_modified)
                .bind(file.folder_id)
                .bind(excluded_at.as_str());
        }
        sql.execute(tx)?;
    }
    Ok(())
}

/// Gives the kept-out files at `files` the hash and stamp they were just read with, so each is
/// followed by its bytes as they now are.
pub(crate) fn restamp_on(
    tx: &Transaction<'_>,
    files: &[(&Path, &ExtractedMetadata)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 4;
    for chunk in files.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
        let mut sql = Sql::vouched(format!(
            "WITH v(path, file_hash, file_size, date_modified) AS (VALUES {rows}) \
             UPDATE excluded_files SET file_hash = v.file_hash, file_size = v.file_size, \
                    date_modified = v.date_modified \
             FROM v WHERE excluded_files.path = v.path"
        ));
        for (path, meta) in chunk {
            sql.bind(path.to_string_lossy().into_owned())
                .bind(meta.file_hash.as_str())
                .bind(meta.file_size)
                .bind(meta.date_modified.as_deref());
        }
        sql.execute(tx)?;
    }
    Ok(())
}

pub async fn count(db: &DbPool) -> Result<i64, AppError> {
    db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM excluded_files", ())).await
}

/// Every kept-out file's id, in path order: the kept-out list's keyset.
pub async fn ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    db.read(|conn| sql::fetch_all(conn, "SELECT id FROM excluded_files ORDER BY path", ())).await
}

/// The kept-out files behind `ids`, in no particular order. An id lifted since has no row.
pub async fn files(db: &DbPool, ids: Vec<i64>) -> Result<Vec<KeptOutFile>, AppError> {
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |placeholders| {
            format!("SELECT id, path, excluded_at FROM excluded_files WHERE id IN ({placeholders})")
        })
    })
    .await
}

/// Every kept-out path, in path order.
pub async fn paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    db.read(|conn| sql::fetch_all(conn, "SELECT path FROM excluded_files ORDER BY path", ())).await
}

/// The kept-out paths among `paths`, off the read pool.
pub async fn among(db: &DbPool, paths: Vec<String>) -> Result<HashSet<String>, AppError> {
    db.read(move |conn| {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        among_on(conn, &paths)
    })
    .await
}

/// [`among`] on a connection the caller holds, inside the transaction that would insert them.
pub(crate) fn among_on(conn: &Connection, paths: &[&str]) -> Result<HashSet<String>, AppError> {
    let rows: Vec<String> = chunked_in_query(conn, paths, |placeholders| {
        format!("SELECT path FROM excluded_files WHERE path IN ({placeholders})")
    })?;
    Ok(rows.into_iter().collect())
}

/// The kept-out files filed under `folder_id`, each with its stamp.
pub async fn in_folder(db: &DbPool, folder_id: i64) -> Result<Vec<(String, FileStamp)>, AppError> {
    let rows: Vec<(String, Option<i64>, Option<String>)> = db
        .read(move |conn| {
            sql::fetch_all(
                conn,
                "SELECT path, file_size, date_modified FROM excluded_files WHERE folder_id = ?",
                (folder_id,),
            )
        })
        .await?;
    let files = rows
        .into_iter()
        .map(|(path, file_size, date_modified)| (path, FileStamp { file_size, date_modified }))
        .collect();
    Ok(files)
}

/// `file_hash → [(id, path)]` of the kept-out files carrying each of `hashes`, the ones a new path
/// with those bytes may have moved from.
pub(crate) fn by_hash_on(
    conn: &Connection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    let unique: Vec<&str> = hashes.iter().copied().collect::<HashSet<_>>().into_iter().collect();
    let rows: Vec<(String, i64, String)> = chunked_in_query(conn, &unique, |placeholders| {
        format!(
            "SELECT file_hash, id, path FROM excluded_files WHERE file_hash IN ({placeholders}) \
             ORDER BY id"
        )
    })?;
    let mut by_hash: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (hash, id, path) in rows {
        by_hash.entry(hash).or_default().push((id, path));
    }
    Ok(by_hash)
}

pub(crate) fn id_at_on(conn: &Connection, path: &str) -> Result<Option<i64>, AppError> {
    sql::fetch_optional(conn, "SELECT id FROM excluded_files WHERE path = ?", (path,))
}

/// Moves an exclusion with its file. `OR REPLACE` because a kept-out file renamed over another
/// takes its path, and the file that path named is gone.
pub(crate) fn repoint_on(
    tx: &Transaction<'_>,
    id: i64,
    path: &str,
    folder_id: i64,
) -> Result<(), AppError> {
    sql::execute(
        tx,
        "UPDATE OR REPLACE excluded_files SET path = ?, folder_id = ? WHERE id = ?",
        (path, folder_id, id),
    )?;
    Ok(())
}

/// Lets the files at `paths` back into the library's reach, returning how many were kept out.
pub(crate) fn lift_on(tx: &Transaction<'_>, paths: &[&str]) -> Result<u64, AppError> {
    chunked_in_execute(tx, paths, |placeholders| {
        format!("DELETE FROM excluded_files WHERE path IN ({placeholders})")
    })
}
