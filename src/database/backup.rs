//! Pre-migration database backups, and the retention that bounds them.
//!
//! **Why `VACUUM INTO` and not a file copy.** The database is open in WAL mode
//! for the whole run, so `melodia.db` on its own is not the database — committed
//! transactions live in the `-wal` sidecar until a checkpoint folds them back.
//! Copying the file alone loses them; copying all three risks catching a write
//! mid-flight. `VACUUM INTO` runs *through* the open connection and writes a
//! fresh, fully-checkpointed database at the target path, which is why `SQLite`'s
//! own corruption guide names it (beside the backup API and `sqlite3_rsync`) as
//! a way to copy a live database and names filesystem copying as a way to
//! corrupt one. The output has no sidecars of its own — but the *live* database
//! usually does, so restoring means deleting `melodia.db-wal` and
//! `melodia.db-shm` beside the rename, with the app closed. A boot that died in
//! a migration is precisely the case that leaves them behind (nothing calls
//! `DbPool::close`), and a WAL left in place is recovered onto page numbers the
//! vacuum has already moved.
//!
//! **Why the copy is staged.** `VACUUM INTO` writes the target incrementally, so
//! a crash part-way leaves a truncated file under a name that reads as a
//! complete backup — and [`create`] skips a version it already has, so nothing
//! would ever replace it. Writing to a `.tmp` sibling and renaming makes "the
//! final name exists" mean "it is complete".
//!
//! **What retention may delete.** [`prune`] removes a file only if it can parse
//! the name back into the scheme this module writes — the newest [`MAX_BACKUPS`]
//! survive, and a staged `.tmp` goes whatever the count is. A backup someone made
//! by hand sits in the same directory and is not ours to retire.
//!
//! **What the sweep may move.** Older versions wrote their backup loose in the
//! data directory, under a name one segment away from the live database's own
//! (`melodia.db.backup-v…` beside `melodia.db`). [`adopt_legacy`] moves those into
//! `backups/` so they come under the retention above instead of being orphaned by
//! it — which means it walks the directory the running library sits in, and
//! [`legacy_version_of`] is the whole of what keeps it off that file. It parses a
//! version out rather than testing a prefix for exactly that reason: loosened to
//! `melodia.db.`, the same loop moves the database out from under the app. Where
//! the version is already present the loose copy is deleted rather than renamed,
//! since `rename` onto an existing path fails on Windows and two snapshots of one
//! schema version is not a choice worth making.

use std::path::{Path, PathBuf};

use sqlx::AssertSqlSafe;
use sqlx::sqlite::SqlitePool;

use crate::error::AppError;

/// How many backups survive a [`maintain`] sweep.
///
/// Each one is a full copy of the library database, so the cost scales with the
/// library rather than with a constant — and a backup's value decays fast, since
/// what you want back is the state from just before the migration that broke
/// something. Three covers that with room for two upgrades of hindsight.
const MAX_BACKUPS: usize = 3;

const PREFIX: &str = "melodia-v";
const SUFFIX: &str = ".db";

/// What [`tmp_file_name`] appends to mark a copy as still in progress. Appended
/// rather than replacing `SUFFIX`, so a half-written file can never satisfy
/// [`version_of`] — the version no longer ends the name.
const TMP_SUFFIX: &str = ".tmp";

// The two names written by the version of this code that kept backups loose in
// the data directory beside the live database; `adopt_legacy` retires both.
const LEGACY_PREFIX: &str = "melodia.db.backup-v";
const LEGACY_INITIAL: &str = "melodia.db.pre-migration-backup";

/// The one definition of the naming scheme; [`version_of`] is its inverse.
fn file_name(version: i64) -> String {
    format!("{PREFIX}{version}{SUFFIX}")
}

/// The staged name [`create`] renames from — [`file_name`] plus a suffix, so the
/// two can't drift into a staged copy that isn't the final name.
fn tmp_file_name(version: i64) -> String {
    format!("{}{TMP_SUFFIX}", file_name(version))
}

/// The schema version a backup captures, or `None` for a name this module did
/// not write. Pruning is gated on this, so it is the whole of what keeps the
/// sweep off a file that isn't ours.
fn version_of(name: &str) -> Option<i64> {
    name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.parse().ok()
}

/// The version a staged copy was going to become. [`prune`]'s `.tmp` sweep is
/// gated on this for the same reason the rest of it is gated on [`version_of`].
fn tmp_version_of(name: &str) -> Option<i64> {
    version_of(name.strip_suffix(TMP_SUFFIX)?)
}

/// Same, for the two pre-`backups/` names.
fn legacy_version_of(name: &str) -> Option<i64> {
    if name == LEGACY_INITIAL {
        // A database with no applied migrations *is* version 0 — it sorts
        // oldest, so retention retires it first, which is right: it holds an
        // empty schema.
        return Some(0);
    }
    name.strip_prefix(LEGACY_PREFIX)?.parse().ok()
}

/// Copy the database as it stands at `version` into `backups_dir`, returning the
/// path written. Call before applying migrations.
///
/// Returns early if that version is already backed up. A migration that fails is
/// rolled back, so a re-attempt finds the database at the same applied version
/// the existing file captured — re-vacuuming would copy identical bytes, and on
/// a large library it would do so on every launch for as long as the migration
/// keeps failing.
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
    // A missing parent directory comes back as "unable to open database: <path>"
    // with nothing pointing at the directory, and this path is fatal to startup.
    // One idempotent call keeps that from being how it surfaces.
    //
    // Every failure below names its file: the boot stops here, and `main`
    // reports an `AppError` that a bare `io::Error` would leave as an errno with
    // no path in it — on a desktop launch, with no terminal to print to at all.
    std::fs::create_dir_all(backups_dir).map_err(|e| {
        AppError::io_other(format!(
            "Could not create the backup directory {}: {e}",
            backups_dir.display()
        ))
    })?;

    let tmp_path = backups_dir.join(tmp_file_name(version));
    if tmp_path.exists() {
        std::fs::remove_file(&tmp_path).map_err(|e| {
            AppError::io_other(format!(
                "Could not clear the staged backup at {}: {e}",
                tmp_path.display()
            ))
        })?;
    }

    // `VACUUM INTO` takes no bind parameter, so the path has to be interpolated.
    // It is machine-derived from `dirs::data_dir()`; doubling any quote is what
    // keeps a directory name with an apostrophe from ending the literal early.
    let escaped = tmp_path.display().to_string().replace('\'', "''");
    sqlx::raw_sql(AssertSqlSafe(format!("VACUUM INTO '{escaped}'"))).execute(pool).await?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        AppError::io_other(format!("Could not publish the backup as {}: {e}", final_path.display()))
    })?;

    log::info!("Database backup created at {}", final_path.display());
    Ok(final_path)
}

/// Adopt any loose legacy backups, then cut the directory back to
/// [`MAX_BACKUPS`]. Best-effort throughout — nothing here can fail a launch,
/// since the only thing at stake is disk space.
///
/// Runs on every launch rather than only on a migration launch — a user whose
/// schema is already current has no pending migration to trigger a sweep, and
/// would otherwise keep their loose files indefinitely.
///
/// Synchronous, unlike `updater::prune_stale_staging`'s `spawn_blocking`: this
/// walks a directory holding at most a handful of entries, so a plain `fn` keeps
/// the call site free of owned clones and stays testable without a runtime.
pub(super) fn maintain(data_dir: &Path, backups_dir: &Path) {
    let _ = std::fs::create_dir_all(backups_dir);
    adopt_legacy(data_dir, backups_dir);
    prune(backups_dir);
}

/// Move backups written under the old loose naming into `backups_dir`, so they
/// come under the retention policy instead of being orphaned by it.
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
        // Renaming onto an existing path fails on Windows, and the file already
        // in `backups/` holds the same schema version — drop the legacy copy
        // rather than deciding between two snapshots of one version.
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
                log::warn!("Could not retire the legacy backup {}: {e}", source.display());
            }
        }
    }
}

/// Delete all but the [`MAX_BACKUPS`] newest backups, plus any `.tmp` left by an
/// interrupted [`create`].
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
                log::warn!("Could not clear the staged backup {name}: {e}");
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
            Err(e) => log::warn!("Could not retire {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
#[path = "tests/backup_tests.rs"]
mod tests;
