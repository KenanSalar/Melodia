//! What the pass marks, and what it answers a second run with.
//!
//! The whole sweep rests on `mark_every_track_stale` being idempotent: `OnFailure::Retry` runs it
//! again on the next launch, and a pass that re-marked rows the scan had already re-read would
//! queue the library for a re-parse every time.

use super::*;
use melodia_core::error::AppError;
use melodia_store::database::queries::fixtures::insert_test_track;

/// A library of `count` tracks, each carrying the mtime a scan wrote.
async fn library(count: usize) -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    melodia_store::database::queries::folder::insert_folder(&db, "/music", true).await?;
    for n in 1..=count {
        let path = format!("/music/{n}.mp3");
        insert_test_track(&db, &path, &format!("Song {n}"), "Artist", "Album", "Rock").await?;
    }
    Ok(db)
}

async fn still_dated(db: &DbPool) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tracks WHERE date_modified IS NOT NULL")
        .fetch_one(db.read())
        .await?;
    Ok(row.0)
}

/// The column means "the mtime the stored row was parsed from", so NULL is honestly unknown —
/// `track_is_current` compares it to the file's own and a missing value cannot match one.
#[tokio::test]
async fn the_pass_blanks_every_mtime_and_answers_with_what_it_moved() -> Result<(), AppError> {
    let db = library(3).await?;
    assert_eq!(still_dated(&db).await?, 3);

    assert_eq!(mark_every_track_stale(&db).await?, 3);

    assert_eq!(still_dated(&db).await?, 0);
    Ok(())
}

/// **The retry costs one more UPDATE and nothing else.** A second pass finds nothing to blank, so
/// the sweep's `OnFailure::Retry` cannot put the library back in the queue on every launch — and
/// `backfill`'s own early return is what reads this zero.
#[tokio::test]
async fn a_second_pass_finds_nothing_left_to_mark() -> Result<(), AppError> {
    let db = library(3).await?;

    mark_every_track_stale(&db).await?;

    assert_eq!(mark_every_track_stale(&db).await?, 0);
    Ok(())
}

#[tokio::test]
async fn an_empty_library_marks_nothing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    assert_eq!(mark_every_track_stale(&db).await?, 0);
    Ok(())
}
