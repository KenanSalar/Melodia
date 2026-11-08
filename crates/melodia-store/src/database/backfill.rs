use rusqlite::Connection;

use melodia_core::error::AppError;

use super::DbPool;
use super::bulk::{CASE_BINDS_PER_ROW, MAX_BINDS_PER_STATEMENT, case_by_id, placeholders};
use super::rows::FromRow;
use super::sql::Sql;

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
