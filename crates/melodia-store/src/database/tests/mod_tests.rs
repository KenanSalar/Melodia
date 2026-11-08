use std::path::Path;

use crate::database::queries;
use crate::database::queries::fixtures::{execute, fetch_one, insert_test_track};
use crate::database::{DbPool, init_database, sql};
use melodia_core::config::Paths;
use melodia_core::error::{AppError, MigrationError};

use super::strip_windows_verbatim_paths;

const NEWER_BUILDS_MIGRATION: i64 = 99_991_231_000_000;

/// **An older build refuses a database a newer one migrated**: opening it would run this build's
/// queries over a schema it has never seen.
#[tokio::test]
async fn a_database_a_newer_build_migrated_refuses_to_open() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let db = init_database(&paths).await?;
    execute(
        &db,
        &format!(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES ({NEWER_BUILDS_MIGRATION}, 'from a newer build', TRUE, x'00', 1)"
        ),
    )
    .await?;
    db.close().await;

    let reopened = init_database(&paths).await;

    assert!(
        matches!(
            reopened,
            Err(AppError::Migration(MigrationError::Unknown(NEWER_BUILDS_MIGRATION)))
        ),
        "{:?}",
        reopened.err()
    );
    Ok(())
}

/// A lone `CREATE INDEX IF NOT EXISTS`, so deleting its history row stages an upgrade that re-runs
/// harmlessly. Shipped, so it stays that shape.
const REPEATABLE_MIGRATION: i64 = 20_260_708_000_000;

const LEAVE_FREE_PAGES: &str = "\
    CREATE TABLE ballast (payload BLOB); \
    INSERT INTO ballast VALUES (zeroblob(1048576)); \
    DROP TABLE ballast;";

const FREE_PAGES: &str = "PRAGMA freelist_count";

/// A migrated database holding free pages, which only a VACUUM gives back.
async fn database_with_free_pages(dir: &Path) -> Result<(Paths, DbPool), AppError> {
    let paths = Paths::rooted_at(dir.to_path_buf());
    paths.create_dirs()?;
    let db = init_database(&paths).await?;
    execute(&db, LEAVE_FREE_PAGES).await?;
    Ok((paths, db))
}

#[tokio::test]
async fn a_boot_that_applies_a_migration_compacts_the_file() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = database_with_free_pages(tmp.path()).await?;
    let rewind = format!("DELETE FROM _sqlx_migrations WHERE version = {REPEATABLE_MIGRATION}");
    execute(&db, &rewind).await?;
    db.close().await;

    let upgraded = init_database(&paths).await?;

    assert_eq!(fetch_one::<i64>(&upgraded, FREE_PAGES).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_boot_with_nothing_to_migrate_leaves_the_file_uncompacted() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (paths, db) = database_with_free_pages(tmp.path()).await?;
    db.close().await;

    let reopened = init_database(&paths).await?;

    assert!(
        fetch_one::<i64>(&reopened, FREE_PAGES).await? > 0,
        "an ordinary launch rewrote the whole file"
    );
    Ok(())
}

// ---- strip_windows_verbatim_paths -----------------------------------------

async fn insert_folder_raw(db: &DbPool, path: &str) -> Result<(), AppError> {
    let path = path.to_owned();
    db.write(move |conn| {
        sql::execute(
            conn,
            "INSERT INTO folders (path, is_enabled, added_at) \
             VALUES (?, 1, '2026-01-01T00:00:00+00:00')",
            (path,),
        )
    })
    .await?;
    Ok(())
}

async fn folder_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    db.read(|conn| sql::fetch_all(conn, "SELECT path FROM folders ORDER BY id", ())).await
}

#[tokio::test]
async fn strip_verbatim_drive_form() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder_raw(&db, r"\\?\C:\Music").await?;
    insert_folder_raw(&db, r"\\?\D:\Library\Albums").await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    assert_eq!(folder_paths(&db).await?, vec![r"C:\Music", r"D:\Library\Albums"]);
    Ok(())
}

#[tokio::test]
async fn strip_verbatim_unc_form() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder_raw(&db, r"\\?\UNC\nas\share\music").await?;
    insert_folder_raw(&db, r"\\?\UNC\server\public\Albums\Rock").await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    assert_eq!(
        folder_paths(&db).await?,
        vec![r"\\nas\share\music", r"\\server\public\Albums\Rock"],
    );
    Ok(())
}

#[tokio::test]
async fn strip_verbatim_leaves_clean_paths_untouched() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder_raw(&db, r"C:\Already\Clean").await?;
    insert_folder_raw(&db, r"\\server\share\plain").await?;
    insert_folder_raw(&db, "/posix/style/path").await?;
    let before = folder_paths(&db).await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    assert_eq!(folder_paths(&db).await?, before);
    Ok(())
}

#[tokio::test]
async fn strip_verbatim_keeps_prefix_when_result_would_exceed_max_path() -> Result<(), AppError> {
    // Stripping `\\?\` would leave a 260+ char path that can't be opened
    // without the verbatim prefix. `dunce::canonicalize` also keeps the
    // prefix in this case, so the strip must too. `tail` is sized so the
    // post-strip length (`C:\` + tail) is exactly 260.
    let db = DbPool::test_pool().await?;
    let tail = "a".repeat(257);
    let long_drive = format!(r"\\?\C:\{tail}"); // LENGTH = 4 + 3 + 257 = 264
    let long_unc = format!(r"\\?\UNC\srv\share\{tail}"); // LENGTH = 8 + 11 + 257 = 276 (post-strip 270)
    insert_folder_raw(&db, &long_drive).await?;
    insert_folder_raw(&db, &long_unc).await?;
    let before = folder_paths(&db).await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    assert_eq!(folder_paths(&db).await?, before);
    Ok(())
}

#[tokio::test]
async fn strip_verbatim_is_idempotent() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    insert_folder_raw(&db, r"\\?\C:\Music").await?;
    insert_folder_raw(&db, r"\\?\UNC\nas\share\music").await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    let after_first = folder_paths(&db).await?;
    db.transaction(strip_windows_verbatim_paths).await?;
    assert_eq!(folder_paths(&db).await?, after_first);
    Ok(())
}

#[tokio::test]
async fn strip_verbatim_normalizes_track_file_paths() -> Result<(), AppError> {
    // Tracks share the same SQL shape as folders; one mixed-form spot-check
    // guards against a future regression where only one table is updated.
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/host", true).await?;
    insert_test_track(&db, r"\\?\C:\Music\drive.mp3", "Drive", "A", "Alb", "Pop").await?;
    insert_test_track(&db, r"\\?\UNC\nas\share\unc.mp3", "UNC", "A", "Alb", "Pop").await?;
    insert_test_track(&db, r"D:\Already\Clean.mp3", "Clean", "A", "Alb", "Pop").await?;

    db.transaction(strip_windows_verbatim_paths).await?;

    let paths: Vec<String> = db
        .read(|conn| sql::fetch_all(conn, "SELECT file_path FROM tracks ORDER BY id", ()))
        .await?;
    assert_eq!(
        paths,
        vec![
            r"C:\Music\drive.mp3".to_owned(),
            r"\\nas\share\unc.mp3".to_owned(),
            r"D:\Already\Clean.mp3".to_owned(),
        ],
    );
    Ok(())
}
