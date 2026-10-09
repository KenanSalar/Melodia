//! Which branch a watcher event takes, and what each one is allowed to write.
//!
//! Two threads run through them. The first is *where the stored mtime comes from*: a re-point
//! writes `date_modified` but not `file_size` / `file_hash`, so it stores the mtime it is handed
//! rather than a fresh `stat`. The second is *what a move costs*. Rating, play count and favourite
//! are not derived from the file, so a batch that reads a relocation as an import loses them and
//! no rescan brings them back. These drive that decision directly, with nothing on disk; the
//! reconcile task's own tests drive it against real bytes, because a delete and a create only meet
//! at that level.

use std::collections::HashMap;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::database::queries;
use crate::database::queries::fixtures::{insert_test_track, make_test_metadata};

/// An mtime no file on disk can have, so a value that reaches the row proves it
/// came from what the handler was handed and not from a fresh `stat`.
const SENTINEL_MTIME: &str = "2001-02-03T04:05:06+00:00";

/// A pool with `dir` registered as the one library folder, so `find_folder_for_path` answers
/// for everything under it and refuses everything beside it.
async fn seed_folder(dir: &Path) -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, &dir.to_string_lossy(), true).await?;
    Ok(db)
}

/// [`seed_folder`] plus one track row at `<dir>/from.mp3` — the row a rename has to re-point.
/// Its `file_hash` is `make_test_metadata("Before")`'s, which is what lets a test collide with
/// it by hash without putting a byte on disk.
async fn seed_folder_with_track(dir: &Path) -> Result<(DbPool, i64), AppError> {
    let db = seed_folder(dir).await?;

    // Joined, not interpolated: the handlers look the row up by the path `Path::join` gives
    // them, so a hand-spelled `/` seeds a row Windows can never match.
    let from = dir.join("from.mp3").to_string_lossy().into_owned();
    let id = insert_test_track(&db, &from, "Before", "Artist A", "Album One", "Rock").await?;
    Ok((db, id))
}

/// Read back the columns a re-point is allowed to touch.
async fn track_location(db: &DbPool, id: i64) -> Result<(String, Option<String>), AppError> {
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT file_path, date_modified FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(db.read())
            .await?;
    Ok(row)
}

async fn track_hash(db: &DbPool, id: i64) -> Result<Option<String>, AppError> {
    let hash: Option<String> = sqlx::query_scalar("SELECT file_hash FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    Ok(hash)
}

async fn track_id_at(db: &DbPool, path: &Path) -> Result<Option<i64>, AppError> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE file_path = ?")
        .bind(path.to_string_lossy().into_owned())
        .fetch_optional(db.read())
        .await?;
    Ok(id)
}

async fn track_count(db: &DbPool) -> Result<i64, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    Ok(count)
}

/// A batch whose one move candidate is the row `id`, last seen at `vanished`.
fn batch_with_candidate(hash: &str, id: i64, vanished: &Path) -> Batch {
    let candidate = (id, vanished.to_string_lossy().into_owned());
    Batch { moved: HashMap::from([(hash.to_owned(), candidate)]), ..Batch::default() }
}

#[tokio::test]
async fn a_rename_stores_the_mtime_it_is_handed() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let to = tmp.path().join("to.mp3");
    std::fs::write(&to, b"audio")?;

    // The file on disk was just written, so its real mtime is ~now. If the
    // handler re-`stat`s instead of storing what it was handed, the row gets that.
    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &tmp.path().join("from.mp3"),
        &to,
        Some(&make_test_metadata("After")),
        Some(SENTINEL_MTIME),
        &mut Batch::default(),
    )
    .await?;
    tx.commit().await?;

    assert!(changed, "an existing row at `from` must be re-pointed");

    let (path, date_modified) = track_location(&db, id).await?;
    assert_eq!(path, to.to_string_lossy());
    assert_eq!(
        date_modified.as_deref(),
        Some(SENTINEL_MTIME),
        "the mtime must be the one the batch carried, not a second stat"
    );
    Ok(())
}

#[tokio::test]
async fn a_created_file_inside_a_library_folder_lands_as_a_row() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(tmp.path()).await?;

    let path = tmp.path().join("new.mp3");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_created(&mut tx, &path, &make_test_metadata("New"), &mut Batch::default()).await?;
    tx.commit().await?;

    assert!(changed);
    assert!(track_id_at(&db, &path).await?.is_some());
    Ok(())
}

/// The watcher and the boot rescan both see a file that is already in the library, so the
/// exists-by-path guard is the common case rather than an edge one.
#[tokio::test]
async fn a_created_path_already_in_the_library_is_not_inserted_twice() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let path = tmp.path().join("from.mp3");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_created(&mut tx, &path, &make_test_metadata("Again"), &mut Batch::default()).await?;
    tx.commit().await?;

    assert!(!changed, "a path already in the library is not a change");
    assert_eq!(track_count(&db).await?, 1);
    assert_eq!(track_id_at(&db, &path).await?, Some(id));
    Ok(())
}

#[tokio::test]
async fn a_created_file_outside_every_library_folder_is_skipped() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(&tmp.path().join("music")).await?;

    let outside = tmp.path().join("elsewhere.mp3");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_created(&mut tx, &outside, &make_test_metadata("Stray"), &mut Batch::default())
            .await?;
    tx.commit().await?;

    assert!(!changed);
    assert_eq!(track_count(&db).await?, 0, "a file nobody asked us to watch is not a track");
    Ok(())
}

/// The re-point arm, and the reason the candidate is consumed: two files with identical bytes
/// can both arrive as `Created` in one batch, and only one of them is the row's new home. The
/// second has to insert.
#[tokio::test]
async fn a_created_file_matching_a_vanished_row_repoints_it_once() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let meta = make_test_metadata("Before");
    let mut batch = batch_with_candidate(&meta.file_hash, id, &tmp.path().join("from.mp3"));

    let moved = tmp.path().join("moved.mp3");
    let twin = tmp.path().join("twin.mp3");

    let mut tx = db.write().begin().await?;
    let repointed = handle_created(&mut tx, &moved, &meta, &mut batch).await?;
    let inserted = handle_created(&mut tx, &twin, &meta, &mut batch).await?;
    tx.commit().await?;

    assert!(repointed);
    assert!(inserted);
    assert!(batch.moved.is_empty(), "a consumed candidate must not re-point a second file");
    assert_eq!(track_id_at(&db, &moved).await?, Some(id), "the move keeps the original row");
    assert_eq!(track_count(&db).await?, 2, "the twin is a new file, not the same one again");
    Ok(())
}

/// A move can land somewhere the library does not watch, and the candidate has to survive that:
/// one batch can carry several `Created` events for the same bytes, and refusing the first is
/// not a reason to import the second as a stranger.
#[tokio::test]
async fn a_move_out_of_the_watched_tree_leaves_the_candidate_alone() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = tmp.path().join("music");
    let (db, id) = seed_folder_with_track(&music).await?;

    let meta = make_test_metadata("Before");
    let mut batch = batch_with_candidate(&meta.file_hash, id, &music.join("from.mp3"));

    let outside = tmp.path().join("outside.mp3");
    let inside = music.join("moved.mp3");

    let mut tx = db.write().begin().await?;
    let refused = handle_created(&mut tx, &outside, &meta, &mut batch).await?;
    assert!(!refused);
    assert!(
        batch.moved.contains_key(&meta.file_hash),
        "refusing a path outside the library must not spend the row's one chance to be found"
    );

    let repointed = handle_created(&mut tx, &inside, &meta, &mut batch).await?;
    tx.commit().await?;

    assert!(repointed);
    assert_eq!(track_id_at(&db, &inside).await?, Some(id));
    assert_eq!(track_count(&db).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_rename_from_outside_the_library_lands_as_a_new_row() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(tmp.path()).await?;

    let from = tmp.path().join("unknown.mp3");
    let to = tmp.path().join("landed.mp3");
    let meta = make_test_metadata("Fresh");

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &from,
        &to,
        Some(&meta),
        meta.date_modified.as_deref(),
        &mut Batch::default(),
    )
    .await?;
    tx.commit().await?;

    assert!(changed);
    assert!(track_id_at(&db, &to).await?.is_some(), "a rename in from outside is an import");
    Ok(())
}

/// A rename out of the library is not a delete. The row is left pointing at a path that no
/// longer exists, and the folder-removal path is what retires it.
#[tokio::test]
async fn a_rename_out_of_the_library_writes_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let music = tmp.path().join("music");
    let (db, id) = seed_folder_with_track(&music).await?;

    let from = music.join("from.mp3");
    let to = tmp.path().join("outside.mp3");
    let meta = make_test_metadata("Before");

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &from,
        &to,
        Some(&meta),
        meta.date_modified.as_deref(),
        &mut Batch::default(),
    )
    .await?;
    tx.commit().await?;

    assert!(!changed);
    let (path, _) = track_location(&db, id).await?;
    assert_eq!(
        path,
        from.to_string_lossy(),
        "the row stays put rather than following the file out"
    );
    Ok(())
}

#[tokio::test]
async fn a_rename_with_neither_a_row_nor_metadata_writes_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(tmp.path()).await?;

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &tmp.path().join("unknown.mp3"),
        &tmp.path().join("gone.mp3"),
        None,
        None,
        &mut Batch::default(),
    )
    .await?;
    tx.commit().await?;

    assert!(!changed);
    assert_eq!(track_count(&db).await?, 0);
    Ok(())
}

/// A modify that changes the hash is a re-tag, not a move: the row keeps its identity and takes
/// the new hash, which is what stops the next scan reading it as a stranger with familiar bytes.
#[tokio::test]
async fn a_modify_rewrites_the_hash_in_place() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let retagged = make_test_metadata("After");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_modified(&mut tx, &tmp.path().join("from.mp3"), &retagged, &mut Batch::default())
            .await?;
    tx.commit().await?;

    assert!(changed);
    assert_eq!(track_count(&db).await?, 1, "a re-tag must not fork the row");
    assert_eq!(track_hash(&db, id).await?, Some(retagged.file_hash));
    Ok(())
}

/// The watcher can report a write to a file that was never scanned, so `handle_modified` owes
/// the same answer `handle_created` would have given.
#[tokio::test]
async fn a_modify_of_a_path_the_library_does_not_know_inserts_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(tmp.path()).await?;

    let path = tmp.path().join("unscanned.mp3");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_modified(&mut tx, &path, &make_test_metadata("Late"), &mut Batch::default()).await?;
    tx.commit().await?;

    assert!(changed);
    assert!(track_id_at(&db, &path).await?.is_some());
    Ok(())
}
