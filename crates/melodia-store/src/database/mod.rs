mod backfill;
mod backup;
mod bulk;
mod collation;
mod migrate;
mod pool;
pub mod queries;
mod rows;
mod sql;

use rusqlite::Transaction;

use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};

pub(crate) use backfill::{FoldedColumn, FoldedInputs, backfill_folded};
pub(crate) use bulk::{
    CASE_BINDS_PER_ROW, MAX_BINDS_PER_STATEMENT, case_by_id, chunked_in_execute, chunked_in_query,
    chunked_in_statements, in_id_order, placeholders, select_in_id_order, values_list,
    values_placeholders,
};
pub use pool::{DbPool, IDLE_CONNECTIONS_CLOSED_WITHIN};

/// Strips the Windows verbatim prefixes older versions stored by calling `std::fs::canonicalize`;
/// a second boot matches nothing. `\\?\<drive>:\…` loses 4 characters and `\\?\UNC\…` swaps 8 for
/// `\\`, each only where the result fits `MAX_PATH`, since a longer path keeps the prefix under
/// `dunce::canonicalize` too.
///
/// Compiled everywhere so every runner tests it, and called behind `cfg!` so Linux and macOS boots
/// skip two full-table `LIKE` scans that can't match.
fn strip_windows_verbatim_paths(tx: &Transaction<'_>) -> Result<(), AppError> {
    // `_` is the drive letter, and the `:` after it keeps a UNC path (`N` there) out. The bound is
    // `MAX_PATH` plus the 4 stripped.
    const FOLDERS_DRIVE: &str = "\
        UPDATE folders SET path = SUBSTR(path, 5) \
        WHERE path LIKE '\\\\?\\_:\\%' AND LENGTH(path) < 264";
    const TRACKS_DRIVE: &str = "\
        UPDATE tracks SET file_path = SUBSTR(file_path, 5) \
        WHERE file_path LIKE '\\\\?\\_:\\%' AND LENGTH(file_path) < 264";
    // `MAX_PATH` plus the net 6 removed.
    const FOLDERS_UNC: &str = "\
        UPDATE folders SET path = '\\\\' || SUBSTR(path, 9) \
        WHERE path LIKE '\\\\?\\UNC\\%' AND LENGTH(path) < 266";
    const TRACKS_UNC: &str = "\
        UPDATE tracks SET file_path = '\\\\' || SUBSTR(file_path, 9) \
        WHERE file_path LIKE '\\\\?\\UNC\\%' AND LENGTH(file_path) < 266";

    let folders_drive = sql::execute(tx, FOLDERS_DRIVE, ())?;
    let folders_unc = sql::execute(tx, FOLDERS_UNC, ())?;
    let tracks_drive = sql::execute(tx, TRACKS_DRIVE, ())?;
    let tracks_unc = sql::execute(tx, TRACKS_UNC, ())?;

    let folders = folders_drive + folders_unc;
    let tracks = tracks_drive + tracks_unc;
    if folders > 0 || tracks > 0 {
        log::info!(
            "Normalized Windows verbatim paths: {folders} folder row(s) ({folders_drive} drive, {folders_unc} UNC), {tracks} track row(s) ({tracks_drive} drive, {tracks_unc} UNC)"
        );
    }
    Ok(())
}

pub async fn init_database(paths: &Paths) -> Result<DbPool, AppError> {
    let db_path = &paths.db_path;
    log::info!("Database path: {}", db_path.display());

    // Before the writer's first open creates the file, or a first launch backs up an empty schema.
    let db_existed = db_path.exists();

    // A small fixed band rather than a connection per core: reads here are tiny and effectively
    // sequential, and more connections only multiply page-cache and statement memory.
    let read_conns = std::thread::available_parallelism()
        .map_or(4, |n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .clamp(2, 4);
    let db = DbPool::open(db_path, read_conns)?;

    let pending = db.write(|conn| migrate::pending(conn)).await?;
    let migrating = !pending.is_empty();

    // Fatal on failure: a migration that runs without a recovery point is worse
    // than a boot that stops and says why.
    let backup_path = if migrating && db_existed {
        Some(backup::create(&db, &paths.backups_dir, pending.applied_version()).await?)
    } else {
        None
    };
    backup::maintain(&paths.data_dir, &paths.backups_dir);

    let migrated = db.write(move |conn| migrate::apply(conn, pending)).await;
    if let Err(e) = migrated {
        if let Some(path) = &backup_path {
            log::error!(
                "Migration failed — the database as it stood before is at {}",
                path.display()
            );
        }
        return Err(e);
    }

    let filled = queries::track::order_keys::fill_missing(&db).await?;
    if filled > 0 {
        log::info!("Filled the order keys of {filled} tracks");
    }

    // Existing rows only, new writes going through `canonicalize_path`: Browse's path-keyed map
    // has to match `read_dir`.
    if cfg!(target_os = "windows") {
        db.transaction(strip_windows_verbatim_paths).await?;
    }

    if migrating {
        let started = std::time::Instant::now();
        match db.compact().await {
            Ok(()) => log::info!("Compacted the database in {} ms", started.elapsed().as_millis()),
            Err(e) => log::warn!("Compacting the database after migrating: {}", describe(&e)),
        }
    }

    log::info!("Database initialized successfully (read pool: {read_conns} connections)");

    Ok(db)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
