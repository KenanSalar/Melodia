//! Brings the schema up to this build's, refusing a history it can't stand behind. The history
//! stays sqlx's `_sqlx_migrations`, in sqlx's row format, so an older build migrating through sqlx
//! still reads it and refuses a newer database.

use rusqlite::{Connection, Transaction};
use std::collections::HashMap;
use std::time::Instant;

use melodia_core::error::{AppError, MigrationError};

use super::rows::by_name;
use super::sql;

/// One file of `migrations/`, as `build.rs` embeds it.
struct Migration {
    version: i64,
    description: &'static str,
    sql: &'static str,
    /// What its history row records, as `build.rs` pins or hashes it.
    checksum: &'static [u8],
    foreign_keys: ForeignKeys,
}

/// Whether foreign keys are enforced while a migration runs. `build.rs` reads `Suspended` off a
/// first line of `-- foreign-keys-off`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ForeignKeys {
    Enforced,
    /// For a migration rebuilding a table other tables reference: enforced, its `DROP TABLE`
    /// empties the table first, cascading into the rows that reference it or failing on one that
    /// doesn't cascade. `PRAGMA foreign_keys` does nothing inside a transaction, so it is set
    /// around the migration's, and [`FOREIGN_KEY_CHECK`] proves the rebuild kept every reference
    /// before it commits.
    Suspended,
}

/// Every migration in `migrations/`, in version order.
static MIGRATIONS: &[Migration] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// sqlx's own DDL down to its whitespace, which `sqlite_master` keeps, so a database created here
/// reads the same as one sqlx created.
const CREATE_HISTORY: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)";

const HISTORY_EXISTS: &str = "\
    SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')";

const READ_HISTORY: &str =
    "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version";

/// Inside the migration's transaction, so the row and the schema land together. The time is `-1`
/// until [`RECORD_TIME`] runs after the commit, as sqlx leaves it.
const RECORD: &str = "\
    INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
    VALUES (?1, ?2, TRUE, ?3, -1)";

const RECORD_TIME: &str = "UPDATE _sqlx_migrations SET execution_time = ?1 WHERE version = ?2";

/// One row per reference to a missing row: `table`, `rowid`, `parent`, `fkid`.
const FOREIGN_KEY_CHECK: &str = "PRAGMA foreign_key_check";

struct Applied {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

by_name!(Applied { version, success, checksum });

/// The migrations a boot would apply, and the version the history stands at before them.
pub(super) struct Pending {
    applied_version: i64,
    migrations: Vec<&'static Migration>,
}

impl Pending {
    pub(super) fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }

    /// The newest version applied, 0 on an empty history.
    pub(super) fn applied_version(&self) -> i64 {
        self.applied_version
    }
}

/// What [`apply`] would run on `conn`'s database, or the [`MigrationError`] refusing its history.
/// It only reads, so a backup can be taken between the two.
pub(super) fn pending(conn: &Connection) -> Result<Pending, AppError> {
    let history_exists: bool = sql::fetch_one(conn, HISTORY_EXISTS, ())?;
    let history: Vec<Applied> =
        if history_exists { sql::fetch_all(conn, READ_HISTORY, ())? } else { Vec::new() };
    Ok(compare(MIGRATIONS, &history)?)
}

/// `known` against a `history` in version order, refusing in sqlx's order: a migration recorded
/// as failed, then one this build lacks, then one whose file changed after it ran. Each refusal
/// names its lowest version.
fn compare(known: &'static [Migration], history: &[Applied]) -> Result<Pending, MigrationError> {
    if let Some(failed) = history.iter().find(|row| !row.success) {
        return Err(MigrationError::Dirty(failed.version));
    }
    if let Some(unknown) =
        history.iter().find(|row| known.iter().all(|migration| migration.version != row.version))
    {
        return Err(MigrationError::Unknown(unknown.version));
    }

    let mut migrations = Vec::new();
    for migration in known {
        match history.iter().find(|row| row.version == migration.version) {
            Some(row) if row.checksum != migration.checksum => {
                return Err(MigrationError::Edited(migration.version));
            }
            Some(_) => {}
            None => migrations.push(migration),
        }
    }
    let applied_version = history.last().map_or(0, |row| row.version);
    Ok(Pending { applied_version, migrations })
}

/// Runs each pending migration in its own transaction, recording it as sqlx does. A failing one
/// rolls back with its history row, and the ones before it stay applied.
pub(super) fn apply(conn: &mut Connection, pending: Pending) -> Result<(), AppError> {
    sql::execute(conn, CREATE_HISTORY, ())?;
    for migration in pending.migrations {
        apply_one(conn, migration)?;
    }
    Ok(())
}

fn apply_one(conn: &mut Connection, migration: &'static Migration) -> Result<(), AppError> {
    let started = Instant::now();
    match migration.foreign_keys {
        ForeignKeys::Enforced => run_and_record(conn, migration)?,
        ForeignKeys::Suspended => {
            with_foreign_keys_suspended(conn, |conn| run_and_record(conn, migration))?;
        }
    }

    let elapsed_nanos = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);
    sql::execute(conn, RECORD_TIME, (elapsed_nanos, migration.version))?;
    Ok(())
}

/// Turns enforcement back on whether `migrate` committed or rolled back, the writer outliving the
/// migration.
fn with_foreign_keys_suspended(
    conn: &mut Connection,
    migrate: impl FnOnce(&mut Connection) -> Result<(), MigrationError>,
) -> Result<(), AppError> {
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let migrated = migrate(conn);
    let restored = conn.pragma_update(None, "foreign_keys", "ON");
    migrated?;
    Ok(restored?)
}

/// Any failure here leaves the migration unapplied.
fn run_and_record(conn: &mut Connection, migration: &Migration) -> Result<(), MigrationError> {
    let version = migration.version;
    let failed = |source: rusqlite::Error| MigrationError::Apply { version, source };

    let tx = conn.transaction().map_err(failed)?;
    let suspended = migration.foreign_keys == ForeignKeys::Suspended;
    let dangling_before = suspended.then(|| dangling(&tx)).transpose().map_err(failed)?;
    tx.execute_batch(migration.sql).map_err(failed)?;
    if let Some(before) = dangling_before {
        check_references(&tx, version, &before)?;
    }
    tx.prepare_cached(RECORD)
        .and_then(|mut record| {
            record.execute((migration.version, migration.description, migration.checksum))
        })
        .map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Refuses a reference the migration left dangling. One already dangling before it ran is the
/// database's own, and refusing that would fail every boot.
fn check_references(
    tx: &Transaction<'_>,
    version: i64,
    before: &DanglingCounts,
) -> Result<(), MigrationError> {
    let after = dangling(tx).map_err(|source| MigrationError::Apply { version, source })?;
    let grew =
        after.into_iter().find(|(pair, count)| before.get(pair).is_none_or(|was| count > was));
    match grew {
        Some(((table, parent), _)) => {
            Err(MigrationError::DanglingReference { version, table, parent })
        }
        None => Ok(()),
    }
}

/// Dangling references per `(table, parent)`, counted rather than named: a rebuild may renumber
/// the rowids and FK indexes that would name each one.
type DanglingCounts = HashMap<(String, String), usize>;

/// Read through a fetch: `execute_batch` steps each statement once and drops its rows, so a check
/// inside a script would pass whatever it found.
fn dangling(tx: &Transaction<'_>) -> rusqlite::Result<DanglingCounts> {
    let mut check = tx.prepare(FOREIGN_KEY_CHECK)?;
    let pairs = check.query_map((), |row| Ok((row.get(0)?, row.get(2)?)))?;
    let mut counts = DanglingCounts::new();
    for pair in pairs {
        *counts.entry(pair?).or_insert(0) += 1;
    }
    Ok(counts)
}

#[cfg(test)]
#[path = "tests/migrate_tests.rs"]
mod tests;
