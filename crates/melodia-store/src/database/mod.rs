mod backup;
mod collation;
mod migrate;
mod pool;
pub mod queries;
mod rows;
mod sql;

use rusqlite::{Connection, Transaction};

use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub use pool::{DbPool, IDLE_CONNECTIONS_CLOSED_WITHIN};
use rows::FromRow;
use sql::{Param, Sql};

/// Bind-variable budget for one statement, divided by a query's binds per row to size its chunks.
///
/// Chosen rather than the engine's: the bundled `SQLite` allows 32766, and 999 is the older default
/// this kept. Moving it trades both ways, a larger chunk buying fewer prepares (every bulk statement
/// is assembled, so prepared per run) for a larger parameter array resident per statement, so move
/// it against a profile of a first scan on a large library.
pub const MAX_BINDS_PER_STATEMENT: usize = 999;

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

/// `rows` comma-separated tuples of `width` `?`s each, for a `VALUES` list.
pub(crate) fn values_placeholders(rows: usize, width: usize) -> String {
    values_list(&format!("({})", placeholders(width)), rows)
}

/// The `VALUES` tuple `row` repeated `rows` times, comma-separated.
pub(crate) fn values_list(row: &str, rows: usize) -> String {
    let mut s = String::with_capacity(rows * (row.len() + 2));
    for i in 0..rows {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(row);
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
#[derive(Clone, Copy)]
pub(crate) struct FoldedColumn {
    pub table: &'static str,
    pub column: &'static str,
    /// The SELECT reading what each row packs, its source aliased `x`.
    pub inputs: &'static str,
}

/// A row [`backfill_folded`] reads, and the value it packs from it.
pub(crate) trait FoldedInputs: FromRow + Send + 'static {
    fn id(&self) -> i64;
    fn packed(&self) -> String;
}

/// Rows per backfill page, so that one page is one `UPDATE` inside the bind budget.
const BACKFILL_PAGE_ROWS: usize = MAX_BINDS_PER_STATEMENT / CASE_BINDS_PER_ROW;

/// Fills `target` on every row still holding `''`, a page at a time, and returns how many it wrote.
///
/// Cheap on every launch, a filled table answering from an empty partial index. Each page is its
/// own write so a waiting writer waits for a page rather than the table, and the `UPDATE` asks
/// again that the column is empty, so a row a writer filled in between keeps the writer's value.
pub(crate) async fn backfill_folded<R: FoldedInputs>(
    db: &DbPool,
    target: FoldedColumn,
) -> Result<u64, AppError> {
    let page_limit = i64::try_from(BACKFILL_PAGE_ROWS).unwrap_or(i64::MAX);
    let page_select = format!(
        "{} WHERE x.{column} = '' AND x.id > ? ORDER BY x.id LIMIT ?",
        target.inputs,
        column = target.column,
    );
    let mut after_id = 0_i64;
    let mut written = 0;
    loop {
        let mut select = Sql::trusted(page_select.clone());
        select.bind(after_id).bind(page_limit);
        let page: Vec<R> = db.read(move |conn| select.fetch_all(conn)).await?;
        let Some(last) = page.last() else {
            return Ok(written);
        };
        after_id = last.id();
        written += db.write(move |conn| fill_folded_page(conn, target, &page)).await?;
    }
}

fn fill_folded_page<R: FoldedInputs>(
    conn: &Connection,
    target: FoldedColumn,
    page: &[R],
) -> Result<u64, AppError> {
    let mut update = Sql::trusted(format!(
        "UPDATE {table} SET {column} = {case} WHERE id IN ({ids}) AND {column} = ''",
        table = target.table,
        column = target.column,
        case = case_by_id(page.len()),
        ids = placeholders(page.len()),
    ));
    for row in page {
        update.bind(row.id()).bind(row.packed());
    }
    for row in page {
        update.bind(row.id());
    }
    update.execute(conn)
}

/// Runs a single-column `IN (…)` query in chunks inside [`MAX_BINDS_PER_STATEMENT`], concatenating
/// the results, on any connection, a transaction's included. One bind per item, so **not** for a
/// tuple-IN clause.
pub(crate) fn chunked_in_query<T, B>(
    conn: &Connection,
    items: &[B],
    build_sql: impl Fn(&str) -> String,
) -> Result<Vec<T>, AppError>
where
    T: FromRow,
    B: Clone + Into<Param>,
{
    let mut all_results: Vec<T> = Vec::new();
    for chunk in items.chunks(MAX_BINDS_PER_STATEMENT) {
        let mut sql = Sql::trusted(build_sql(&placeholders(chunk.len())));
        for item in chunk {
            sql.bind(item.clone());
        }
        all_results.extend(sql.fetch_all::<T>(conn)?);
    }
    Ok(all_results)
}

/// [`chunked_in_query`] for a statement that returns no rows, answering how many it touched. It
/// takes a transaction, so a write spanning chunks lands whole.
pub(crate) fn chunked_in_execute_on<B>(
    tx: &Transaction<'_>,
    items: &[B],
    build_sql: impl Fn(&str) -> String,
) -> Result<u64, AppError>
where
    B: Clone + Into<Param>,
{
    let mut touched = 0;
    for chunk in items.chunks(MAX_BINDS_PER_STATEMENT) {
        let mut sql = Sql::trusted(build_sql(&placeholders(chunk.len())));
        for item in chunk {
            sql.bind(item.clone());
        }
        touched += sql.execute(tx)?;
    }
    Ok(touched)
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
pub(crate) fn select_in_id_order<T: FromRow>(
    conn: &Connection,
    select: &str,
    ids: &[i64],
    id_of: impl Fn(&T) -> i64,
) -> Result<Vec<T>, AppError> {
    let rows = chunked_in_query(conn, ids, |placeholders| {
        format!("{select} WHERE id IN ({placeholders})")
    })?;
    Ok(in_id_order(rows, ids, id_of))
}

/// Ids per statement that binds one value ahead of its id list, as [`set_where_id_in`]'s does.
pub(crate) const IDS_PER_SET: usize = MAX_BINDS_PER_STATEMENT - 1;

/// One chunk of an `UPDATE … SET col = ? WHERE id IN (…)`, `value` bound first. The caller chunks
/// at [`IDS_PER_SET`] and picks the connection, some holding a transaction.
pub(crate) fn set_where_id_in(
    update_set: &'static str,
    value: impl Into<Param>,
    ids: &[i64],
) -> Sql {
    let mut sql = Sql::new(update_set);
    sql.bind(value).push(" WHERE id IN (").push_trusted(&placeholders(ids.len())).push(")");
    for id in ids {
        sql.bind(*id);
    }
    sql
}

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

    // Fatal on failure: a migration that runs without a recovery point is worse
    // than a boot that stops and says why.
    let backup_path = if !pending.is_empty() && db_existed {
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

    // Existing rows only, new writes going through `canonicalize_path`: Browse's path-keyed map
    // has to match `read_dir`.
    if cfg!(target_os = "windows") {
        db.transaction(strip_windows_verbatim_paths).await?;
    }

    log::info!("Database initialized successfully (read pool: {read_conns} connections)");

    Ok(db)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
