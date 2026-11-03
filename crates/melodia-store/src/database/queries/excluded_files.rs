//! Files the duplicate finder took out of the library, which no scan or watcher event brings back.
//!
//! A path here never has a track row: the merge deletes the row it excludes, an import lifts the
//! exclusion ahead of its insert, and every other ingest path asks here inside its own write
//! transaction, so a walk read before an exclusion committed still can't bring its file back.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use sqlx::sqlite::SqliteConnection;
use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite, Transaction};

use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_execute_on, chunked_in_query, chunked_in_query_on,
    values_placeholders,
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

pub(crate) async fn exclude_on(
    tx: &mut Transaction<'_, Sqlite>,
    files: &[NewExclusion<'_>],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 6;
    let excluded_at = melodia_core::utils::now_rfc3339();
    for chunk in files.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO excluded_files \
             (path, file_hash, file_size, date_modified, folder_id, excluded_at) ",
        );
        builder.push_values(chunk, |mut row, file| {
            row.push_bind(file.path)
                .push_bind(file.file_hash)
                .push_bind(file.file_size)
                .push_bind(file.date_modified)
                .push_bind(file.folder_id)
                .push_bind(excluded_at.as_str());
        });
        builder.build().persistent(false).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Gives the kept-out files at `files` the hash and stamp they were just read with, so each is
/// followed by its bytes as they now are.
pub(crate) async fn restamp_on(
    tx: &mut Transaction<'_, Sqlite>,
    files: &[(&Path, &ExtractedMetadata)],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 4;
    for chunk in files.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
        let sql = format!(
            "WITH v(path, file_hash, file_size, date_modified) AS (VALUES {rows}) \
             UPDATE excluded_files SET file_hash = v.file_hash, file_size = v.file_size, \
                    date_modified = v.date_modified \
             FROM v WHERE excluded_files.path = v.path"
        );
        let paths: Vec<Cow<'_, str>> =
            chunk.iter().map(|(path, _)| path.to_string_lossy()).collect();
        let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for (path, (_, meta)) in paths.iter().zip(chunk) {
            query = query
                .bind(path.as_ref())
                .bind(meta.file_hash.as_str())
                .bind(meta.file_size)
                .bind(meta.date_modified.as_deref());
        }
        query.execute(&mut **tx).await?;
    }
    Ok(())
}

pub async fn count(db: &DbPool) -> Result<i64, AppError> {
    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM excluded_files")
        .fetch_one(db.read())
        .await?;
    Ok(count)
}

/// Every kept-out file's id, in path order: the kept-out list's keyset.
pub async fn ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    let ids = sqlx::query_scalar::<_, i64>("SELECT id FROM excluded_files ORDER BY path")
        .fetch_all(db.read())
        .await?;
    Ok(ids)
}

/// The kept-out files behind `ids`, in no particular order. An id lifted since has no row.
pub async fn files(db: &DbPool, ids: &[i64]) -> Result<Vec<KeptOutFile>, AppError> {
    chunked_in_query(db.read(), ids, |placeholders| {
        format!("SELECT id, path, excluded_at FROM excluded_files WHERE id IN ({placeholders})")
    })
    .await
}

/// Every kept-out path, in path order.
pub async fn paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    let paths = sqlx::query_scalar::<_, String>("SELECT path FROM excluded_files ORDER BY path")
        .fetch_all(db.read())
        .await?;
    Ok(paths)
}

/// The kept-out paths among `paths`, off the read pool.
pub async fn among(db: &DbPool, paths: &[&str]) -> Result<HashSet<String>, AppError> {
    let rows: Vec<(String,)> = chunked_in_query(db.read(), paths, select_among).await?;
    Ok(rows.into_iter().map(|(path,)| path).collect())
}

/// [`among`] on a connection the caller holds, inside the transaction that would insert them.
pub(crate) async fn among_on(
    conn: &mut SqliteConnection,
    paths: &[&str],
) -> Result<HashSet<String>, AppError> {
    let rows: Vec<(String,)> = chunked_in_query_on(conn, paths, select_among).await?;
    Ok(rows.into_iter().map(|(path,)| path).collect())
}

fn select_among(placeholders: &str) -> String {
    format!("SELECT path FROM excluded_files WHERE path IN ({placeholders})")
}

/// The kept-out files filed under `folder_id`, each with its stamp.
pub async fn in_folder(db: &DbPool, folder_id: i64) -> Result<Vec<(String, FileStamp)>, AppError> {
    let rows = sqlx::query_as::<_, (String, Option<i64>, Option<String>)>(
        "SELECT path, file_size, date_modified FROM excluded_files WHERE folder_id = ?",
    )
    .bind(folder_id)
    .fetch_all(db.read())
    .await?;
    let files = rows
        .into_iter()
        .map(|(path, file_size, date_modified)| (path, FileStamp { file_size, date_modified }))
        .collect();
    Ok(files)
}

/// `file_hash → [(id, path)]` of the kept-out files carrying each of `hashes`, the ones a new path
/// with those bytes may have moved from.
pub(crate) async fn by_hash_on(
    conn: &mut SqliteConnection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<(i64, String)>>, AppError> {
    let unique: Vec<&str> = hashes.iter().copied().collect::<HashSet<_>>().into_iter().collect();
    let rows: Vec<(String, i64, String)> = chunked_in_query_on(conn, &unique, |placeholders| {
        format!(
            "SELECT file_hash, id, path FROM excluded_files WHERE file_hash IN ({placeholders}) \
             ORDER BY id"
        )
    })
    .await?;
    let mut by_hash: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (hash, id, path) in rows {
        by_hash.entry(hash).or_default().push((id, path));
    }
    Ok(by_hash)
}

pub(crate) async fn id_at_on(
    conn: &mut SqliteConnection,
    path: &str,
) -> Result<Option<i64>, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM excluded_files WHERE path = ?")
        .bind(path)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(id)
}

/// Moves an exclusion with its file. `OR REPLACE` because a kept-out file renamed over another
/// takes its path, and the file that path named is gone.
pub(crate) async fn repoint_on(
    tx: &mut Transaction<'_, Sqlite>,
    id: i64,
    path: &str,
    folder_id: i64,
) -> Result<(), AppError> {
    sqlx::query("UPDATE OR REPLACE excluded_files SET path = ?, folder_id = ? WHERE id = ?")
        .bind(path)
        .bind(folder_id)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Lets the files at `paths` back into the library's reach, returning how many were kept out.
pub(crate) async fn lift_on(
    tx: &mut Transaction<'_, Sqlite>,
    paths: &[&str],
) -> Result<u64, AppError> {
    chunked_in_execute_on(tx, paths, |placeholders| {
        format!("DELETE FROM excluded_files WHERE path IN ({placeholders})")
    })
    .await
}
