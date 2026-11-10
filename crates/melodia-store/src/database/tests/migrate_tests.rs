use rusqlite::Connection;

use super::{Applied, ForeignKeys, MIGRATIONS, Migration, Pending, apply, compare};
use crate::database::{DbPool, sql};
use melodia_core::error::{AppError, MigrationError};

static KNOWN: &[Migration] = &[
    Migration {
        version: 1,
        description: "first",
        sql: "",
        checksum: b"one",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 2,
        description: "second",
        sql: "",
        checksum: b"two",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 3,
        description: "third",
        sql: "",
        checksum: b"three",
        foreign_keys: ForeignKeys::Enforced,
    },
];

fn ran(version: i64, checksum: &[u8]) -> Applied {
    Applied { version, success: true, checksum: checksum.to_vec() }
}

fn failed(version: i64, checksum: &[u8]) -> Applied {
    Applied { version, success: false, checksum: checksum.to_vec() }
}

/// What a boot would run, and the version its backup is named after.
fn plan(pending: &Pending) -> (Vec<i64>, i64) {
    let versions = pending.migrations.iter().map(|migration| migration.version).collect();
    (versions, pending.applied_version())
}

#[test]
fn an_empty_history_leaves_every_migration_pending() -> Result<(), MigrationError> {
    let pending = compare(KNOWN, &[])?;
    assert_eq!(plan(&pending), (vec![1, 2, 3], 0));
    Ok(())
}

#[test]
fn a_history_at_this_build_leaves_nothing_pending() -> Result<(), MigrationError> {
    let history = [ran(1, b"one"), ran(2, b"two"), ran(3, b"three")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![], 3));
    Ok(())
}

#[test]
fn a_history_behind_this_build_leaves_only_the_newer_migrations_pending()
-> Result<(), MigrationError> {
    let history = [ran(1, b"one"), ran(2, b"two")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![3], 2));
    Ok(())
}

/// The backup is named after the newest version applied, not after how many rows there are.
#[test]
fn a_version_missing_from_the_middle_of_the_history_is_still_pending() -> Result<(), MigrationError>
{
    let history = [ran(1, b"one"), ran(3, b"three")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![2], 3));
    Ok(())
}

#[test]
fn a_migration_recorded_as_failed_refuses_the_history_as_dirty() {
    let history = [ran(1, b"one"), failed(2, b"two")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Dirty(2))), "{:?}", refused.err());
}

#[test]
fn a_version_this_build_lacks_refuses_the_history_as_unknown() {
    let history = [ran(1, b"one"), ran(2, b"two"), ran(3, b"three"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Unknown(4))), "{:?}", refused.err());
}

#[test]
fn a_migration_whose_file_changed_refuses_the_history_as_edited() {
    let history = [ran(1, b"one"), ran(2, b"changed")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Edited(2))), "{:?}", refused.err());
}

#[test]
fn a_failed_migration_is_refused_before_an_unknown_one() {
    let history = [ran(1, b"one"), failed(2, b"two"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Dirty(2))), "{:?}", refused.err());
}

#[test]
fn an_unknown_migration_is_refused_before_an_edited_one() {
    let history = [ran(1, b"changed"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Unknown(4))), "{:?}", refused.err());
}

static LANDS_THEN_FAILS: [Migration; 2] = [
    Migration {
        version: 1,
        description: "lands",
        sql: "CREATE TABLE landed (x)",
        checksum: b"1",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 2,
        description: "fails",
        sql: "CREATE TABLE half_made (x); INSERT INTO no_such_table VALUES (1)",
        checksum: b"2",
        foreign_keys: ForeignKeys::Enforced,
    },
];

fn apply_lands_then_fails(conn: &mut Connection) -> Result<(), AppError> {
    let pending = Pending { applied_version: 0, migrations: LANDS_THEN_FAILS.iter().collect() };
    apply(conn, pending)
}

#[test]
fn a_failing_migration_is_named_by_its_version() -> Result<(), AppError> {
    let mut conn = Connection::open_in_memory()?;

    let outcome = apply_lands_then_fails(&mut conn);

    assert!(
        matches!(outcome, Err(AppError::Migration(MigrationError::Apply { version: 2, .. }))),
        "{:?}",
        outcome.err()
    );
    Ok(())
}

/// The next boot retries from the version the backup captured, so a half-applied migration would
/// meet its own tables and fail again.
#[test]
fn a_failing_migration_rolls_back_with_its_history_row() -> Result<(), AppError> {
    let mut conn = Connection::open_in_memory()?;

    let _failed = apply_lands_then_fails(&mut conn);

    let tables: Vec<String> = sql::fetch_all(
        &conn,
        "SELECT name FROM sqlite_master WHERE name IN ('landed', 'half_made') ORDER BY name",
        (),
    )?;
    let history: Vec<i64> =
        sql::fetch_all(&conn, "SELECT version FROM _sqlx_migrations ORDER BY version", ())?;
    assert_eq!((tables, history), (vec!["landed".to_owned()], vec![1]));
    Ok(())
}

type HistoryRow = (i64, String, bool, Vec<u8>, bool);

/// **An older build reads this history through sqlx**, so each row is one sqlx would have written.
#[tokio::test]
async fn every_applied_migration_is_recorded_as_sqlx_records_it() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let history: Vec<HistoryRow> = db
        .read(|conn| {
            sql::fetch_all(
                conn,
                "SELECT version, description, success, checksum, execution_time >= 0 \
                 FROM _sqlx_migrations ORDER BY version",
                (),
            )
        })
        .await?;

    let embedded: Vec<HistoryRow> = MIGRATIONS
        .iter()
        .map(|m| (m.version, m.description.to_owned(), true, m.checksum.to_vec(), true))
        .collect();
    assert_eq!(history, embedded);
    Ok(())
}

/// sqlx-sqlite's DDL as `sqlite_master` keeps it, so a fresh database's schema reads byte for byte
/// as one sqlx created.
const SQLX_HISTORY_DDL: &str = "CREATE TABLE _sqlx_migrations (\n    \
    version BIGINT PRIMARY KEY,\n    \
    description TEXT NOT NULL,\n    \
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,\n    \
    success BOOLEAN NOT NULL,\n    \
    checksum BLOB NOT NULL,\n    \
    execution_time BIGINT NOT NULL\n\
)";

#[tokio::test]
async fn the_history_table_is_created_with_sqlx_s_own_ddl() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let ddl: String = db
        .read(|conn| {
            sql::fetch_one(
                conn,
                "SELECT sql FROM sqlite_master WHERE name = '_sqlx_migrations'",
                (),
            )
        })
        .await?;

    assert_eq!(ddl, SQLX_HISTORY_DDL);
    Ok(())
}
