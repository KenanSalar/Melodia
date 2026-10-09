//! Tests for the watcher's reconcile pass: what it hands the store, and what a batch of real
//! bytes does to the library.
//!
//! Two threads run through them. The first is *where a rename's mtime comes from*: a re-point
//! writes `date_modified` but not `file_size` / `file_hash`, so the mtime has to be the one
//! extraction already read, never a fresh `stat`. The second is *what a move costs*. Rating, play
//! count and favourite are not derived from the file, so a reconcile that reads a relocation as an
//! import loses them and no rescan brings them back. The store's own tests drive each handler;
//! these drive the batch, because a delete and a create only meet at that level.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use melodia_artwork::media::image::artwork;
use melodia_core::error::AppError;
use melodia_store::database::DbPool;
use melodia_store::database::queries::fixtures::{insert_test_track, make_test_metadata};
use melodia_testkit::ASSETS_DIR;

/// An mtime no file on disk can have, so a value that reaches the change proves it
/// came from the `ExtractedMetadata` and not from a fresh `stat`.
const SENTINEL_MTIME: &str = "2001-02-03T04:05:06+00:00";

/// A pool with `dir` registered as the one library folder.
async fn seed_folder(dir: &Path) -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, &dir.to_string_lossy(), true).await?;
    Ok(db)
}

/// The two columns a move must not cost the user.
async fn track_stats(db: &DbPool, id: i64) -> Result<(i64, i64), AppError> {
    let row: (i64, i64) = sqlx::query_as("SELECT rating, play_count FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    Ok(row)
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

// --- the hand-off: the mtime a rename carries to the store -----------------------------------

/// The mtime the change for a rename onto `to` carries, given what extraction read there.
fn rename_mtime(to: &Path, meta: Option<ExtractedMetadata>) -> Result<Option<String>, AppError> {
    let event = FileEvent::Renamed { from: to.with_file_name("from.mp3"), to: to.to_path_buf() };
    let metadata: HashMap<PathBuf, ExtractedMetadata> =
        meta.map(|meta| HashMap::from([(to.to_path_buf(), meta)])).unwrap_or_default();

    let changes = watched_changes(vec![event], metadata);

    let [WatchedChange::Renamed { date_modified, .. }] = changes.as_slice() else {
        return Err(AppError::Validation("a rename must reach the store as one rename".into()));
    };
    Ok(date_modified.clone())
}

#[test]
fn a_rename_carries_the_mtime_from_the_extracted_metadata() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let to = tmp.path().join("to.mp3");
    std::fs::write(&to, b"audio")?;

    // The file on disk was just written, so its real mtime is ~now. A change that
    // re-`stat`s instead of reading `meta` carries that, not this.
    let mut meta = make_test_metadata("After");
    meta.date_modified = Some(SENTINEL_MTIME.to_owned());

    assert_eq!(
        rename_mtime(&to, Some(meta))?.as_deref(),
        Some(SENTINEL_MTIME),
        "the mtime must come from the batch's ExtractedMetadata, not a second stat"
    );
    Ok(())
}

/// Extraction can fail (unreadable/corrupt file) or the renamed-to path can
/// vanish before the batch is extracted, leaving no metadata. That is the
/// one case with nothing in hand, and the only one allowed to `stat`.
#[test]
fn a_rename_without_metadata_falls_back_to_a_stat() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let to = tmp.path().join("to.mp3");
    std::fs::write(&to, b"audio")?;
    let on_disk = extract_date_modified(&to);
    assert!(on_disk.is_some(), "the temp file must have a readable mtime");

    assert_eq!(rename_mtime(&to, None)?, on_disk);
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
        let events = if delete_first { vec![removed, created] } else { vec![created, removed] };
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

/// `prune_orphans` and the cover roll-ups are gated on the batch having written
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
