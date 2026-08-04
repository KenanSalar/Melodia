use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::{MAX_BACKUPS, create, file_name, maintain, prune};
use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::insert_test_track;
use crate::error::AppError;
use crate::test_support::paths_in;

/// Everything in `dir`, sorted. Backup names are equal-width within a
/// generation, so lexicographic order is chronological order — the same
/// property retention leans on.
fn entry_names(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// A real, file-backed, migrated database in `dir`.
///
/// Deliberately not `DbPool::test_pool()`: that one is `sqlite::memory:`, and
/// `VACUUM INTO` from an in-memory database through sqlx returns `Ok` while
/// writing no file at all. Every test that exercises the copy has to run against
/// a pool shaped like the one production backs up.
async fn open_db(dir: &Path) -> Result<(Paths, DbPool), AppError> {
    let paths = paths_in(dir);
    let db = crate::database::init_database(&paths).await?;
    Ok((paths, db))
}

/// The question the whole module exists to answer: is the file it leaves behind
/// a database you could restore from?
///
/// The track is inserted and never checkpointed, so at the moment of the backup
/// it lives in the `-wal` sidecar rather than in `melodia.db` — copying that one
/// file would lose it. Counting it back out of the copy is what says `VACUUM
/// INTO` folded the WAL in, and that the result stands alone with no sidecars of
/// its own.
#[tokio::test]
async fn a_backup_round_trips_as_an_openable_database() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = open_db(tmp.path()).await?;
    queries::folder::insert_folder(&db, "/test", true).await?;
    insert_test_track(&db, "/test/song.mp3", "Song", "Art", "Alb", "Pop").await?;

    // A 32-byte WAL is the header alone. Anything past it is a frame a copy of
    // `melodia.db` on its own would have lost — assert it, or a checkpoint
    // landing here first would leave the test passing and proving nothing.
    let wal = std::fs::metadata(tmp.path().join("melodia.db-wal"))?.len();
    assert!(wal > 32, "nothing was left in the WAL for the backup to fold in");

    let path = create(db.write(), &paths.backups_dir, 20_260_514_000_000).await?;
    assert_eq!(
        entry_names(&paths.backups_dir)?,
        ["melodia-v20260514000000.db"],
    );
    db.close().await;

    let opts =
        SqliteConnectOptions::from_str(&format!("sqlite:{}", path.display()))?.read_only(true);
    let restored = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    let (tracks,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tracks")
        .fetch_one(&restored)
        .await?;
    restored.close().await;

    assert_eq!(tracks, 1);
    Ok(())
}

/// The write pool's `create_if_missing` makes the database file exist before the
/// backup is reached, so the existence check has to happen earlier than the
/// backup itself. Get that wrong and every first launch leaves a snapshot of an
/// empty schema behind permanently.
#[tokio::test]
async fn a_fresh_database_is_not_backed_up() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = open_db(tmp.path()).await?;
    db.close().await;
    assert!(
        entry_names(&paths.backups_dir)?.is_empty(),
        "a first launch backed up its own empty schema",
    );

    // Second launch: the schema is current, so there is still nothing pending
    // and still nothing to copy.
    let (paths, db) = open_db(tmp.path()).await?;
    db.close().await;
    assert!(entry_names(&paths.backups_dir)?.is_empty());
    Ok(())
}

/// Written out of order on purpose: a prune that trusted `read_dir` order rather
/// than the version in the name would pass on a sorted directory.
#[test]
fn prune_keeps_the_newest_and_deletes_the_rest() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    for version in [5, 1, 4, 2, 3] {
        std::fs::write(tmp.path().join(file_name(version)), b"x")?;
    }

    prune(tmp.path());

    assert_eq!(
        entry_names(tmp.path())?,
        ["melodia-v3.db", "melodia-v4.db", "melodia-v5.db"],
    );
    Ok(())
}

/// The safety property: pruning is gated on parsing a name back into the scheme
/// this module writes, not on the file merely sitting in the directory. The
/// mutation to catch is a sweep that deletes by age or by count alone.
#[test]
fn prune_never_touches_a_name_it_did_not_write() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let decoys = [
        "melodia.db",
        "melodia.db-wal",
        "melodia.db-shm",
        "melodia.db.backup-checksum-20260803-015933",
        "melodia-vNOPE.db",
        "melodia-vNOPE.db.tmp",
        "notes.txt",
    ];
    for name in decoys {
        std::fs::write(tmp.path().join(name), b"x")?;
    }
    for version in 1..=5 {
        std::fs::write(tmp.path().join(file_name(version)), b"x")?;
    }

    prune(tmp.path());

    for name in decoys {
        assert!(tmp.path().join(name).exists(), "prune deleted {name}");
    }
    Ok(())
}

/// Loose backups from the version that kept them beside the live database come
/// under the policy rather than being orphaned by it — including the one written
/// before any migration had been applied, which maps to version 0 and is
/// therefore the first thing retention retires.
///
/// The live database sits in the very directory this sweep walks, and its name
/// is the legacy prefix minus one segment, so the decoys matter more here than
/// they do for `prune`: a matcher loosened to `melodia.db.` would move the
/// library out from under the running app and every other assertion would still
/// pass.
#[test]
fn legacy_backups_are_adopted_and_then_pruned() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let data_dir = tmp.path();
    let backups_dir = data_dir.join("backups");
    std::fs::create_dir_all(&backups_dir)?;

    let legacy = [
        "melodia.db.pre-migration-backup",
        "melodia.db.backup-v20260514000000",
        "melodia.db.backup-v20260612000000",
        "melodia.db.backup-v20260705000000",
        "melodia.db.backup-v20260708000000",
        "melodia.db.backup-v20260802000000",
    ];
    for name in legacy {
        std::fs::write(data_dir.join(name), b"x")?;
    }
    // The database the app is running on, plus the sidecars an unclean exit
    // leaves beside it.
    let live = ["melodia.db", "melodia.db-wal", "melodia.db-shm"];
    for name in live {
        std::fs::write(data_dir.join(name), b"x")?;
    }
    // Made by hand, so neither ours to move nor ours to delete.
    let manual = data_dir.join("melodia.db.backup-checksum-20260803-015933");
    std::fs::write(&manual, b"x")?;
    // Adopted on an earlier launch. Renaming onto an existing path fails on
    // Windows, so the loose copy of that same version is dropped and this one
    // stands — the branch that deletes rather than moves.
    let already = backups_dir.join(file_name(20_260_802_000_000));
    std::fs::write(&already, b"the copy already in backups")?;

    maintain(data_dir, &backups_dir);

    for name in legacy {
        assert!(!data_dir.join(name).exists(), "{name} was left loose");
    }
    for name in live {
        assert!(data_dir.join(name).exists(), "the sweep took {name}");
    }
    assert!(manual.exists(), "a hand-made backup was swept up");
    assert_eq!(
        std::fs::read(&already)?,
        b"the copy already in backups",
        "adoption overwrote the copy already under the policy",
    );

    let kept = entry_names(&backups_dir)?;
    assert_eq!(kept.len(), MAX_BACKUPS);
    assert_eq!(
        kept,
        [
            "melodia-v20260705000000.db",
            "melodia-v20260708000000.db",
            "melodia-v20260802000000.db",
        ],
    );
    Ok(())
}

/// `VACUUM INTO` fills its target incrementally, so a crash part-way leaves a
/// partial file. Staging it under `.tmp` is what keeps that file from wearing a
/// name that reads as a complete backup — which `create` would then skip over
/// forever, and a user could restore from.
#[tokio::test]
async fn an_interrupted_backup_leaves_nothing_to_restore_from() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = open_db(tmp.path()).await?;

    let retried = paths.backups_dir.join("melodia-v7.db.tmp");
    let abandoned = paths.backups_dir.join("melodia-v9.db.tmp");
    std::fs::write(&retried, b"not a database")?;
    std::fs::write(&abandoned, b"not a database")?;

    create(db.write(), &paths.backups_dir, 7).await?;
    db.close().await;
    assert!(!retried.exists(), "create wrote around the staged copy");

    // Nobody retried version 9, so retention is what collects it — and it never
    // counted toward the retention limit on the way.
    prune(&paths.backups_dir);
    assert_eq!(entry_names(&paths.backups_dir)?, ["melodia-v7.db"]);
    Ok(())
}

/// A failed migration is rolled back, so a re-attempt finds the database at the
/// version the existing file already captured. Re-copying it would be identical
/// bytes, on every launch, for as long as the migration keeps failing.
#[tokio::test]
async fn create_skips_a_version_it_already_has() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = open_db(tmp.path()).await?;

    let existing = paths.backups_dir.join(file_name(3));
    std::fs::write(&existing, b"the earlier attempt")?;

    create(db.write(), &paths.backups_dir, 3).await?;
    db.close().await;

    assert_eq!(std::fs::read(&existing)?, b"the earlier attempt");
    Ok(())
}
