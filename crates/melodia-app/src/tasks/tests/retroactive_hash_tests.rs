//! The backfill for `tracks.file_hash`, which is the column a cross-device move is matched on.
//!
//! A move arrives as a delete plus a create, and `process_batch` carries the user's rating, play
//! count and favourite across it by resolving candidate hashes. A row this pass silently skips
//! has none, so the move becomes a re-import and the row comes back blank — and the id is no help
//! in noticing, `tracks.id` being a bare `INTEGER PRIMARY KEY` that an insert will happily re-use.

use std::collections::HashMap;

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;
use melodia_store::database::queries::fixtures::insert_test_track;

/// The `date_modified` every seeded row starts with, so a value still equal to it after the pass
/// means nothing was written rather than that the file happened to match.
const STALE_MTIME: &str = "2024-01-01T00:00:00+00:00";

/// A library of rows whose files hold exactly the bytes given. Hashes are nulled after the insert
/// — the seeding fixture writes one of its own, and an unhashed row is what this pass is for.
async fn library(files: &[(&str, &[u8])]) -> Result<(DbPool, TempDir), AppError> {
    let tmp = TempDir::new()?;
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, &tmp.path().to_string_lossy(), true).await?;

    for (name, bytes) in files {
        let path = tmp.path().join(name);
        std::fs::write(&path, bytes)?;
        insert_test_track(&db, &path.to_string_lossy(), name, "Artist", "Album", "Rock").await?;
    }
    sqlx::query("UPDATE tracks SET file_hash = NULL").execute(db.write()).await?;
    Ok((db, tmp))
}

/// Every row's hash and mtime, keyed by the file name the row points at.
async fn rows(db: &DbPool) -> Result<HashMap<String, (Option<String>, Option<String>)>, AppError> {
    let rows: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT file_name, file_hash, date_modified FROM tracks")
            .fetch_all(db.read())
            .await?;
    Ok(rows.into_iter().map(|(name, hash, mtime)| (name, (hash, mtime))).collect())
}

async fn hash_of(db: &DbPool, name: &str) -> Result<Option<String>, AppError> {
    Ok(rows(db).await?.get(name).and_then(|(hash, _)| hash.clone()))
}

/// Each row has to get the hash of the file *it* names. The pass hashes in parallel and writes
/// back as `(id, hash, mtime)` tuples, so a pairing that slips is invisible in the column — every
/// row is populated and every value is a real hash — and turns the next move into a re-import.
#[tokio::test]
async fn an_unhashed_row_gets_the_hash_of_its_own_file() -> Result<(), AppError> {
    let (db, _tmp) = library(&[
        ("first.mp3", b"the same bytes"),
        ("second.mp3", b"different bytes entirely"),
        ("third.mp3", b"the same bytes"),
    ])
    .await?;

    hash_unhashed_tracks(&db).await?;

    let first = hash_of(&db, "first.mp3").await?;
    let third = hash_of(&db, "third.mp3").await?;
    assert!(first.is_some(), "an unhashed row must come back hashed");
    assert_eq!(first, third, "identical bytes are what a move is recognised by");
    assert_ne!(first, hash_of(&db, "second.mp3").await?, "and different bytes must not collide");
    Ok(())
}

/// The hash and the mtime are written together or the row is worse off than before: a fresh hash
/// beside the mtime the row already had is the one state `track_is_current` reads as current and
/// no re-scan repairs.
#[tokio::test]
async fn the_backfilled_mtime_comes_off_the_file() -> Result<(), AppError> {
    let (db, _tmp) = library(&[("song.mp3", b"bytes")]).await?;

    hash_unhashed_tracks(&db).await?;

    let mtime = rows(&db).await?.get("song.mp3").and_then(|(_, mtime)| mtime.clone());
    assert_ne!(mtime.as_deref(), Some(STALE_MTIME), "the row kept the mtime it was seeded with");
    Ok(())
}

/// A library always has a few rows whose files have moved or gone. The pass is one parallel walk
/// and one batch update, so a missing file that took the batch down with it would leave every
/// other row on the list unhashed too.
#[tokio::test]
async fn a_row_whose_file_is_gone_does_not_cost_its_neighbour_a_hash() -> Result<(), AppError> {
    let (db, tmp) = library(&[("present.mp3", b"bytes"), ("gone.mp3", b"bytes")]).await?;
    std::fs::remove_file(tmp.path().join("gone.mp3"))?;

    hash_unhashed_tracks(&db).await?;

    assert!(hash_of(&db, "present.mp3").await?.is_some(), "a file that is there must be hashed");
    assert_eq!(hash_of(&db, "gone.mp3").await?, None, "and one that is not stays unanswered");
    Ok(())
}

/// The pass runs on every startup and after every folder add. A row that already has a hash is
/// not on the work list, so the steady state is one query and no file read at all.
#[tokio::test]
async fn a_row_that_already_has_a_hash_is_left_alone() -> Result<(), AppError> {
    let (db, _tmp) = library(&[("song.mp3", b"bytes")]).await?;
    sqlx::query("UPDATE tracks SET file_hash = 'from an earlier pass'")
        .execute(db.write())
        .await?;

    hash_unhashed_tracks(&db).await?;

    let row = rows(&db).await?;
    let (hash, mtime) = row.get("song.mp3").ok_or_else(|| AppError::NotFound("song.mp3".into()))?;
    assert_eq!(hash.as_deref(), Some("from an earlier pass"), "a hashed row is not on the list");
    assert_eq!(mtime.as_deref(), Some(STALE_MTIME), "and its mtime is not rewritten either");
    Ok(())
}
