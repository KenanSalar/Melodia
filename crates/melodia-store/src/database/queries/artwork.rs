use std::collections::HashSet;
use std::path::Path;

use sqlx::AssertSqlSafe;

use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, placeholders};
use melodia_core::error::AppError;

/// Every column that points into the artwork stores, as `(table, column)`.
///
/// **Six, and the last three are the ones that bite.** The first three name covers a rescan
/// rebuilds from the user's own files. `playlists.thumbnail_path` carries composites that
/// `compose_artwork` wrote and no other row names, so a union that omits it reads them as orphans
/// and the sweep blanks every custom playlist mosaic in the app; the auto-populated thumbnails
/// alias a track's cover and would survive by coincidence, which is not a property to rely on.
/// `radio_stations.artwork_path` is the same shape and worse, a station logo having come off a
/// third-party host that is often already dead, so nothing can re-derive it.
///
/// [`repoint_all`] builds its statements from this. [`REFERENCED_PATHS`] spells the same six out
/// by hand, a union reading better written than generated, and is pinned against this list.
pub(super) const ARTWORK_COLUMNS: [(&str, &str); 6] = [
    ("tracks", "artwork_path"),
    ("albums", "artwork_path"),
    ("artists", "image_path"),
    ("playlists", "thumbnail_path"),
    ("radio_stations", "artwork_path"),
    // The one entry that is not a row the user owns. A cached browse logo is held alive by its
    // cache row alone, so the sweep has to see it or the cache would name a file the sweep just
    // deleted; `library::radio`'s retention pass drops the row, and the file follows.
    ("radio_logo_answers", "artwork_path"),
];

/// The four arms of the reference set a library scan can re-derive, as one literal both unions
/// below are built from. Radio's two stay out: `library::radio` drops a logo whose file is gone and
/// fetches it again on its own.
macro_rules! library_reference_arms {
    () => {
        "\
        SELECT artwork_path FROM tracks WHERE artwork_path IS NOT NULL AND artwork_path <> '' \
        UNION \
        SELECT artwork_path FROM albums WHERE artwork_path IS NOT NULL AND artwork_path <> '' \
        UNION \
        SELECT image_path FROM artists WHERE image_path IS NOT NULL AND image_path <> '' \
        UNION \
        SELECT thumbnail_path FROM playlists \
        WHERE thumbnail_path IS NOT NULL AND thumbnail_path <> ''"
    };
}

/// The read half of [`ARTWORK_COLUMNS`].
///
/// `UNION` rather than `UNION ALL`: the tracks arm is one row per track, and deduplicating in
/// `SQLite` is cheaper than moving a large library's worth of repeated paths across the boundary.
/// The empty-string arm is not redundant — the schema leaves all six nullable and the ingest
/// paths write `''` as readily as `NULL`.
const REFERENCED_PATHS: &str = concat!(
    library_reference_arms!(),
    " UNION \
    SELECT artwork_path FROM radio_stations WHERE artwork_path IS NOT NULL AND artwork_path <> '' \
    UNION \
    SELECT artwork_path FROM radio_logo_answers \
    WHERE artwork_path IS NOT NULL AND artwork_path <> ''"
);

/// The library's share of [`REFERENCED_PATHS`], spelled through the same arms so a column added
/// to it cannot be missed by the sweep.
const LIBRARY_REFERENCED_PATHS: &str = library_reference_arms!();

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

/// The paths a library scan can put back if their file goes missing.
pub async fn referenced_library_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar(LIBRARY_REFERENCED_PATHS).fetch_all(db.read()).await?)
}

/// Clears every library reference to `missing`, returning rows touched.
///
/// A cleared reference is one the existing refills already answer: the album roll-up and the
/// artist-image fetch fill an empty column and leave a set one alone, which is why a path to a
/// deleted file stopped them; a playlist's automatic thumbnail follows its first track's cover.
/// A track also loses its `date_modified`, the lever `tasks::tag_backfill` pulls, so the
/// next scan reads it as changed and extracts its cover again. A custom playlist image has no
/// source to come back from, so the playlist falls back to its automatic thumbnail.
pub async fn forget_paths(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    missing: &[String],
) -> Result<u64, AppError> {
    const FORGETS: [(&str, &str, &str); 4] = [
        ("tracks", "artwork_path", "artwork_path = NULL, date_modified = NULL"),
        ("albums", "artwork_path", "artwork_path = NULL"),
        ("artists", "image_path", "image_path = NULL"),
        ("playlists", "thumbnail_path", "thumbnail_path = NULL, custom_thumbnail = FALSE"),
    ];

    let mut touched = 0;
    for chunk in missing.chunks(MAX_BINDS_PER_STATEMENT) {
        let list = placeholders(chunk.len());
        for (table, column, cleared) in FORGETS {
            let sql = format!("UPDATE {table} SET {cleared} WHERE {column} IN ({list})");
            let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false);
            for path in chunk {
                query = query.bind(path);
            }
            touched += query.execute(&mut **tx).await?.rows_affected();
        }
    }
    Ok(touched)
}

/// Re-points every artwork column across `moves`, returning rows touched.
///
/// One transaction for the whole pass, so a half-way interruption leaves no row pointing at a file
/// the sweep is about to retire — and one statement per column per chunk rather than one per
/// column per *file*, the write pool being single-connection and the caller running while a boot
/// scan is using it. `playlists.custom_thumbnail` is deliberately untouched: re-encoding a
/// thumbnail does not make it less the user's own choice.
pub async fn repoint_all(db: &DbPool, moves: &[(String, String)]) -> Result<u64, AppError> {
    /// The old and new path each row binds.
    const COLS_PER_ROW: usize = 2;

    if moves.is_empty() {
        return Ok(0);
    }

    let mut tx = db.write().begin().await?;
    let mut touched = 0;
    for chunk in moves.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let rows = std::iter::repeat_n("(?,?)", chunk.len()).collect::<Vec<_>>().join(",");
        for (table, column) in ARTWORK_COLUMNS {
            let sql = format!(
                "WITH m(old_path, new_path) AS (VALUES {rows})
                 UPDATE {table}
                    SET {column} = (SELECT new_path FROM m WHERE m.old_path = {table}.{column})
                  WHERE {column} IN (SELECT old_path FROM m)"
            );
            let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false);
            for (from, to) in chunk {
                query = query.bind(from).bind(to);
            }
            touched += query.execute(&mut *tx).await?.rows_affected();
        }
    }
    tx.commit().await?;
    Ok(touched)
}

#[cfg(test)]
#[path = "tests/artwork_tests.rs"]
mod tests;
