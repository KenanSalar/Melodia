//! Read-side helpers consumed by the scan and the watcher batch:
//! exists-by-path, folder resolution by path prefix, and the folder read
//! that gates the parallel scan's per-file work. Move detection resolves its
//! hashes in one batch, through `track::lowest_id_by_hash_on`.

use futures_util::TryStreamExt;

use melodia_core::entities::scan::ExistingTrackSummary;
use melodia_core::error::AppError;

/// Check if a track with the given file path already exists.
pub(crate) async fn track_exists_by_path(
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
///
/// **The separator is bound, never spelled.** Paths are stored exactly as the OS handed
/// them over, so a Windows library folder is `C:\Music` and its tracks `C:\Music\a.mp3` —
/// against a hardcoded `'/'` that answers `None` for every file on the platform, and each
/// caller reads that as "outside the library": a tag write refuses the file, and the
/// watcher drops a create, rename or modify on the floor. `/` stays in the
/// comparison unconditionally rather than being swapped out, Win32 accepting it as a
/// separator too and a path that arrived through a playlist or a URI carrying it.
///
/// Only the separator at the *boundary* is answered, that being the only character the
/// prefix comparison reaches. Three shapes still miss, none of them new: a path spelled
/// with `/` throughout (`C:/Music/a.mp3` against a folder picked as `C:\Music` —
/// `std::path::absolute` doesn't normalise those), a folder that is a drive or filesystem
/// root and so already ends in a separator, and a case difference on a filesystem that
/// doesn't care about one, `SQLite` comparing bytes.
pub(crate) async fn find_folder_for_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<Option<i64>, AppError> {
    let id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM folders WHERE SUBSTR(?, 1, LENGTH(path) + 1) IN (path || '/', path || ?) \
         ORDER BY LENGTH(path) DESC LIMIT 1",
    )
    .bind(file_path)
    .bind(std::path::MAIN_SEPARATOR_STR)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(id)
}

/// Hands each of a folder's rows to `visit`, with the columns the incremental-scan filter needs,
/// one row at a time, so the folder's paths are never all held at once. Read through the read
/// pool, before the scan's writer transaction opens.
pub async fn visit_folder_tracks(
    db: &crate::database::DbPool,
    folder_id: i64,
    mut visit: impl FnMut(String, ExistingTrackSummary),
) -> Result<(), AppError> {
    let mut rows = sqlx::query_as::<_, (String, Option<i64>, Option<String>)>(
        "SELECT file_path, file_size, date_modified FROM tracks WHERE folder_id = ?",
    )
    .bind(folder_id)
    .fetch(db.read());

    while let Some((path, file_size, date_modified)) = rows.try_next().await? {
        visit(path, ExistingTrackSummary { file_size, date_modified });
    }
    Ok(())
}

/// Get a track's ID by its file path. Returns None if not found.
pub(crate) async fn get_track_id_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<Option<i64>, AppError> {
    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM tracks WHERE file_path = ? LIMIT 1")
        .bind(file_path)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(id)
}
