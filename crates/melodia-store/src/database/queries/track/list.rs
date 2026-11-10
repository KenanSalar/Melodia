//! The `TrackListRow` fetches every track list renders from.

use std::path::{MAIN_SEPARATOR, MAIN_SEPARATOR_STR};
use std::sync::LazyLock;

use super::columns;
use super::file_rows::{FILE_ORDER, IN_LIBRARY};
use crate::database::{DbPool, sql};
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

static ALL_TRACK_LIST_ROWS: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} FROM tracks WHERE {IN_LIBRARY} ORDER BY {TRACK_LIST_ORDER}",
        columns::TRACK_LIST.joined()
    )
});

/// Every library track as a list row, in [`TRACK_LIST_ORDER`], for the checks that hold an id query
/// against them; a list fetches its rows by id.
pub async fn get_all_tracks_for_list(db: &DbPool) -> Result<Vec<track::TrackListRow>, AppError> {
    db.read(|conn| sql::fetch_all(conn, ALL_TRACK_LIST_ROWS.as_str(), ())).await
}

/// The byte after the native separator: a directory ended with it sorts past everything under it.
const AFTER_SEPARATOR: char = (MAIN_SEPARATOR as u8 + 1) as char;

/// The half-open `file_path` range holding everything under `dir_path`, in the column's BINARY
/// order. A range walks the column's index and has no wildcards to escape, where a `LIKE` folds
/// case and so can't use a BINARY index.
pub(crate) fn subtree_bounds(dir_path: &str) -> (String, String) {
    // A drive root's path already ends in the separator.
    let dir = dir_path.trim_end_matches(MAIN_SEPARATOR);
    (format!("{dir}{MAIN_SEPARATOR}"), format!("{dir}{AFTER_SEPARATOR}"))
}

static TRACKS_IN_DIRECTORY: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT file_path, {} FROM tracks \
         WHERE file_path >= ?1 AND file_path < ?2 \
           AND instr(substr(file_path, length(?1) + 1), ?3) = 0 \
         ORDER BY {FILE_ORDER}",
        columns::TRACK_LIST.joined()
    )
});

/// The tracks directly inside `dir_path`, each with the path Browse matches its file by, a file's
/// rows in its sheet's order.
///
/// `file_path` holds the native separator, so `dir_path` comes in native form through
/// `canonicalize_path`.
pub async fn get_tracks_in_directory(
    db: &DbPool,
    dir_path: &str,
) -> Result<Vec<DirectoryTrack>, AppError> {
    let (from, before) = subtree_bounds(dir_path);
    db.read(move |conn| {
        sql::fetch_all(conn, TRACKS_IN_DIRECTORY.as_str(), (from, before, MAIN_SEPARATOR_STR))
    })
    .await
}

const ANY_TRACK_UNDER: &str =
    "SELECT EXISTS (SELECT 1 FROM tracks WHERE file_path >= ?1 AND file_path < ?2)";

/// Whether a track sits anywhere under one of `dirs`, subdirectories included.
pub async fn holds_tracks_under(db: &DbPool, dirs: Vec<String>) -> Result<bool, AppError> {
    db.read(move |conn| {
        for dir in &dirs {
            let (from, before) = subtree_bounds(dir);
            if sql::fetch_one(conn, ANY_TRACK_UNDER, (from, before))? {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
}

static RECENTLY_PLAYED: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {} FROM tracks \
         WHERE last_played IS NOT NULL AND {IN_LIBRARY} \
         ORDER BY last_played DESC, id DESC \
         LIMIT ?",
        columns::TRACK_LIST.joined()
    )
});

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
    db.read(move |conn| sql::fetch_all(conn, RECENTLY_PLAYED.as_str(), (limit,))).await
}
