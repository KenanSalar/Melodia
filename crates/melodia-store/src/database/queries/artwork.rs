use std::collections::HashSet;
use std::path::Path;

use sqlx::AssertSqlSafe;

use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, placeholders, queries, values_placeholders,
};
use melodia_core::error::AppError;

/// Every column that points into the artwork stores, as `(table, column)`.
///
/// **The last three are the ones that bite**, each reachable through no other row:
/// `playlists.thumbnail_path` carries the composites `compose_artwork` wrote, and a station logo
/// came off a third-party host nothing can re-derive it from. [`repoint_all`] builds its statements
/// from this, and [`REFERENCED_PATHS`] spells the same six out by hand, pinned against it.
pub(super) const ARTWORK_COLUMNS: [(&str, &str); 6] = [
    ("tracks", "artwork_path"),
    ("albums", "artwork_path"),
    ("artists", "image_path"),
    ("playlists", "thumbnail_path"),
    ("radio_stations", "artwork_path"),
    // Not a row the user owns: a browsed logo is held alive by its cache row alone.
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

/// The read half of [`ARTWORK_COLUMNS`]. `UNION`, so a library's repeated track covers are
/// deduplicated in `SQLite` rather than carried across; the `''` test because ingest writes it as
/// readily as `NULL`.
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

/// The bare filenames every artwork column points at, which the sweep compares against a listing;
/// a row written before the data directory moved still names its file.
pub async fn referenced_filenames(db: &DbPool) -> Result<HashSet<String>, AppError> {
    Ok(referenced_paths(db)
        .await?
        .iter()
        .filter_map(|path| Path::new(path).file_name()?.to_str().map(str::to_owned))
        .collect())
}

/// The same set as whole paths, for the renormalize pass, which opens each file.
pub async fn referenced_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar(REFERENCED_PATHS).fetch_all(db.read()).await?)
}

/// The paths a library scan can put back if their file goes missing.
pub async fn referenced_library_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query_scalar(LIBRARY_REFERENCED_PATHS).fetch_all(db.read()).await?)
}

/// Clears every library reference to `missing`, returning rows touched.
///
/// Each cleared column is one an existing refill answers: the album roll-up and the artist image
/// fill only an empty column, and a playlist falls back to its automatic thumbnail. A track also
/// loses its `date_modified`, `tasks::tag_backfill`'s lever, so the next scan re-extracts its
/// cover. The roll-ups run in the same transaction, so a cover a sibling track holds is back
/// before that scan starts.
pub async fn forget_paths(db: &DbPool, missing: &[String]) -> Result<u64, AppError> {
    const FORGETS: [(&str, &str, &str); 4] = [
        ("tracks", "artwork_path", "artwork_path = NULL, date_modified = NULL"),
        ("albums", "artwork_path", "artwork_path = NULL"),
        ("artists", "image_path", "image_path = NULL"),
        ("playlists", "thumbnail_path", "thumbnail_path = NULL, custom_thumbnail = FALSE"),
    ];

    let mut tx = db.write().begin().await?;
    let mut touched = 0;
    for chunk in missing.chunks(MAX_BINDS_PER_STATEMENT) {
        let list = placeholders(chunk.len());
        for (table, column, cleared) in FORGETS {
            let sql = format!("UPDATE {table} SET {cleared} WHERE {column} IN ({list})");
            let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false);
            for path in chunk {
                query = query.bind(path);
            }
            touched += query.execute(&mut *tx).await?.rows_affected();
        }
    }
    queries::scan::roll_up_covers(&mut tx).await?;
    tx.commit().await?;
    Ok(touched)
}

/// Re-points every artwork column across `moves`, returning rows touched.
///
/// One transaction, so an interruption leaves no row naming a file the sweep is about to retire,
/// and a statement per column per chunk, the caller sharing the one write connection with a boot
/// scan. `custom_thumbnail` stays: a re-encoded thumbnail is still the user's choice.
pub async fn repoint_all(db: &DbPool, moves: &[(String, String)]) -> Result<u64, AppError> {
    /// The old and new path each row binds.
    const COLS_PER_ROW: usize = 2;

    if moves.is_empty() {
        return Ok(0);
    }

    let mut tx = db.write().begin().await?;
    let mut touched = 0;
    for chunk in moves.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
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
