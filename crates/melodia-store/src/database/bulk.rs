//! Statements over more values than one may bind, each split to fit the budget here.

use rusqlite::{Connection, Transaction};

use melodia_core::error::AppError;

use super::rows::FromRow;
use super::sql::{Param, Sql};

/// Bind-variable budget for one statement, divided by a query's binds per row to size its chunks.
///
/// Chosen rather than the engine's: the bundled `SQLite` allows 32766, and 999 is the older default
/// this kept. Moving it trades both ways, a larger chunk buying fewer prepares (every bulk statement
/// is assembled, so prepared per run) for a larger parameter array resident per statement, so move
/// it against a profile of a first scan on a large library.
pub(crate) const MAX_BINDS_PER_STATEMENT: usize = 999;

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
    for sql in statements_per_chunk(NO_HEAD, items, build_sql) {
        all_results.extend(sql.fetch_all::<T>(conn)?);
    }
    Ok(all_results)
}

/// [`chunked_in_query`] for a statement that returns no rows, answering how many it touched. It
/// takes a transaction, so a write spanning chunks lands whole.
pub(crate) fn chunked_in_execute<B>(
    tx: &Transaction<'_>,
    items: &[B],
    build_sql: impl Fn(&str) -> String,
) -> Result<u64, AppError>
where
    B: Clone + Into<Param>,
{
    let mut touched = 0;
    for sql in statements_per_chunk(NO_HEAD, items, build_sql) {
        touched += sql.execute(tx)?;
    }
    Ok(touched)
}

/// One statement per chunk of `ids`, `head` bound ahead of each list.
///
/// Statements rather than a run because the caller picks where each goes: one transaction, or a
/// `db.write` apiece so a waiting writer gets a turn between chunks.
pub(crate) fn chunked_in_statements<H>(
    head: H,
    ids: &[i64],
    build_sql: impl Fn(&str) -> String,
) -> impl Iterator<Item = Sql>
where
    H: Copy + Into<Param>,
{
    statements_per_chunk(Some(head), ids, build_sql)
}

/// Ids per statement that binds one value ahead of its id list.
pub(crate) const IDS_PER_SET: usize = MAX_BINDS_PER_STATEMENT - 1;

/// For a statement that binds nothing ahead of its list, the head's type being arbitrary.
const NO_HEAD: Option<i64> = None;

fn statements_per_chunk<H, B>(
    head: Option<H>,
    items: &[B],
    build_sql: impl Fn(&str) -> String,
) -> impl Iterator<Item = Sql>
where
    H: Copy + Into<Param>,
    B: Clone + Into<Param>,
{
    let per_statement = if head.is_some() { IDS_PER_SET } else { MAX_BINDS_PER_STATEMENT };
    items.chunks(per_statement).map(move |chunk| {
        let mut sql = Sql::trusted(build_sql(&placeholders(chunk.len())));
        if let Some(head) = head {
            sql.bind(head);
        }
        for item in chunk {
            sql.bind(item.clone());
        }
        sql
    })
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

#[cfg(test)]
#[path = "tests/bulk_tests.rs"]
mod tests;
