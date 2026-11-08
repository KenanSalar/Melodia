use std::path::Path;

use rusqlite::Connection;

use super::open::{self, Role};
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

/// A pool like a launch's, with a reader and the writer apart.
async fn file_pool(dir: &Path) -> Result<(Paths, DbPool), AppError> {
    let paths = Paths::rooted_at(dir.to_path_buf());
    paths.create_dirs()?;
    let db = init_database(&paths).await?;
    Ok((paths, db))
}

/// **Every connection a keyset query can run on knows `UNICODE_LOWER`**, the writer and the test
/// pool included: one without it fails any statement naming it, and one that registered a looser
/// fold would sort an artist list differently from the comparator it reproduces.
#[tokio::test]
async fn every_pool_orders_text_the_way_to_lowercase_does() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (_paths, db) = file_pool(tmp.path()).await?;
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

fn select_one(conn: &Connection) -> Result<i64, AppError> {
    sql::fetch_one(conn, "SELECT 1", ())
}

#[tokio::test]
async fn a_closed_pool_refuses_every_call_as_closed() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (_paths, db) = file_pool(tmp.path()).await?;
    db.close().await;

    let read = db.read(select_one).await;
    let write = db.write(|conn| select_one(conn)).await;

    assert!(matches!(read, Err(AppError::DatabaseClosed)), "the read lane: {read:?}");
    assert!(matches!(write, Err(AppError::DatabaseClosed)), "the write lane: {write:?}");
    Ok(())
}

const INSERT_FOLDER: &str = "\
    INSERT INTO folders (path, is_enabled, added_at) VALUES ('/music', 1, '2026-01-01T00:00:00+00:00')";

#[tokio::test]
async fn a_transaction_whose_work_fails_keeps_none_of_its_writes() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let failed = db
        .transaction(|tx| {
            sql::execute(tx, INSERT_FOLDER, ())?;
            Err::<(), _>(AppError::validation_msg("the work failed after its write"))
        })
        .await;

    let folders: i64 =
        db.read(|conn| sql::fetch_one(conn, "SELECT COUNT(*) FROM folders", ())).await?;
    assert!(matches!(failed, Err(AppError::Validation { .. })), "{failed:?}");
    assert_eq!(folders, 0);
    Ok(())
}

fn select_double_quoted(conn: &Connection) -> Result<String, AppError> {
    sql::fetch_one(conn, "SELECT \"no_such_column\"", ())
}

#[tokio::test]
async fn every_pool_refuses_a_double_quoted_string_literal() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (_paths, db) = file_pool(tmp.path()).await?;
    let test_pool = DbPool::test_pool().await?;

    let answers = [
        (db.read(select_double_quoted).await, "read"),
        (db.write(|conn| select_double_quoted(conn)).await, "write"),
        (test_pool.read(select_double_quoted).await, "test"),
    ];
    for (answer, name) in answers {
        assert!(matches!(answer, Err(AppError::Database(_))), "the {name} pool: {answer:?}");
    }
    Ok(())
}

/// The open reader is what makes the order matter: one closing after the writer leaves the WAL
/// beside the file.
#[tokio::test]
async fn closing_the_pool_folds_the_wal_into_the_database() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = file_pool(tmp.path()).await?;
    db.write(|conn| sql::execute(conn, INSERT_FOLDER, ())).await?;
    db.read(select_one).await?;
    let wal = paths.db_path.with_extension("db-wal");
    assert!(wal.exists(), "the write never reached a WAL, so its absence proves nothing");

    db.close().await;

    assert!(!wal.exists(), "{} outlived the pool", wal.display());
    Ok(())
}

#[tokio::test]
async fn compacting_leaves_the_wal_empty() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = file_pool(tmp.path()).await?;
    db.write(|conn| sql::execute(conn, INSERT_FOLDER, ())).await?;
    let wal = paths.db_path.with_extension("db-wal");
    assert!(std::fs::metadata(&wal)?.len() > 0, "nothing reached the WAL, so empty proves nothing");

    db.compact().await?;

    assert_eq!(std::fs::metadata(&wal)?.len(), 0);
    Ok(())
}

fn temp_store(conn: &Connection) -> Result<i64, AppError> {
    sql::fetch_one(conn, "PRAGMA temp_store", ())
}

#[tokio::test]
async fn compacting_puts_the_writer_back_on_its_temp_store() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = file_pool(tmp.path()).await?;
    // A new connection's, the pool's writer having already been through the boot's own compact.
    let as_opened = temp_store(&open::open(&paths.db_path, Role::Writer)?)?;

    db.compact().await?;

    assert_eq!(db.write(|conn| temp_store(conn)).await?, as_opened);
    Ok(())
}
