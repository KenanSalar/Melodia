//! Brings the schema up to this build's, refusing a history it can't stand behind. The history
//! stays sqlx's `_sqlx_migrations`, row for row, so an older build migrating through sqlx still
//! reads it and refuses a newer database as before.

use sqlx::Connection as _;
use sqlx::sqlite::SqliteConnection;
use std::time::Instant;

use melodia_core::error::{AppError, MigrationError};

/// One file of `migrations/`, as `build.rs` embeds it.
struct Migration {
    version: i64,
    description: &'static str,
    sql: &'static str,
    /// SHA-384 over the file's bytes.
    checksum: [u8; 48],
}

/// Every migration in `migrations/`, in version order.
static MIGRATIONS: &[Migration] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// sqlx's own DDL, column for column.
const CREATE_HISTORY: &str = "\
    CREATE TABLE IF NOT EXISTS _sqlx_migrations (
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

#[derive(sqlx::FromRow)]
struct Applied {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

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
pub(super) async fn pending(conn: &mut SqliteConnection) -> Result<Pending, AppError> {
    let history_exists: bool = sqlx::query_scalar(HISTORY_EXISTS).fetch_one(&mut *conn).await?;
    let history: Vec<Applied> = if history_exists {
        sqlx::query_as(READ_HISTORY).fetch_all(&mut *conn).await?
    } else {
        Vec::new()
    };
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
pub(super) async fn apply(conn: &mut SqliteConnection, pending: Pending) -> Result<(), AppError> {
    sqlx::query(CREATE_HISTORY).execute(&mut *conn).await?;
    for migration in pending.migrations {
        apply_one(conn, migration).await?;
    }
    Ok(())
}

async fn apply_one(
    conn: &mut SqliteConnection,
    migration: &'static Migration,
) -> Result<(), AppError> {
    let started = Instant::now();
    run_and_record(conn, migration)
        .await
        .map_err(|source| MigrationError::Apply { version: migration.version, source })?;

    let elapsed_nanos = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);
    sqlx::query(RECORD_TIME).bind(elapsed_nanos).bind(migration.version).execute(conn).await?;
    Ok(())
}

/// Any failure here leaves the migration unapplied, so the caller names its version.
async fn run_and_record(
    conn: &mut SqliteConnection,
    migration: &Migration,
) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
    sqlx::query(RECORD)
        .bind(migration.version)
        .bind(migration.description)
        .bind(migration.checksum.as_slice())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
