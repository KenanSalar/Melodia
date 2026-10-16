//! A folder scan's last write: which rows it purges, and the counts it leaves behind.

use std::path::PathBuf;

use super::commit_scan;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::fixtures::{
    insert_test_track, make_test_metadata, numbered_library, setup_seeded_db,
};
use crate::database::queries::ingest::{Ingested, commit_scan_chunk};
use crate::media::ingest::scan_pool::ScanPool;
use melodia_core::entities::scan::ScannedFile;
use melodia_core::error::AppError;

/// Orphans past [`BULK_THRESHOLD`](crate::database::queries::ingest::BULK_THRESHOLD), so the
/// purge runs with the stats triggers dropped.
const BULK_ORPHANS: usize = 21;

async fn library_paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    let paths = sqlx::query_scalar("SELECT file_path FROM tracks ORDER BY file_path")
        .fetch_all(db.read())
        .await?;
    Ok(paths)
}

async fn stored_artist_count(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let count = sqlx::query_scalar("SELECT track_count FROM artists WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(count)
}

/// `numbered_library`'s first [`BULK_ORPHANS`] paths, purged as a bulk scan purges them.
async fn purge_in_bulk(db: &DbPool) -> Result<(), AppError> {
    let folder = queries::folder::get_folder_by_id(db, 1).await?;
    let orphans: Vec<String> =
        (0..BULK_ORPHANS).map(|index| format!("/music/{index:04}.mp3")).collect();
    commit_scan(db, &folder, &orphans, &Ingested::for_delta(BULK_ORPHANS)).await
}

/// The orphans are found before the import and handed over, so a row the watcher adds while the
/// scan runs is never taken for one.
#[tokio::test]
async fn a_scan_commit_deletes_exactly_the_orphans_it_is_handed() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let folder = queries::folder::get_folder_by_id(&db, 1).await?;

    commit_scan(&db, &folder, &["/music/track2.mp3".to_owned()], &Ingested::for_delta(1)).await?;

    assert_eq!(library_paths(&db).await?, ["/music/track1.mp3", "/music/track3.mp3"]);
    Ok(())
}

/// A file moved within the folder is re-pointed by the import and still named as an orphan by the
/// walk that preceded it. The purge goes by path, so it finds nothing to delete and the moved
/// track keeps its rating.
#[tokio::test]
async fn an_orphan_whose_row_a_chunk_repointed_deletes_nothing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let folder = queries::folder::insert_folder(&db, "/music", true).await?;
    let (old_path, new_path) = ("/music/old.mp3", "/music/new.mp3");
    let id = insert_test_track(&db, old_path, "Song", "Artist", "Album", "Rock").await?;
    queries::track::set_rating(&db, &[id], 5).await?;
    let moved = ScannedFile { path: PathBuf::from(new_path), metadata: make_test_metadata("Song") };
    let mut ingested = Ingested::for_delta(2);
    let chunk = commit_scan_chunk(
        &db,
        &[moved],
        folder.id,
        "2024-01-01T00:00:00Z",
        &ScanPool::default(),
        &ingested,
    )
    .await?;
    ingested.add(&chunk);

    commit_scan(&db, &folder, &[old_path.to_owned()], &ingested).await?;

    let track = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!((track.file_path.as_str(), track.rating), (new_path, 5), "(path, rating)");
    Ok(())
}

/// A bulk purge drops the stats triggers for its deletes, and a scan's chunks are not the only
/// writer: a favourite or a watcher batch after it relies on them being back. Read as a step from
/// wherever the purge left the count, so the recount isn't a second reason to fail.
#[tokio::test]
async fn a_bulk_purge_leaves_the_stats_triggers_live() -> Result<(), AppError> {
    let (db, _) = numbered_library(BULK_ORPHANS + 1).await?;
    purge_in_bulk(&db).await?;
    let after_purge = stored_artist_count(&db, "Artist").await?;

    insert_test_track(&db, "/music/added.mp3", "Added", "Artist", "Album", "Rock").await?;

    assert_eq!(stored_artist_count(&db, "Artist").await?, after_purge + 1);
    Ok(())
}

/// With the triggers dropped nothing kept the counts while the rows went, so the recalc after
/// the purge is the only thing that brings them back.
#[tokio::test]
async fn a_bulk_purge_leaves_the_counts_a_recount_gives() -> Result<(), AppError> {
    let (db, _) = numbered_library(BULK_ORPHANS + 1).await?;

    purge_in_bulk(&db).await?;

    assert_eq!(stored_artist_count(&db, "Artist").await?, 1);
    Ok(())
}

/// A rescan that only lost files still owes the sweep of what they emptied, or a deleted album
/// lingers on the grid with no tracks under it.
#[tokio::test]
async fn an_orphan_only_purge_prunes_the_album_it_emptied() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let folder = queries::folder::get_folder_by_id(&db, 1).await?;

    commit_scan(&db, &folder, &["/music/track2.mp3".to_owned()], &Ingested::for_delta(1)).await?;

    let albums: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums WHERE name = 'Album Two'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(albums, 0);
    Ok(())
}
