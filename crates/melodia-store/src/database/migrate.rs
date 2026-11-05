//! Brings the schema up to this build's, refusing a history it can't stand behind. The history
//! stays sqlx's `_sqlx_migrations`, in sqlx's row format, so an older build migrating through sqlx
//! still reads it and refuses a newer database as before.

use rusqlite::Connection;
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
    run_and_record(conn, migration)
        .map_err(|source| MigrationError::Apply { version: migration.version, source })?;

    let elapsed_nanos = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);
    sql::execute(conn, RECORD_TIME, (elapsed_nanos, migration.version))?;
    Ok(())
}

/// Any failure here leaves the migration unapplied, so the caller names its version.
fn run_and_record(conn: &mut Connection, migration: &Migration) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(migration.sql)?;
    tx.prepare_cached(RECORD)?.execute((
        migration.version,
        migration.description,
        migration.checksum,
    ))?;
    tx.commit()
}
