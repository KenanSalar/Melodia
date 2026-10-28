//! The `TrackListRow` fetches every track list renders from.

use sqlx::AssertSqlSafe;

use super::columns;
use crate::database::DbPool;
use melodia_core::entities::browse::DirectoryTrack;
use melodia_core::entities::track;
use melodia_core::error::AppError;

/// The `ORDER BY` body a whole-table fetch shares: the scan-time natural key.
///
/// **Not a page's order**, which is its keyset's: copied into a batch-action query it would queue
/// Artist and Genre Detail's `"album"` default by title. The clause is for **determinism**, the
/// comparator these rows feed breaking ties on `sort_key` with a stable sort, and costs an index
/// scan of `idx_tracks_sort_key` rather than a sort.
pub(super) const TRACK_LIST_ORDER: &str = "sort_key COLLATE NOCASE ASC";

/// Every track as a list row, in [`TRACK_LIST_ORDER`], for the checks that hold an id query
/// against them; a list fetches its rows by id.
pub async fn get_all_tracks_for_list(db: &DbPool) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = columns::TRACK_LIST.joined();
    let sql = format!("SELECT {cols} FROM tracks ORDER BY {TRACK_LIST_ORDER}");
    let tracks =
        sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql)).fetch_all(db.read()).await?;
    Ok(tracks)
}

/// The native path separator as it must appear *inside* a LIKE pattern. `\` is the ESCAPE
/// character, so on Windows a literal backslash is doubled.
#[cfg(windows)]
pub(super) const LIKE_SEPARATOR: &str = "\\\\";
#[cfg(not(windows))]
pub(super) const LIKE_SEPARATOR: &str = "/";

/// `dir_path` escaped for `LIKE … ESCAPE '\'` and ended with [`LIKE_SEPARATOR`], the prefix every
/// "files under this directory" pattern starts from: a user's folder name can hold `_` or `%`.
pub(super) fn directory_like_prefix(dir_path: &str) -> String {
    let escaped = dir_path.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("{escaped}{LIKE_SEPARATOR}")
}

/// The tracks directly inside `dir_path`, each with the path Browse matches its file by: `LIKE`
/// folds ASCII case, so a sibling folder spelled in another case answers here too.
///
/// `file_path` holds the native separator, so `dir_path` comes in native form through
/// `canonicalize_path`. The `ESCAPE` clause rules out `SQLite`'s prefix range scan, and a scan per
/// navigation doesn't justify a generated `parent_dir` column and index slowing every insert.
pub async fn get_tracks_in_directory(
    db: &DbPool,
    dir_path: &str,
) -> Result<Vec<DirectoryTrack>, AppError> {
    let prefix = directory_like_prefix(dir_path);
    let pattern = format!("{prefix}%");
    let subdir_pattern = format!("{prefix}%{LIKE_SEPARATOR}%");

    let cols = columns::TRACK_LIST.joined();
    let sql = format!(
        "SELECT file_path, {cols} FROM tracks WHERE file_path LIKE ? ESCAPE '\\' AND file_path NOT LIKE ? ESCAPE '\\'"
    );
    let tracks = sqlx::query_as::<_, DirectoryTrack>(AssertSqlSafe(sql))
        .bind(&pattern)
        .bind(&subdir_pattern)
        .fetch_all(db.read())
        .await?;

    Ok(tracks)
}

/// The `limit` most recently played tracks, newest first, which the view re-sorts and filters in
/// memory rather than re-querying.
///
/// RFC 3339 `last_played` sorts chronologically as text whatever width `AutoSi` gives the fraction,
/// `+` sorting below every digit. The partial `idx_tracks_last_played` leaves out the never-played,
/// and `id DESC` settles a flush's shared stamp in the order that index's walk returns anyway.
pub async fn get_recently_played(
    db: &DbPool,
    limit: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = columns::TRACK_LIST.joined();
    let sql = format!(
        "SELECT {cols} FROM tracks \
         WHERE last_played IS NOT NULL \
         ORDER BY last_played DESC, id DESC \
         LIMIT ?"
    );
    let rows = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(db.read())
        .await?;
    Ok(rows)
}
