use rusqlite::Connection;

use super::{DbPool, FTS_OPTIMIZE};
use crate::database::{init_database, sql};
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Strings `to_lowercase()` orders differently from `NOCASE`, which folds ASCII alone, and from a
/// char-by-char lowercase, which misses the final sigma. Ties go to the raw bytes so the expected
/// order is total.
const UNICODE_LOWER_ORDER: &str = "\
    WITH v(s) AS (VALUES ('Zebra'), ('b'), ('Ωmega'), ('apple'), ('Émile'), ('οδος'), ('éa'), \
                         ('ΟΔΟΣ'), ('αlpha')) \
    SELECT s FROM v ORDER BY s COLLATE UNICODE_LOWER, s";

fn unicode_lower_order(conn: &Connection) -> Result<Vec<String>, AppError> {
    sql::fetch_all(conn, UNICODE_LOWER_ORDER, ())
}

/// **Every connection a keyset query can run on knows `UNICODE_LOWER`**, the writer and the test
/// pool included: one without it fails any statement naming it, and one that registered a looser
/// fold would sort an artist list differently from the comparator it reproduces.
#[tokio::test]
async fn every_pool_orders_text_the_way_to_lowercase_does() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let db = init_database(&paths).await?;
    let test_pool = DbPool::test_pool().await?;
    let expected = ["apple", "b", "Zebra", "éa", "Émile", "αlpha", "ΟΔΟΣ", "οδος", "Ωmega"];

    let orders = [
        (db.read(unicode_lower_order).await?, "read"),
        (db.write(|conn| unicode_lower_order(conn)).await?, "write"),
        (test_pool.read(unicode_lower_order).await?, "test"),
    ];
    for (order, name) in orders {
        assert_eq!(order, expected, "the {name} pool");
    }
    Ok(())
}

#[tokio::test]
async fn db_pool_close_succeeds() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    db.read(|conn| sql::fetch_all::<i64>(conn, "SELECT 1", ())).await?;
    db.close().await;
    Ok(())
}

/// `close` only logs a refused [`FTS_OPTIMIZE`], so a mistyped command fails here or nowhere.
#[tokio::test]
async fn the_shutdown_fts_optimize_is_a_command_fts5_accepts() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    db.write(|conn| sql::execute(conn, FTS_OPTIMIZE, ())).await?;
    Ok(())
}

#[tokio::test]
async fn db_pool_read_write_accessors() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    db.read(|conn| sql::fetch_one::<i64>(conn, "SELECT 42", ())).await?;
    db.write(|conn| sql::fetch_one::<i64>(conn, "SELECT 42", ())).await?;
    Ok(())
}
