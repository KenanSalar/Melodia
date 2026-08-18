use std::collections::HashSet;
use std::path::Path;

use crate::database::DbPool;
use crate::error::AppError;

/// Every column that points into the artwork stores.
///
/// **Four, and the fourth is the one that bites.** `playlists.thumbnail_path` carries composites
/// that `compose_artwork` wrote and no other row names, so a three-column union reads them as
/// orphans and the sweep blanks every custom playlist mosaic in the app. The auto-populated
/// thumbnails alias a track's cover and would survive by coincidence, which is not a property to
/// rely on.
///
/// `UNION` rather than `UNION ALL`: the tracks arm is one row per track, and deduplicating in
/// `SQLite` is cheaper than moving a large library's worth of repeated paths across the boundary.
/// The empty-string arm is not redundant — the schema leaves all four nullable and the ingest
/// paths write `''` as readily as `NULL`.
const REFERENCED_PATHS: &str = "\
    SELECT artwork_path FROM tracks WHERE artwork_path IS NOT NULL AND artwork_path <> '' \
    UNION \
    SELECT artwork_path FROM albums WHERE artwork_path IS NOT NULL AND artwork_path <> '' \
    UNION \
    SELECT image_path FROM artists WHERE image_path IS NOT NULL AND image_path <> '' \
    UNION \
    SELECT thumbnail_path FROM playlists WHERE thumbnail_path IS NOT NULL AND thumbnail_path <> ''";

/// The bare filenames every artwork column still points at.
///
/// Reduced to basenames because that is what the sweep compares against a directory listing, and
/// because a row written before the data directory moved still names the file correctly.
pub async fn referenced_filenames(db: &DbPool) -> Result<HashSet<String>, AppError> {
    Ok(referenced_paths(db)
        .await?
        .iter()
        .filter_map(|path| Path::new(path).file_name()?.to_str().map(str::to_owned))
        .collect())
}

/// The same set as whole paths, for the renormalize pass — which has to open each file, so it
/// needs where the file is rather than only what it is called.
pub async fn referenced_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar(REFERENCED_PATHS).fetch_all(db.read()).await?)
}

/// The write side of [`REFERENCED_PATHS`], and it has the same four-column failure mode: a column
/// missing here leaves rows naming a file the renormalize pass has just orphaned, which the next
/// sweep then deletes. Same list, so a fifth column has to reach both.
const REPOINT_UPDATES: [&str; 4] = [
    "UPDATE tracks SET artwork_path = ? WHERE artwork_path = ?",
    "UPDATE albums SET artwork_path = ? WHERE artwork_path = ?",
    "UPDATE artists SET image_path = ? WHERE image_path = ?",
    "UPDATE playlists SET thumbnail_path = ? WHERE thumbnail_path = ?",
];

/// Re-points every artwork column from `old_path` to `new_path`, returning rows touched.
///
/// All four, since a playlist thumbnail is as re-pointable as a track's cover — and one
/// transaction, so a pass interrupted half way leaves no row pointing at a file the sweep is
/// about to retire. `playlists.custom_thumbnail` is deliberately untouched: re-encoding a
/// thumbnail does not make it less the user's own choice.
pub async fn repoint(db: &DbPool, old_path: &str, new_path: &str) -> Result<u64, AppError> {
    let mut tx = db.write().begin().await?;
    let mut touched = 0;
    for statement in REPOINT_UPDATES {
        let result = sqlx::query(statement).bind(new_path).bind(old_path).execute(&mut *tx).await?;
        touched += result.rows_affected();
    }
    tx.commit().await?;
    Ok(touched)
}

#[cfg(test)]
#[path = "tests/artwork_tests.rs"]
mod tests;
