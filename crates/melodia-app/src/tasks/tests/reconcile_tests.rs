//! Tests for the watcher's reconcile pass: which branch a file event takes, and what each one
//! is allowed to write.
//!
//! Two threads run through them. The first is *where the stored mtime comes from*:
//! `update_track_location` writes `date_modified` but not `file_size` / `file_hash`, so a mtime
//! re-read at write time would land in the row beside the previous scan's size, and an in-place
//! tag edit that happened not to change the size would then read as current to
//! `scanner::track_is_current` forever. The mtime must come from the `ExtractedMetadata` the
//! batch already produced for that path.
//!
//! The second is *what a move costs*. Rating, play count and favourite are not derived from the
//! file, so a reconcile that reads a relocation as an import loses them and no rescan brings
//! them back. The handler tests drive that decision directly; the `process_batch` tests at the
//! bottom drive it against real bytes, because a delete and a create only meet at that level.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use melodia_artwork::media::image::artwork;
use melodia_core::error::AppError;
use melodia_store::database::DbPool;
use melodia_store::database::queries::fixtures::{insert_test_track, make_test_metadata};
use melodia_testkit::ASSETS_DIR;

/// An mtime no file on disk can have, so a value that reaches the row proves it
/// came from the `ExtractedMetadata` and not from a fresh `stat`.
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

/// The two columns a move must not cost the user.
async fn track_stats(db: &DbPool, id: i64) -> Result<(i64, i64), AppError> {
    let row: (i64, i64) = sqlx::query_as("SELECT rating, play_count FROM tracks WHERE id = ?")
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

async fn albums_named(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(count)
}

// --- handler-level: the decision, with nothing on disk ---------------------------------------

#[tokio::test]
async fn renamed_stores_the_mtime_from_the_extracted_metadata() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let to = tmp.path().join("to.mp3");
    std::fs::write(&to, b"audio")?;

    // The file on disk was just written, so its real mtime is ~now. If the
    // handler re-`stat`s instead of reading `meta`, the row gets that, not this.
    let mut meta = make_test_metadata("After");
    meta.date_modified = Some(SENTINEL_MTIME.to_owned());

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &tmp.path().join("from.mp3"),
        &to,
        Some(&meta),
        &mut HashMap::new(),
    )
    .await?;
    tx.commit().await?;

    assert!(changed, "an existing row at `from` must be re-pointed");

    let (path, date_modified) = track_location(&db, id).await?;
    assert_eq!(path, to.to_string_lossy());
    assert_eq!(
        date_modified.as_deref(),
        Some(SENTINEL_MTIME),
        "the mtime must come from the batch's ExtractedMetadata, not a second stat"
    );
    Ok(())
}

/// Extraction can fail (unreadable/corrupt file) or the renamed-to path can
/// vanish before the batch is extracted, leaving `meta` as `None`. That is the
/// one case with nothing in hand, and the only one allowed to `stat`.
#[tokio::test]
async fn renamed_without_metadata_falls_back_to_a_stat() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let (db, id) = seed_folder_with_track(tmp.path()).await?;

    let to = tmp.path().join("to.mp3");
    std::fs::write(&to, b"audio")?;
    let on_disk = melodia_store::media::ingest::metadata::extract_date_modified(&to);
    assert!(on_disk.is_some(), "the temp file must have a readable mtime");

    let mut tx = db.write().begin().await?;
    let changed =
        handle_renamed(&mut tx, &tmp.path().join("from.mp3"), &to, None, &mut HashMap::new())
            .await?;
    tx.commit().await?;

    assert!(changed);

    let (path, date_modified) = track_location(&db, id).await?;
    assert_eq!(path, to.to_string_lossy());
    assert_eq!(date_modified, on_disk);
    assert_ne!(date_modified.as_deref(), Some(SENTINEL_MTIME));
    Ok(())
}

#[tokio::test]
async fn a_created_file_inside_a_library_folder_lands_as_a_row() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let db = seed_folder(tmp.path()).await?;

    let path = tmp.path().join("new.mp3");
    let mut tx = db.write().begin().await?;
    let changed =
        handle_created(&mut tx, &path, &make_test_metadata("New"), &mut HashMap::new()).await?;
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
        handle_created(&mut tx, &path, &make_test_metadata("Again"), &mut HashMap::new()).await?;
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
        handle_created(&mut tx, &outside, &make_test_metadata("Stray"), &mut HashMap::new())
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
    let vanished = tmp.path().join("from.mp3").to_string_lossy().into_owned();
    let mut candidates = HashMap::from([(meta.file_hash.clone(), (id, vanished))]);

    let moved = tmp.path().join("moved.mp3");
    let twin = tmp.path().join("twin.mp3");

    let mut tx = db.write().begin().await?;
    let repointed = handle_created(&mut tx, &moved, &meta, &mut candidates).await?;
    let inserted = handle_created(&mut tx, &twin, &meta, &mut candidates).await?;
    tx.commit().await?;

    assert!(repointed);
    assert!(inserted);
    assert!(candidates.is_empty(), "a consumed candidate must not re-point a second file");
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
    let vanished = music.join("from.mp3").to_string_lossy().into_owned();
    let mut candidates = HashMap::from([(meta.file_hash.clone(), (id, vanished))]);

    let outside = tmp.path().join("outside.mp3");
    let inside = music.join("moved.mp3");

    let mut tx = db.write().begin().await?;
    let refused = handle_created(&mut tx, &outside, &meta, &mut candidates).await?;
    assert!(!refused);
    assert!(
        candidates.contains_key(&meta.file_hash),
        "refusing a path outside the library must not spend the row's one chance to be found"
    );

    let repointed = handle_created(&mut tx, &inside, &meta, &mut candidates).await?;
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

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &from,
        &to,
        Some(&make_test_metadata("Fresh")),
        &mut HashMap::new(),
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

    let mut tx = db.write().begin().await?;
    let changed = handle_renamed(
        &mut tx,
        &from,
        &to,
        Some(&make_test_metadata("Before")),
        &mut HashMap::new(),
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
        &mut HashMap::new(),
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
        handle_modified(&mut tx, &tmp.path().join("from.mp3"), &retagged, &mut HashMap::new())
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
        handle_modified(&mut tx, &path, &make_test_metadata("Late"), &mut HashMap::new()).await?;
    tx.commit().await?;

    assert!(changed);
    assert!(track_id_at(&db, &path).await?.is_some());
    Ok(())
}

// --- batch level: where the delete and the create meet ---------------------------------------

/// A `Paths` rooted under `tmp`, beside rather than inside the watched folder, with its
/// directories made so extraction has somewhere to put artwork.
fn paths_in(tmp: &TempDir) -> Result<Paths, AppError> {
    let paths = Paths::rooted_at(tmp.path().join("data"));
    paths.create_dirs()?;
    Ok(paths)
}

/// Copy a checked-in asset to `dest`. Real bytes, because `process_batch` hashes what is on
/// disk; never the asset itself, which stays read-only.
fn stage(name: &str, dest: &Path) -> Result<(), AppError> {
    std::fs::copy(PathBuf::from(ASSETS_DIR).join(name), dest)?;
    Ok(())
}

/// A library folder holding `silence.mp3`, already ingested through `process_batch` so the row's
/// hash is the file's, with a rating and a play count on it. Returns the folder, the ingested
/// path and the row id.
async fn seed_ingested(
    tmp: &TempDir,
    paths: &Paths,
    cover_cache: &CoverCache,
) -> Result<(DbPool, PathBuf, i64), AppError> {
    let music = tmp.path().join("music");
    std::fs::create_dir_all(&music)?;
    let track = music.join("old.mp3");
    stage("silence.mp3", &track)?;

    let db = seed_folder(&music).await?;
    process_batch(&db, paths, cover_cache, vec![FileEvent::Created(track.clone())]).await?;

    let Some(id) = track_id_at(&db, &track).await? else {
        return Err(AppError::Validation("the seed ingest wrote no row".into()));
    };
    sqlx::query("UPDATE tracks SET rating = 4, play_count = 7 WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;

    Ok((db, track, id))
}

/// A move between filesystems reaches the watcher as a delete plus a create rather than a
/// rename, so both events land in one batch and the row's fate depends on which is applied
/// first. `deduplicate_events` emits from a `HashMap`, so the arrival order is not ours to
/// predict; both are driven here, and both have to keep the row the user rated.
///
/// The rating and the play count are the evidence, not the id: `tracks.id` is a bare
/// `INTEGER PRIMARY KEY`, so an insert after a delete takes `max(rowid) + 1` and hands the
/// replacement row the id the original just freed whenever that was the largest, as it is
/// here. An id check alone passes while the user's state is gone.
#[tokio::test]
async fn a_delete_and_create_of_the_same_bytes_keeps_the_original_row() -> Result<(), AppError> {
    for delete_first in [true, false] {
        let tmp = TempDir::new()?;
        let paths = paths_in(&tmp)?;
        let cover_cache = artwork::new_cover_cache();
        let (db, old, id) = seed_ingested(&tmp, &paths, &cover_cache).await?;

        let new = old.with_file_name("new.mp3");
        std::fs::rename(&old, &new)?;

        let removed = FileEvent::Removed(old.clone());
        let created = FileEvent::Created(new.clone());
        let events = if delete_first {
            vec![removed, created]
        } else {
            vec![created, removed]
        };
        process_batch(&db, &paths, &cover_cache, events).await?;

        assert_eq!(
            track_count(&db).await?,
            1,
            "delete_first={delete_first}: the move must re-point the row, not insert a second"
        );
        assert_eq!(
            track_id_at(&db, &new).await?,
            Some(id),
            "delete_first={delete_first}: the file's new home must be the same row"
        );
        assert_eq!(
            track_stats(&db, id).await?,
            (4, 7),
            "delete_first={delete_first}: rating and play count are not in the file, so a \
             re-import loses them for good"
        );
    }
    Ok(())
}

/// The other reading of one hash matching two paths. Move detection keeps only candidates whose
/// old path is gone, so a copy made beside the original is an import.
#[tokio::test]
async fn a_copy_beside_a_file_that_still_exists_is_a_duplicate() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = paths_in(&tmp)?;
    let cover_cache = artwork::new_cover_cache();
    let (db, original, id) = seed_ingested(&tmp, &paths, &cover_cache).await?;

    let copy = original.with_file_name("copy.mp3");
    std::fs::copy(&original, &copy)?;

    process_batch(&db, &paths, &cover_cache, vec![FileEvent::Created(copy.clone())]).await?;

    assert_eq!(track_count(&db).await?, 2, "the original is still there, so this is a new file");
    assert_eq!(track_id_at(&db, &original).await?, Some(id));
    assert!(track_id_at(&db, &copy).await?.is_some());
    Ok(())
}

/// `prune_orphans` and the album-artwork rollup are gated on the batch having written
/// something, so a batch of events for files nobody tracks pays for neither. Both sides of that
/// gate, since a floor on one of them would pass whichever way the gate was wired.
#[tokio::test]
async fn the_post_batch_sweeps_wait_for_a_real_change() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = paths_in(&tmp)?;
    let cover_cache = artwork::new_cover_cache();

    let music = tmp.path().join("music");
    std::fs::create_dir_all(&music)?;
    let db = seed_folder(&music).await?;

    // A row deleted straight out of the table, so its album is stranded with no tracks and
    // nothing but `prune_orphans` will retire it.
    let stranded = music.join("stranded.mp3").to_string_lossy().into_owned();
    let id = insert_test_track(&db, &stranded, "Gone", "Artist A", "Orphan Album", "Rock").await?;
    sqlx::query("DELETE FROM tracks WHERE id = ?").bind(id).execute(db.write()).await?;

    let untracked = vec![FileEvent::Removed(music.join("never-scanned.mp3"))];
    process_batch(&db, &paths, &cover_cache, untracked).await?;
    assert_eq!(
        albums_named(&db, "Orphan Album").await?,
        1,
        "a batch that wrote nothing must not pay for the full-table sweeps"
    );

    let arrival = music.join("arrival.mp3");
    stage("silence.mp3", &arrival)?;
    process_batch(&db, &paths, &cover_cache, vec![FileEvent::Created(arrival)]).await?;
    assert_eq!(
        albums_named(&db, "Orphan Album").await?,
        0,
        "one real change is what the sweeps are waiting for"
    );
    Ok(())
}
