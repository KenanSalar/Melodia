use crate::database::queries;
use crate::database::queries::fixtures::{execute, insert_test_track};
use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_execute_on, chunked_in_query, init_database, sql,
};
use melodia_core::config::Paths;
use melodia_core::error::{AppError, MigrationError};

use super::strip_windows_verbatim_paths;

/// The ids among `ids` that name a track, as `chunked_in_query` reads them.
async fn ids_found(db: &DbPool, ids: Vec<i64>) -> Result<Vec<i64>, AppError> {
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |ph| format!("SELECT id FROM tracks WHERE id IN ({ph})"))
    })
    .await
}

#[test]
fn max_binds_per_statement_is_999() {
    assert_eq!(MAX_BINDS_PER_STATEMENT, 999);
}

#[tokio::test]
async fn chunked_in_query_empty_input() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = ids_found(&db, Vec::new()).await?;
    assert!(result.is_empty());
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_small_set() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    // Insert a folder + track so we have data to query
    queries::folder::insert_folder(&db, "/test", true).await?;

    let track_id =
        insert_test_track(&db, "/test/song.mp3", "Test Song", "Test Artist", "Test Album", "Rock")
            .await?;

    let ids = vec![track_id, 99999]; // one real, one missing
    let result = ids_found(&db, ids).await?;

    assert_eq!(result, [track_id]);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_single_item() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;
    let track_id = insert_test_track(&db, "/test/a.mp3", "A", "Art", "Alb", "Pop").await?;

    let result = ids_found(&db, vec![track_id]).await?;

    assert_eq!(result, [track_id]);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_preserves_all_results() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;

    let mut track_ids = Vec::new();
    for i in 0..5 {
        let id = insert_test_track(
            &db,
            &format!("/test/track_{i}.mp3"),
            &format!("Track {i}"),
            "Artist",
            "Album",
            "Rock",
        )
        .await?;
        track_ids.push(id);
    }

    let result = ids_found(&db, track_ids.clone()).await?;

    assert_eq!(result.len(), 5);
    for id in &track_ids {
        assert!(result.contains(id), "missing track id {id}");
    }
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_large_set_splits_correctly() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;

    // Insert more tracks than MAX_BINDS_PER_STATEMENT to test chunking
    // Use a smaller number that still validates the chunking logic
    let count = 50;
    let mut track_ids = Vec::with_capacity(count);
    for i in 0..count {
        let id = insert_test_track(
            &db,
            &format!("/test/track_{i}.mp3"),
            &format!("Track {i}"),
            "Artist",
            "Album",
            "Rock",
        )
        .await?;
        track_ids.push(id);
    }

    let result = ids_found(&db, track_ids).await?;

    assert_eq!(result.len(), count);
    Ok(())
}

/// One past the bind budget, so the statement runs as two. The count answered is every chunk's:
/// a scan's purge decides from it whether anything changed.
#[tokio::test]
async fn chunked_in_execute_on_counts_every_row_across_its_chunks() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let names: Vec<String> = (0..=MAX_BINDS_PER_STATEMENT).map(|n| format!("item {n}")).collect();
    let inserted = u64::try_from(names.len()).ok();

    let deleted = db
        .transaction(move |tx| {
            sql::execute(tx, "CREATE TEMP TABLE items (name TEXT)", ())?;
            for name in &names {
                sql::execute(tx, "INSERT INTO items (name) VALUES (?)", (name,))?;
            }
            chunked_in_execute_on(tx, &names, |placeholders| {
                format!("DELETE FROM items WHERE name IN ({placeholders})")
            })
        })
        .await?;

    assert_eq!(Some(deleted), inserted);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_with_string_binds() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;
    insert_test_track(&db, "/test/song.mp3", "Song", "Art", "Alb", "Pop").await?;

    let paths = vec!["/test/song.mp3".to_owned(), "/test/missing.mp3".to_owned()];
    let result: Vec<i64> = db
        .read(move |conn| {
            chunked_in_query(conn, &paths, |ph| {
                format!("SELECT id FROM tracks WHERE file_path IN ({ph})")
            })
        })
        .await?;

    assert_eq!(result.len(), 1);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_all_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = ids_found(&db, vec![9999_i64, 8888, 7777]).await?;
    assert!(result.is_empty());
    Ok(())
}

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
