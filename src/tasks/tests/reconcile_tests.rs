//! Tests for the watcher's re-point paths. These pin *where the stored mtime
//! comes from*: `update_track_location` writes `date_modified` but not
//! `file_size` / `file_hash`, so a mtime re-read at write time would land in
//! the row beside the previous scan's size — and an in-place tag edit that
//! happened not to change the size would then read as current to
//! `scanner::track_is_current` forever. The mtime must come from the
//! `ExtractedMetadata` the batch already produced for that path.

use std::collections::HashMap;

use tempfile::TempDir;

use super::*;
use crate::database::DbPool;
use crate::database::queries::tests::helpers::{insert_test_track, make_test_metadata};
use crate::error::AppError;

/// An mtime no file on disk can have, so a value that reaches the row proves it
/// came from the `ExtractedMetadata` and not from a fresh `stat`.
const SENTINEL_MTIME: &str = "2001-02-03T04:05:06+00:00";

/// A pool with `dir` registered as a library folder (id 1) and one track row at
/// `<dir>/from.mp3` — the row a rename has to re-point.
async fn seed_folder_with_track(dir: &std::path::Path) -> Result<(DbPool, i64), AppError> {
    let db = DbPool::test_pool().await?;
    let folder = dir.to_string_lossy().into_owned();
    queries::folder::insert_folder(&db, &folder, true).await?;

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
    let on_disk = crate::media::ingest::metadata::extract_date_modified(&to);
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
