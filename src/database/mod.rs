pub mod queries;

use sqlx::AssertSqlSafe;
use sqlx::migrate::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

use crate::config::Paths;
use crate::error::AppError;

/// `SQLite` bind variable limit — queries with more placeholders will fail.
pub const SQLITE_BIND_LIMIT: usize = 999;

/// sqlx's default migrations-tracking table; the 0.9 `Migrate` trait methods
/// take it explicitly (it became configurable via `sqlx.toml`).
const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// Build a `?, ?, …` placeholder list for an `IN (...)` clause. Single-pass,
/// capacity-preallocated — replaces the previous `repeat_n("?", n).collect::<Vec<_>>().join(", ")`
/// idiom that allocated an intermediate `Vec<&str>` of size `n` before joining.
pub(crate) fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n * 3);
    for i in 0..n {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('?');
    }
    s
}

/// Execute a chunked single-column IN-clause query, staying within `SQLite`'s bind limit.
/// Returns all results concatenated across chunks.
///
/// Each item binds exactly one placeholder (`IN (?, ?, …)`). Do not use this for
/// tuple-IN clauses (`WHERE (a,b) IN ((?,?), …)`) — the chunk size assumes one
/// bind per item and would bust the 999-bind cap.
pub async fn chunked_in_query<T, B>(
    pool: &SqlitePool,
    items: &[B],
    build_sql: impl Fn(&str) -> String,
) -> Result<Vec<T>, AppError>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
    for<'q> B: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Sync,
{
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let mut all_results: Vec<T> = Vec::new();

    for chunk in items.chunks(SQLITE_BIND_LIMIT) {
        let sql = build_sql(&placeholders(chunk.len()));

        let mut query = sqlx::query_as::<_, T>(AssertSqlSafe(sql));
        for item in chunk {
            query = query.bind(item);
        }

        let rows = query.persistent(false).fetch_all(pool).await?;
        all_results.extend(rows);
    }

    Ok(all_results)
}

#[derive(Clone)]
pub struct DbPool {
    read: SqlitePool,
    write: SqlitePool,
}

impl DbPool {
    pub fn read(&self) -> &SqlitePool {
        &self.read
    }

    pub fn write(&self) -> &SqlitePool {
        &self.write
    }

    /// Run `PRAGMA optimize` and close both pools. Call on application shutdown.
    pub async fn close(&self) {
        let _ = sqlx::query("PRAGMA optimize")
            .execute(&self.write)
            .await;
        self.write.close().await;
        self.read.close().await;
    }
}

/// Create a WAL-safe backup via `VACUUM INTO` before applying migrations.
/// Produces a self-contained copy with all WAL data merged in.
/// Skips if the database file doesn't exist (first launch).
///
/// If a backup with this name already exists (e.g. the previous launch
/// hit a failed migration and the file is left behind from that
/// attempt), it's removed first. `VACUUM INTO` refuses to write to an
/// existing path, and the stale file is equivalent to the new one
/// anyway — a failed migration is rolled back so the DB on disk is at
/// the same applied-version it was at the prior backup.
async fn backup_database(
    pool: &SqlitePool,
    db_path: &std::path::Path,
    backup_ext: &str,
) -> Result<(), AppError> {
    if !db_path.exists() {
        return Ok(());
    }
    let backup_path = db_path.with_extension(backup_ext);
    if backup_path.exists() {
        std::fs::remove_file(&backup_path).map_err(|e| {
            AppError::io_other(format!(
                "Failed to remove stale backup at {}: {}",
                backup_path.display(),
                e
            ))
        })?;
        log::info!("Removed stale backup at {}", backup_path.display());
    }
    let safe_path = backup_path.display().to_string().replace('\'', "''");
    let sql = format!("VACUUM INTO '{safe_path}'");
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(pool).await?;
    log::info!("Database backup created at {}", backup_path.display());
    Ok(())
}

/// One-shot normalization of Windows verbatim/extended-length path strings
/// in `folders.path` and `tracks.file_path`, for libraries that were
/// scanned by older versions which called `std::fs::canonicalize` directly
/// (producing `\\?\C:\...` for drive paths and `\\?\UNC\server\share\...`
/// for network shares). Runs immediately after migrations so the rest of
/// init sees the canonical form.
///
/// Two forms, two transformations:
///   * `\\?\<drive>:\…`            → `<drive>:\…`              (drop 4-char prefix)
///   * `\\?\UNC\server\share\…`    → `\\server\share\…`        (replace 8-char `\\?\UNC\` with `\\`)
///
/// Both UPDATEs gate on `LENGTH(path) < 260` (the post-strip length must
/// fit in `MAX_PATH`); paths that genuinely exceed `MAX_PATH` keep the
/// prefix because `dunce::canonicalize` would also keep it. Wrapped in a
/// single transaction. Idempotent: a second boot matches zero rows.
///
/// The function itself is not `#[cfg(windows)]`, so it stays compiled and
/// unit-testable on every CI runner; its *call site* in [`init_database`] is
/// guarded by a runtime `cfg!(target_os = "windows")` instead. That keeps the
/// two full-table `tracks` LIKE scans (the `\\?\…` patterns can never match
/// off-Windows, where paths don't start with `\\?\`) out of every Linux/macOS
/// boot — the default case-insensitive LIKE collation can't use the
/// `file_path` index, so each scan is a full table walk.
async fn strip_windows_verbatim_paths(pool: &SqlitePool) -> Result<(), AppError> {
    // `_` in LIKE matches exactly one char (the drive letter); the literal
    // `:` after it prevents this pattern from ever matching a UNC path
    // (`\\?\UNC\…` has `N` at that position). Post-strip length cap = 259
    // chars, so original LENGTH(path) < 264 (== 259 + 4-char prefix + 1).
    const FOLDERS_DRIVE: &str = "\
        UPDATE folders SET path = SUBSTR(path, 5) \
        WHERE path LIKE '\\\\?\\_:\\%' AND LENGTH(path) < 264";
    const TRACKS_DRIVE: &str = "\
        UPDATE tracks SET file_path = SUBSTR(file_path, 5) \
        WHERE file_path LIKE '\\\\?\\_:\\%' AND LENGTH(file_path) < 264";
    // `\\?\UNC\` is 8 chars; SUBSTR(path, 9) yields `server\share\…`,
    // prepend `\\` to get the standard UNC form `\\server\share\…`.
    // Net length change is -6, so original LENGTH(path) < 266 (== 259 + 6 + 1).
    const FOLDERS_UNC: &str = "\
        UPDATE folders SET path = '\\\\' || SUBSTR(path, 9) \
        WHERE path LIKE '\\\\?\\UNC\\%' AND LENGTH(path) < 266";
    const TRACKS_UNC: &str = "\
        UPDATE tracks SET file_path = '\\\\' || SUBSTR(file_path, 9) \
        WHERE file_path LIKE '\\\\?\\UNC\\%' AND LENGTH(file_path) < 266";

    let mut tx = pool.begin().await?;
    let folders_drive = sqlx::query(FOLDERS_DRIVE).execute(&mut *tx).await?.rows_affected();
    let folders_unc = sqlx::query(FOLDERS_UNC).execute(&mut *tx).await?.rows_affected();
    let tracks_drive = sqlx::query(TRACKS_DRIVE).execute(&mut *tx).await?.rows_affected();
    let tracks_unc = sqlx::query(TRACKS_UNC).execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;

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
    let db_path = paths.db_path.clone();
    let db_url = format!("sqlite:{}", db_path.display());

    log::info!("Database path: {}", db_path.display());

    // Write pool: single connection for serialized writes.
    // `busy_timeout` lets a writer wait briefly for the WAL checkpointer or a
    // read connection's brief lock instead of returning SQLITE_BUSY immediately.
    //
    // Transaction mode audit (§1.6): the rule of thumb in
    // `.claude/rules/sqlx.md` is "use BEGIN IMMEDIATE for write transactions
    // to fail fast on contention." With this architecture — a single-
    // -connection write pool and WAL readers that don't block writers — the
    // only contention source would be an external process opening the same
    // SQLite file, which doesn't happen for Melodia. SQLx 0.9 does expose
    // `Pool::begin_with("BEGIN IMMEDIATE")`, but with a single write
    // connection there is no intra-process writer contention for it to
    // fail fast on. DEFERRED + `busy_timeout(5s)` therefore stays — revisit
    // if a future second writer (sidecar tool, multi-window mode, etc.) gets
    // added.
    let write_opts = SqliteConnectOptions::from_str(&db_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON")
        .pragma("synchronous", "NORMAL")
        // Page cache sized for a single-user desktop library DB, not a
        // server. The working set fits comfortably; oversizing the cache
        // only inflates idle resident memory. (Negative value = KiB.)
        .pragma("cache_size", "-16000")
        .pragma("temp_store", "MEMORY");

    let write_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(write_opts)
        .await?;

    // Check for pending migrations and backup before applying
    let migrator = sqlx::migrate!("./migrations");
    let (has_pending, backup_ext) = {
        let mut conn = write_pool.acquire().await?;
        conn.ensure_migrations_table(MIGRATIONS_TABLE).await?;
        let applied: std::collections::HashSet<i64> = conn
            .list_applied_migrations(MIGRATIONS_TABLE)
            .await?
            .into_iter()
            .map(|m| m.version)
            .collect();

        let pending = migrator
            .iter()
            .filter(|m| !m.migration_type.is_down_migration())
            .any(|m| !applied.contains(&m.version));

        let ext = match applied.iter().max() {
            None => "db.pre-migration-backup".to_owned(),
            Some(max_version) => format!("db.backup-v{max_version}"),
        };
        // conn dropped here — releases the single write connection
        (pending, ext)
    };

    if has_pending {
        backup_database(&write_pool, &db_path, &backup_ext).await?;
    }

    migrator.run(&write_pool).await?;

    // One-shot repair for Windows verbatim/extended-length path prefixes
    // (`\\?\C:\…` for drive paths, `\\?\UNC\server\share\…` for shares)
    // stored by older versions that used `std::fs::canonicalize` directly.
    // New writes route through `crate::utils::canonicalize_path` (→
    // `dunce::canonicalize` on Windows), which never produces the prefix for
    // `MAX_PATH`-fitting paths; this brings existing rows in line so the
    // Browse view's path-keyed HashMap matches `read_dir` output. Idempotent
    // — the `WHERE` filters match zero rows on subsequent boots.
    //
    // Gated to Windows: off-Windows the patterns can never match, so the two
    // `tracks` UPDATEs would just full-scan the table (LIKE can't use the
    // index) to touch zero rows. `cfg!` const-folds the guard, leaving the fn
    // compiled + unit-testable everywhere.
    if cfg!(target_os = "windows") {
        strip_windows_verbatim_paths(&write_pool).await?;
    }

    // Read pool: a small, fixed band of connections for concurrent reads.
    // Melodia is single-user and its reads are tiny and effectively
    // sequential, so a handful of connections is ample. Clamp to 2..=4
    // rather than scaling with core count: one connection per core (the old
    // `.max(4)` floor with no ceiling) needlessly multiplies per-connection
    // page-cache and prepared-statement memory on many-core machines for
    // concurrency this workload never uses. (`.claude/rules/sqlx.md` advises
    // a num_cpus read pool — that is server-workload guidance; a desktop
    // app is a different profile.)
    let read_conns = std::thread::available_parallelism()
        .map_or(4, |n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .clamp(2, 4);

    let read_opts = SqliteConnectOptions::from_str(&db_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON")
        // Modest per-connection page cache: with `mmap_size` below, cold
        // pages stay cheaply file-backed (shared) rather than being copied
        // into per-connection heap cache, so a large cache here would just
        // be idle resident overhead multiplied across the pool.
        .pragma("cache_size", "-16000")
        .pragma("temp_store", "MEMORY")
        .pragma("mmap_size", "268435456");

    // `idle_timeout` reaps connections opened during the concurrent boot
    // prefetch burst once they go idle; without it sqlx keeps them for the
    // process lifetime. `min_connections` stays at its default (0) — the
    // cold-reopen cost after idle is negligible for this workload.
    let read_pool = SqlitePoolOptions::new()
        .max_connections(read_conns)
        .idle_timeout(Duration::from_mins(1))
        .connect_with(read_opts)
        .await?;

    log::info!("Database initialized successfully (read pool: {read_conns} connections)");

    Ok(DbPool {
        read: read_pool,
        write: write_pool,
    })
}

#[doc(hidden)]
impl DbPool {
    /// Create an in-memory `DbPool` for tests. Both read and write share the same
    /// single-connection pool (in-memory `SQLite` is per-connection).
    #[allow(clippy::unwrap_used)] // test helper; failure here aborts the test run by design
    pub async fn test_pool() -> Self {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .pragma("foreign_keys", "ON")
            .pragma("synchronous", "NORMAL")
            .pragma("temp_store", "MEMORY");

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .unwrap();

        DbPool {
            read: pool.clone(),
            write: pool,
        }
    }
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
