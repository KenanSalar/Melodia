//! Files the duplicate finder took out of the library, which no scan or watcher event brings back.
//!
//! A path here never has a track row: the merge deletes the row it excludes, an import lifts the
//! exclusion ahead of its insert, and every other ingest path asks here inside its own write
//! transaction, so a walk read before an exclusion committed still can't bring its file back.

use std::collections::{HashMap, HashSet};

use sqlx::sqlite::SqliteConnection;
use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite, Transaction};

use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_query, chunked_in_query_on};
use melodia_core::error::AppError;

/// A spare's file as the merge keeps it out.
pub(crate) struct NewExclusion<'a> {
    pub(crate) path: &'a str,
    pub(crate) file_hash: Option<&'a str>,
    pub(crate) folder_id: i64,
}

pub(crate) async fn exclude_on(
    tx: &mut Transaction<'_, Sqlite>,
    files: &[NewExclusion<'_>],
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 4;
    let excluded_at = melodia_core::utils::now_rfc3339();
    for chunk in files.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO excluded_files (path, file_hash, folder_id, excluded_at) ",
        );
        builder.push_values(chunk, |mut row, file| {
            row.push_bind(file.path)
                .push_bind(file.file_hash)
                .push_bind(file.folder_id)
                .push_bind(excluded_at.as_str());
        });
        builder.build().persistent(false).execute(&mut **tx).await?;
    }
    Ok(())
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

/// Every kept-out path under `root`, by prefix rather than by folder row: a file dropped in from
/// a library folder sits under the disabled row its import made for the file's own directory.
/// The separator is bound as `find_folder_for_path` binds it, for the same reason.
pub async fn under(db: &DbPool, root: &str) -> Result<HashSet<String>, AppError> {
    let paths = sqlx::query_scalar::<_, String>(
        "SELECT path FROM excluded_files \
         WHERE SUBSTR(path, 1, LENGTH(?1) + 1) IN (?1 || '/', ?1 || ?2)",
    )
    .bind(root)
    .bind(std::path::MAIN_SEPARATOR_STR)
    .fetch_all(db.read())
    .await?;
    Ok(paths.into_iter().collect())
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
    let mut lifted = 0;
    for chunk in paths.chunks(MAX_BINDS_PER_STATEMENT) {
        let sql = format!(
            "DELETE FROM excluded_files WHERE path IN ({})",
            crate::database::placeholders(chunk.len())
        );
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for path in chunk {
            query = query.bind(*path);
        }
        lifted += query.persistent(false).execute(&mut **tx).await?.rows_affected();
    }
    Ok(lifted)
}
