//! Pre-migration database backups, and the retention that bounds them. The three invariants are
//! the root `CLAUDE.md`'s; what is written down only here:
//!
//! **Restoring** means deleting `melodia.db-wal` and `melodia.db-shm` beside the rename, with the
//! app closed. A boot that died in a migration never reached `DbPool::close` and leaves them, and a
//! WAL left in place is recovered onto page numbers the vacuum already moved.
//!
//! **The sweep walks the live database's directory.** Older versions wrote backups loose beside
//! `melodia.db` (`melodia.db.backup-v…`), and [`adopt_legacy`] moves them under retention, so
//! [`legacy_version_of`] is all that keeps it off the database itself. It parses a version out
//! rather than testing a prefix, since `melodia.db.` would move the database out from under the app.

use std::path::{Path, PathBuf};

use sqlx::AssertSqlSafe;
use sqlx::sqlite::SqlitePool;

use melodia_core::error::{AppError, describe};

/// How many backups a [`maintain`] sweep keeps. Each is a full copy of the library, and what you
/// want back is the state just before the migration that broke something.
const MAX_BACKUPS: usize = 3;

const PREFIX: &str = "melodia-v";
const SUFFIX: &str = ".db";

/// What [`tmp_file_name`] appends to mark a copy as still in progress. Appended rather than
/// replacing `SUFFIX`, so a half-written file can never satisfy [`version_of`].
const TMP_SUFFIX: &str = ".tmp";

// The pre-`backups/` names, loose beside the live database.
const LEGACY_PREFIX: &str = "melodia.db.backup-v";
const LEGACY_INITIAL: &str = "melodia.db.pre-migration-backup";

/// The one definition of the naming scheme; [`version_of`] is its inverse.
fn file_name(version: i64) -> String {
    format!("{PREFIX}{version}{SUFFIX}")
}

/// The staged name [`create`] renames from.
fn tmp_file_name(version: i64) -> String {
    format!("{}{TMP_SUFFIX}", file_name(version))
}

/// The schema version a backup captures, or `None` for a name this module did not write. Pruning
/// is gated on this, so it is the whole of what keeps the sweep off a file that isn't ours.
fn version_of(name: &str) -> Option<i64> {
    name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.parse().ok()
}

/// The version a staged copy was going to become, gating [`prune`]'s `.tmp` sweep as
/// [`version_of`] gates the rest.
fn tmp_version_of(name: &str) -> Option<i64> {
    version_of(name.strip_suffix(TMP_SUFFIX)?)
}

/// Same, for the two pre-`backups/` names.
fn legacy_version_of(name: &str) -> Option<i64> {
    if name == LEGACY_INITIAL {
        // No migrations applied is version 0, the empty schema retention retires first.
        return Some(0);
    }
    name.strip_prefix(LEGACY_PREFIX)?.parse().ok()
}

/// Copies the database as it stands at `version` into `backups_dir`, returning the path written.
///
/// Returns early where that version is already backed up: a failed migration rolls back, so each
/// retry would copy identical bytes.
pub(super) async fn create(
    pool: &SqlitePool,
    backups_dir: &Path,
    version: i64,
) -> Result<PathBuf, AppError> {
    let final_path = backups_dir.join(file_name(version));
    if final_path.exists() {
        log::info!("Database backup already present at {}", final_path.display());
        return Ok(final_path);
    }
    // Fatal to startup, so every failure here names its file: a missing directory otherwise reads
    // "unable to open database", and a bare `io::Error` is an errno with no path.
    std::fs::create_dir_all(backups_dir).map_err(|e| {
        AppError::io(format!("Could not create the backup directory {}", backups_dir.display()), e)
    })?;

    let tmp_path = backups_dir.join(tmp_file_name(version));
    if tmp_path.exists() {
        std::fs::remove_file(&tmp_path).map_err(|e| {
            AppError::io(format!("Could not clear the staged backup at {}", tmp_path.display()), e)
        })?;
    }

    // `VACUUM INTO` takes no bind, so the path is interpolated with its quotes doubled.
    let escaped = tmp_path.display().to_string().replace('\'', "''");
    sqlx::raw_sql(AssertSqlSafe(format!("VACUUM INTO '{escaped}'"))).execute(pool).await?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        AppError::io(format!("Could not publish the backup as {}", final_path.display()), e)
    })?;

    log::info!("Database backup created at {}", final_path.display());
    Ok(final_path)
}

/// Adopts loose legacy backups, then cuts the directory back to [`MAX_BACKUPS`]. Best-effort, only
/// disk space being at stake.
///
/// Every launch, since a current schema has no migration to trigger a sweep. Synchronous: it walks
/// a handful of entries, and a plain `fn` stays testable without a runtime.
pub(super) fn maintain(data_dir: &Path, backups_dir: &Path) {
    let _ = std::fs::create_dir_all(backups_dir);
    adopt_legacy(data_dir, backups_dir);
    prune(backups_dir);
}

fn adopt_legacy(data_dir: &Path, backups_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let file_name_os = entry.file_name();
        let Some(name) = file_name_os.to_str() else {
            continue;
        };
        let Some(version) = legacy_version_of(name) else {
            continue;
        };

        let source = entry.path();
        let target = backups_dir.join(file_name(version));
        // `rename` onto an existing path fails on Windows, and that file holds this version already.
        let superseded = target.exists();
        let outcome = if superseded {
            std::fs::remove_file(&source)
        } else {
            std::fs::rename(&source, &target)
        };

        match (outcome, superseded) {
            (Ok(()), false) => log::info!("Adopted legacy database backup {name}"),
            (Ok(()), true) => {
                log::info!("Dropped legacy backup {name} — v{version} is already under retention");
            }
            (Err(e), _) => {
                log::warn!(
                    "Could not retire the legacy backup {}: {}",
                    source.display(),
                    describe(&e)
                );
            }
        }
    }
}

/// Deletes all but the [`MAX_BACKUPS`] newest backups, and any `.tmp` an interrupted [`create`]
/// left.
fn prune(backups_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(backups_dir) else {
        return;
    };

    let mut backups: Vec<(i64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let file_name_os = entry.file_name();
        let Some(name) = file_name_os.to_str() else {
            continue;
        };
        // A staged copy is never restorable, so it goes whatever the count is.
        if tmp_version_of(name).is_some() {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                log::warn!("Could not clear the staged backup {name}: {}", describe(&e));
            }
            continue;
        }
        if let Some(version) = version_of(name) {
            backups.push((version, entry.path()));
        }
    }

    if backups.len() <= MAX_BACKUPS {
        return;
    }

    backups.sort_unstable_by_key(|(version, _)| *version);
    for (_, path) in &backups[..backups.len() - MAX_BACKUPS] {
        match std::fs::remove_file(path) {
            Ok(()) => log::info!("Retired old database backup {}", path.display()),
            Err(e) => log::warn!("Could not retire {}: {}", path.display(), describe(&e)),
        }
    }
}

#[cfg(test)]
#[path = "tests/backup_tests.rs"]
mod tests;
