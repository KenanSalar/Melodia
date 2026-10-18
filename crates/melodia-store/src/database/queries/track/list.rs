//! The `TrackListRow` fetches every track list renders from.

use sqlx::AssertSqlSafe;

use super::columns;
use crate::database::DbPool;
use melodia_core::entities::browse::DirectoryTrack;
use melodia_core::entities::track;
use melodia_core::error::AppError;

/// The `ORDER BY` body a whole-table fetch shares: the natural-ordering key built at scan time from
/// title/artist/album.
///
/// **Not a page's order**, which is its keyset's, and copying it into a batch-action query is the
/// trap: `queries::track::entity_ids` owes agreement to the default of the page beside the card,
/// and Artist and Genre Detail default to `"album"`, so `sort_key` taken from here queues a
/// catalogue by title under a page that shows it by album.
///
/// What the clause is for is **determinism**: the in-memory comparator these rows are handed to
/// appends `sort_key` as its tie-breaker and `sort_by_cached_key` is stable, so rows tied on it
/// fall back to the order `SQLite` handed over. Dropping the clause would leave that to the query
/// plan, and it is close to free, `idx_tracks_sort_key` being the same expression under the same
/// collation, so this reads as an index scan rather than a sort.
pub(super) const TRACK_LIST_ORDER: &str = "sort_key COLLATE NOCASE ASC";

/// Every track as a list row, in [`TRACK_LIST_ORDER`].
///
/// No list reads the whole table: a library-sized one holds a `keyset` and fetches its rows by
/// id. What still wants every row at once is a check that holds an id query against them.
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

/// `dir_path` escaped for `LIKE … ESCAPE '\'` and terminated with [`LIKE_SEPARATOR`]: the prefix
/// every "files under this directory" pattern is built from.
///
/// One home for the escaping, shared with [`super::entity_ids::track_ids_under_directory`]. A
/// folder is named by the user, and `_` is `LIKE`'s single-character wildcard.
pub(super) fn directory_like_prefix(dir_path: &str) -> String {
    let escaped = dir_path.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("{escaped}{LIKE_SEPARATOR}")
}

/// Fetch tracks whose files live directly inside `dir_path`, not in subdirectories, each with
/// its path. Browse matches a row to its file on disk by that path, and has to: `LIKE` folds
/// ASCII case, so a sibling folder spelled in another case answers here too.
///
/// **Platform invariant**: `tracks.file_path` is stored verbatim from `Path::to_string_lossy()` at
/// scan time, so it carries the platform's native separator and the LIKE pattern below matches
/// that. Pass `dir_path` in native form too — callers route through
/// `melodia_core::utils::canonicalize_path`.
///
/// Query-plan note: `file_path` has a UNIQUE auto-index, but the `ESCAPE` clause disables
/// `SQLite`'s prefix-`LIKE`→range-scan optimisation entirely. At realistic library sizes a one-off
/// index scan per *navigation* isn't worth a generated `parent_dir` column and index, which would
/// slow every insert. Revisit only if a profiler flags it at a much larger library size.
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

/// The N most-recently-played tracks (list-view projection), newest first.
///
/// Ordering rides on `last_played` (RFC-3339 UTC, written by the play-count flusher /
/// `update_play_count`), whose lexical order is chronological even though `chrono`'s `AutoSi`
/// fractional seconds vary in width: the offset's `'+'` sorts below every digit, and `AutoSi` only
/// drops trailing-zero groups. Rows without a `last_played` (never played) are excluded —
/// index-backed by the partial `idx_tracks_last_played`.
///
/// Returns the `TrackListRow` projection so the Recently-Played view can render
/// through the shared `TrackList`; `last_played` itself is not selected (it is
/// only the sort key). Membership is the top `limit` by recency — the view
/// re-sorts/filters this set in memory, it never re-queries.
pub async fn get_recently_played(
    db: &DbPool,
    limit: i64,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = columns::TRACK_LIST.joined();
    let sql = format!(
        "SELECT {cols} FROM tracks \
         WHERE last_played IS NOT NULL \
         ORDER BY last_played DESC \
         LIMIT ?"
    );
    let rows = sqlx::query_as::<_, track::TrackListRow>(AssertSqlSafe(sql))
        .bind(limit)
        .fetch_all(db.read())
        .await?;
    Ok(rows)
}
