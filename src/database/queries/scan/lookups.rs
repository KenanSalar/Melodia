//! Read-side helpers consumed by the scanner and the file-event
//! processor: exists-by-path, folder resolution by path prefix, and bulk
//! pre-reads that gate the parallel scan's per-file work. Move detection
//! resolves hashes in batch via `batch_lookup_by_hash` / the reconcile
//! pre-pass — there is no per-file hash lookup anymore.

use crate::error::AppError;

/// Check if a track with the given file path already exists.
pub async fn track_exists_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<bool, AppError> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM tracks WHERE file_path = ? LIMIT 1")
        .bind(file_path)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(exists.is_some())
}

/// Find the library folder that contains the given file path (longest prefix match).
/// Returns `None` if the path doesn't belong to any known folder.
pub async fn find_folder_for_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<Option<i64>, AppError> {
    let id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM folders WHERE SUBSTR(?, 1, LENGTH(path) + 1) = path || '/' ORDER BY LENGTH(path) DESC LIMIT 1",
    )
    .bind(file_path)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(id)
}

/// Get all track file paths for a given folder.
/// Used for orphan detection during scans and startup verification.
pub async fn get_all_track_paths_for_folder(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    folder_id: i64,
) -> Result<Vec<String>, AppError> {
    let paths = sqlx::query_scalar::<_, String>("SELECT file_path FROM tracks WHERE folder_id = ?")
        .bind(folder_id)
        .fetch_all(&mut **tx)
        .await?;
    Ok(paths)
}

/// Existing-track summary (size + mtime) feeding the incremental-scan
/// filter, which decides whether an on-disk file is unchanged and can be
/// skipped entirely. See `scanner::track_is_current`.
#[derive(Debug, Clone)]
pub struct ExistingTrackSummary {
    pub file_size: Option<i64>,
    pub date_modified: Option<String>,
}

/// Read just the columns the incremental-scan filter needs (size, mtime),
/// keyed by `file_path`. Runs against the read pool (called before the
/// writer transaction begins) so it doesn't contend with the scan's writes.
pub async fn get_existing_track_summaries_for_folder(
    db: &crate::database::DbPool,
    folder_id: i64,
) -> Result<std::collections::HashMap<String, ExistingTrackSummary>, AppError> {
    let rows = sqlx::query_as::<_, (String, Option<i64>, Option<String>)>(
        "SELECT file_path, file_size, date_modified
         FROM tracks WHERE folder_id = ?",
    )
    .bind(folder_id)
    .fetch_all(db.read())
    .await?;

    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for (path, size, mtime) in rows {
        out.insert(
            path,
            ExistingTrackSummary {
                file_size: size,
                date_modified: mtime,
            },
        );
    }
    Ok(out)
}

/// Get a track's ID by its file path. Returns None if not found.
pub async fn get_track_id_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<Option<i64>, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM tracks WHERE file_path = ? LIMIT 1")
        .bind(file_path)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(id)
}
