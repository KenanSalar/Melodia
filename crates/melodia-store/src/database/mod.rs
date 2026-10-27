mod backup;
mod collation;
pub mod queries;

use sqlx::AssertSqlSafe;
use sqlx::migrate::Migrate;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};

/// Bind-variable budget for one statement, divided by a query's binds per row to size its chunks.
///
/// Chosen rather than the engine's: the bundled `SQLite` allows 32766, and 999 is the older default
/// this kept. Moving it trades both ways, a larger chunk buying fewer prepares (every bulk site is
/// `.persistent(false)`) for a larger parameter array resident per statement, so move it against a
/// profile of a first scan on a large library.
pub const MAX_BINDS_PER_STATEMENT: usize = 999;

/// sqlx's default, which the `Migrate` methods take explicitly.
const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// The fts5 compaction [`DbPool::close`] issues, named so the test proving it still runs binds the
/// same string: fts5 rejects an unknown command only at step time, and `close` can only log that.
///
/// The full collapse rather than the bounded `'merge'`: `automerge` already folds segments as
/// writes accumulate, so what reaches shutdown is the tail, and collapsing it leaves the next call a
/// no-op where a page budget only spreads the work across more shutdowns.
const FTS_OPTIMIZE: &str = "INSERT INTO tracks_fts(tracks_fts) VALUES('optimize')";

/// `n` comma-separated `?`s for an `IN (…)` clause.
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

/// What one row spends in an `UPDATE … SET col = CASE id WHEN ? THEN ? … END WHERE id IN (…)`: the
/// `WHEN ? THEN ?` pair, then its id again in the `IN` list.
pub(crate) const CASE_BINDS_PER_ROW: usize = 3;

/// `CASE id WHEN ? THEN ? … END` over `rows` pairs, each bound id first.
pub(crate) fn case_by_id(rows: usize) -> String {
    const WHEN_THEN: &str = " WHEN ? THEN ?";
    let mut s = String::with_capacity("CASE id END".len() + rows * WHEN_THEN.len());
    s.push_str("CASE id");
    for _ in 0..rows {
        s.push_str(WHEN_THEN);
    }
    s.push_str(" END");
    s
}

/// A folded copy of other columns, filled at write time, that a per-launch pass fills on the rows
/// written before it existed.
pub(crate) struct FoldedColumn {
    pub table: &'static str,
    pub column: &'static str,
    /// The SELECT reading what each row packs, its source aliased `x`.
    pub inputs: &'static str,
}

/// A row [`backfill_folded`] reads, and the value it packs from it.
pub(crate) trait FoldedInputs:
    for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin
{
    fn id(&self) -> i64;
    fn packed(&self) -> String;
}

/// Rows per backfill page, so that one page is one `UPDATE` inside the bind budget.
const BACKFILL_PAGE_ROWS: usize = MAX_BINDS_PER_STATEMENT / CASE_BINDS_PER_ROW;

/// Fills `target` on every row still holding `''`, a page at a time, and returns how many it wrote.
///
/// Cheap on every launch, a filled table answering from an empty partial index. Each page is its
/// own statement so a waiting writer waits for a page rather than the table, and the `UPDATE` asks
/// again that the column is empty, so a row a writer filled in between keeps the writer's value.
pub(crate) async fn backfill_folded<R: FoldedInputs>(
    db: &DbPool,
    target: &FoldedColumn,
) -> Result<u64, AppError> {
    let page_limit = i64::try_from(BACKFILL_PAGE_ROWS).unwrap_or(i64::MAX);
    let sql = format!(
        "{} WHERE x.{column} = '' AND x.id > ? ORDER BY x.id LIMIT ?",
        target.inputs,
        column = target.column,
    );
    let mut after_id = 0_i64;
    let mut written = 0;
    loop {
        let page: Vec<R> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
            .bind(after_id)
            .bind(page_limit)
            .fetch_all(db.read())
            .await?;
        let Some(last) = page.last() else {
            return Ok(written);
        };
        after_id = last.id();
        written += fill_folded_page(db, target, &page).await?;
    }
}

async fn fill_folded_page<R: FoldedInputs>(
    db: &DbPool,
    target: &FoldedColumn,
    page: &[R],
) -> Result<u64, AppError> {
    let sql = format!(
        "UPDATE {table} SET {column} = {case} WHERE id IN ({ids}) AND {column} = ''",
        table = target.table,
        column = target.column,
        case = case_by_id(page.len()),
        ids = placeholders(page.len()),
    );
    let mut query = sqlx::query(AssertSqlSafe(sql));
    for row in page {
        query = query.bind(row.id()).bind(row.packed());
    }
    for row in page {
        query = query.bind(row.id());
    }
    Ok(query.persistent(false).execute(db.write()).await?.rows_affected())
}

/// Runs a single-column `IN (…)` query in chunks inside [`MAX_BINDS_PER_STATEMENT`], concatenating
/// the results. One bind per item, so **not** for a tuple-IN clause.
pub(crate) async fn chunked_in_query<T, B>(
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

    for chunk in items.chunks(MAX_BINDS_PER_STATEMENT) {
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

/// `rows`, fetched by an `IN (…)` over `ids`, put back in the order `ids` asked for. A row the
/// fetch didn't return, deleted since the ids were read, is skipped, and so is a second ask for
/// one id.
pub(crate) fn in_id_order<T>(rows: Vec<T>, ids: &[i64], id_of: impl Fn(&T) -> i64) -> Vec<T> {
    let mut by_id: std::collections::HashMap<i64, T> =
        std::collections::HashMap::with_capacity(rows.len());
    by_id.extend(rows.into_iter().map(|row| (id_of(&row), row)));
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

/// The rows `select` reads for `ids`, in the order `ids` asked for ([`in_id_order`]). `select` is
/// the statement up to its `WHERE`, over a source with an `id` column.
pub(crate) async fn select_in_id_order<T>(
    db: &DbPool,
    select: &str,
    ids: &[i64],
    id_of: impl Fn(&T) -> i64,
) -> Result<Vec<T>, AppError>
where
    T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> + Send + Unpin,
{
    let rows = chunked_in_query(db.read(), ids, |placeholders| {
        format!("{select} WHERE id IN ({placeholders})")
    })
    .await?;
    Ok(in_id_order(rows, ids, id_of))
}

/// Ids per [`set_where_id_in`] statement, the value it sets taking one bind of the budget.
pub(crate) const IDS_PER_SET: usize = MAX_BINDS_PER_STATEMENT - 1;

/// One chunk of an `UPDATE … SET col = ? WHERE id IN (…)`, `value` bound first. `update_set` is
/// `'static` because it is spliced in rather than bound; the caller chunks at [`IDS_PER_SET`] and
/// picks the executor, some holding a transaction.
pub(crate) fn set_where_id_in<'v, V>(
    update_set: &'static str,
    value: V,
    ids: &[i64],
) -> sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>
where
    V: sqlx::Encode<'v, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    let sql = format!("{update_set} WHERE id IN ({})", placeholders(ids.len()));
    let mut query = sqlx::query(AssertSqlSafe(sql)).persistent(false).bind(value);
    for id in ids {
        query = query.bind(*id);
    }
    query
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

    /// Refreshes planner statistics, compacts the search index and closes both pools, at shutdown.
    ///
    /// `PRAGMA optimize` updates `sqlite_stat1`; [`FTS_OPTIMIZE`] collapses the index's segments
    /// and drops the tombstone every track delete leaves, a smaller index for each pre-migration
    /// `VACUUM INTO` to copy. The merge is what can outgrow `flush_tasks_and_db`'s budget, and an
    /// unfinished one is rolled back on the next open, the index correct and merely un-compacted.
    /// Both **log** rather than discard, a mistyped fts5 command having no other symptom.
    pub async fn close(&self) {
        if let Err(e) = sqlx::query("PRAGMA optimize").execute(&self.write).await {
            log::warn!("db close: PRAGMA optimize: {}", describe(&e));
        }
        if let Err(e) = sqlx::query(FTS_OPTIMIZE).execute(&self.write).await {
            log::warn!("db close: fts5 optimize: {}", describe(&e));
        }
        self.write.close().await;
        self.read.close().await;
    }
}

/// Strips the Windows verbatim prefixes older versions stored by calling `std::fs::canonicalize`,
/// in one transaction; a second boot matches nothing. `\\?\<drive>:\…` loses 4 characters and
/// `\\?\UNC\…` swaps 8 for `\\`, each only where the result fits `MAX_PATH`, since a longer path
/// keeps the prefix under `dunce::canonicalize` too.
///
/// Compiled everywhere so every runner tests it, and called behind `cfg!` so Linux and macOS boots
/// skip two full-table `LIKE` scans that can't match.
async fn strip_windows_verbatim_paths(pool: &SqlitePool) -> Result<(), AppError> {
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

/// The map every connection reads through, so a library-sized read stays in the file-backed page
/// cache instead of being copied into that connection's heap.
const MMAP_SIZE: &str = "268435456";

/// How long a pooled connection sits idle before it may close, taking its page cache with it.
/// sqlx's reaper checks once per this period, so one can stay open up to twice as long, which
/// `scripts/measure-footprint.py` sizes both its settles past.
const IDLE_TIMEOUT: Duration = Duration::from_mins(1);

/// By when the pool has closed a connection left idle since launch: [`IDLE_TIMEOUT`] twice, the
/// reaper's first check finding it not yet idle long enough.
pub const IDLE_CONNECTIONS_CLOSED_WITHIN: Duration =
    Duration::from_secs(IDLE_TIMEOUT.as_secs() * 2);

pub async fn init_database(paths: &Paths) -> Result<DbPool, AppError> {
    let db_path = paths.db_path.clone();
    let db_url = format!("sqlite:{}", db_path.display());

    log::info!("Database path: {}", db_path.display());

    // Before the pool's `create_if_missing` makes it true, or a first launch backs up an empty
    // schema.
    let db_existed = db_path.exists();

    // `busy_timeout` lets the one writer wait out the WAL checkpointer rather than fail with
    // `SQLITE_BUSY`. DEFERRED rather than `sqlx.md`'s `BEGIN IMMEDIATE`: with one write connection
    // and no second process there is no contention to fail fast on.
    let write_opts = collation::register(SqliteConnectOptions::from_str(&db_url)?)
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON")
        .pragma("synchronous", "NORMAL")
        // Small in both pools: a desktop library's working set fits, and `mmap_size` keeps cold
        // pages file-backed rather than in each connection's heap.
        .pragma("cache_size", "-16000")
        .pragma("temp_store", "MEMORY")
        // The writer reads whole tables too: a bulk scan's stats recalc walks every track.
        .pragma("mmap_size", MMAP_SIZE);

    // Reaped like the read pool, so the pages a scan's writes leave cached don't outlive it.
    let write_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .idle_timeout(IDLE_TIMEOUT)
        .connect_with(write_opts)
        .await?;

    let migrator = sqlx::migrate!("../../migrations");
    let (has_pending, applied_version) = {
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

        // The scope releases the single write connection.
        (pending, applied.iter().max().copied().unwrap_or(0))
    };

    // Fatal on failure: a migration that runs without a recovery point is worse
    // than a boot that stops and says why.
    let backup_path = if has_pending && db_existed {
        Some(backup::create(&write_pool, &paths.backups_dir, applied_version).await?)
    } else {
        None
    };
    backup::maintain(&paths.data_dir, &paths.backups_dir);

    if let Err(e) = migrator.run(&write_pool).await {
        if let Some(path) = &backup_path {
            log::error!(
                "Migration failed — the database as it stood before is at {}",
                path.display()
            );
        }
        return Err(e.into());
    }

    // Existing rows only, new writes going through `canonicalize_path`: Browse's path-keyed map
    // has to match `read_dir`.
    if cfg!(target_os = "windows") {
        strip_windows_verbatim_paths(&write_pool).await?;
    }

    // A small fixed band rather than a connection per core: reads here are tiny and effectively
    // sequential, and more connections only multiply page-cache and statement memory.
    let read_conns = std::thread::available_parallelism()
        .map_or(4, |n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .clamp(2, 4);

    let read_opts = collation::register(SqliteConnectOptions::from_str(&db_url)?)
        .journal_mode(SqliteJournalMode::Wal)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("foreign_keys", "ON")
        .pragma("cache_size", "-16000")
        .pragma("temp_store", "MEMORY")
        .pragma("mmap_size", MMAP_SIZE);

    // Reaps what the boot's prefetch burst opens; a cold reopen costs nothing at this scale.
    let read_pool = SqlitePoolOptions::new()
        .max_connections(read_conns)
        .idle_timeout(IDLE_TIMEOUT)
        .connect_with(read_opts)
        .await?;

    log::info!("Database initialized successfully (read pool: {read_conns} connections)");

    Ok(DbPool { read: read_pool, write: write_pool })
}

#[doc(hidden)]
impl DbPool {
    /// An in-memory `DbPool` for tests, read and write sharing the one connection an in-memory
    /// database lives in.
    pub async fn test_pool() -> Result<Self, AppError> {
        let opts = collation::register(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .pragma("foreign_keys", "ON")
            .pragma("synchronous", "NORMAL")
            .pragma("temp_store", "MEMORY");

        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;

        sqlx::migrate!("../../migrations").run(&pool).await?;

        Ok(DbPool { read: pool.clone(), write: pool })
    }
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
