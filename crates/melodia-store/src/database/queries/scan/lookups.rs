//! The reads the scan and the watcher batch make.

use futures_util::TryStreamExt;

use melodia_core::entities::scan::ExistingTrackSummary;
use melodia_core::error::AppError;

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

/// The library folder holding `file_path`, by longest prefix, or `None` outside every one.
///
/// **The separator is bound, never spelled**: a Windows folder is `C:\Music`, and a hardcoded `/`
/// answers `None` for every file there, which callers read as outside the library. `/` is
/// compared too, Win32 accepting it and playlists and URIs carrying it. Only the boundary
/// separator is asked, so three shapes still miss: a path spelled with `/` throughout against a
/// `\` folder, a root folder already ending in a separator, and a case difference, `SQLite`
/// comparing bytes.
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
