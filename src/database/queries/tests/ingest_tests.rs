use std::path::PathBuf;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::ingest::{FolderResolution, ingest_scanned_files};
use crate::database::queries::tests::helpers::{insert_test_track, make_test_metadata};
use crate::error::AppError;
use crate::media::scanner::ScannedFile;

fn make_scanned_file(path: &str, title: &str) -> ScannedFile {
    let mut meta = make_test_metadata(title);
    // Give each file a unique hash derived from path content
    let hash = blake3::hash(path.as_bytes());
    meta.file_hash = hash.to_hex().to_string();
    ScannedFile {
        path: PathBuf::from(path),
        metadata: meta,
    }
}

fn make_scanned_file_with_hash(path: &str, title: &str, hash: &str) -> ScannedFile {
    let mut meta = make_test_metadata(title);
    meta.file_hash = hash.to_owned();
    ScannedFile {
        path: PathBuf::from(path),
        metadata: meta,
    }
}

#[tokio::test]
async fn ingest_empty_list_returns_zero() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &[],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    assert_eq!(result.inserted_count, 0);
    assert!(result.inserted_track_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn ingest_single_file_fixed_folder() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let files = vec![make_scanned_file("/music/song.mp3", "My Song")];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 1);
    assert_eq!(result.inserted_track_ids.len(), 1);
    assert!(result.inserted_track_ids[0] > 0);

    // Verify track exists in DB
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_optional(db.read())
            .await?;
    assert!(exists.is_some());
    Ok(())
}

#[tokio::test]
async fn ingest_deduplicates_artists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut f1 = make_scanned_file("/music/a.mp3", "A");
    f1.metadata.artist = Some("Same Artist".to_owned());
    let mut f2 = make_scanned_file("/music/b.mp3", "B");
    f2.metadata.artist = Some("Same Artist".to_owned());
    let mut f3 = make_scanned_file("/music/c.mp3", "C");
    f3.metadata.artist = Some("Same Artist".to_owned());

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &[f1, f2, f3],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 3);

    // Verify only one artist row (plus the sentinel "Unknown Artist")
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM artists WHERE name = 'Same Artist'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(count.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_deduplicates_albums() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut f1 = make_scanned_file("/music/a.mp3", "A");
    f1.metadata.artist = Some("Artist".to_owned());
    f1.metadata.album = Some("Same Album".to_owned());
    let mut f2 = make_scanned_file("/music/b.mp3", "B");
    f2.metadata.artist = Some("Artist".to_owned());
    f2.metadata.album = Some("Same Album".to_owned());

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[f1, f2],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM albums WHERE name = 'Same Album'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(count.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_deduplicates_genres() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut f1 = make_scanned_file("/music/a.mp3", "A");
    f1.metadata.genre = Some("Rock".to_owned());
    let mut f2 = make_scanned_file("/music/b.mp3", "B");
    f2.metadata.genre = Some("Rock".to_owned());

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[f1, f2],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM genres WHERE name = 'Rock'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(count.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_skips_existing_track() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/existing.mp3", "Existing", "Art", "Alb", "Pop").await?;

    let files = vec![make_scanned_file("/music/existing.mp3", "Existing")];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 0);
    assert!(result.inserted_track_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn ingest_updates_artwork_on_existing_when_flag_set() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Art", "Alb", "Pop").await?;

    let mut file = make_scanned_file("/music/song.mp3", "Song");
    file.metadata.artwork_path = Some("/art/cover.jpg".to_owned());

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &[file],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        true, // update artwork on existing
    )
    .await?;
    tx.commit().await?;

    // Track was not inserted (already existed)
    assert_eq!(result.inserted_count, 0);

    // But artwork should have been updated
    let artwork: Option<(Option<String>,)> =
        sqlx::query_as("SELECT artwork_path FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_optional(db.read())
            .await?;
    assert_eq!(
        artwork.ok_or_else(|| AppError::Validation("track missing".into()))?.0.as_deref(),
        Some("/art/cover.jpg")
    );
    Ok(())
}

#[tokio::test]
async fn ingest_does_not_update_artwork_when_flag_false() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Art", "Alb", "Pop").await?;

    let mut file = make_scanned_file("/music/song.mp3", "Song");
    file.metadata.artwork_path = Some("/art/cover.jpg".to_owned());

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[file],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false, // don't update artwork
    )
    .await?;
    tx.commit().await?;

    let artwork: (Option<String>,) =
        sqlx::query_as("SELECT artwork_path FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_one(db.read())
            .await?;
    assert!(artwork.0.is_none());
    Ok(())
}

#[tokio::test]
async fn ingest_from_parent_dir_creates_folder() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let files = vec![make_scanned_file("/home/user/music/song.mp3", "Song")];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::FromParentDir,
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 1);

    // Verify folder was created
    let folders = queries::folder::get_all_folders(&db).await?;
    assert!(folders.iter().any(|f| f.path == "/home/user/music"));
    Ok(())
}

#[tokio::test]
async fn ingest_from_parent_dir_caches_folder() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let files = vec![
        make_scanned_file("/data/music/a.mp3", "A"),
        make_scanned_file("/data/music/b.mp3", "B"),
    ];

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::FromParentDir,
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Only one folder row for /data/music
    let folders = queries::folder::get_all_folders(&db).await?;
    let count = folders.iter().filter(|f| f.path == "/data/music").count();
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_empty_artist_gets_unknown_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut file = make_scanned_file("/music/no_artist.mp3", "No Artist Track");
    file.metadata.artist = None;

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[file],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Track should be linked to the sentinel "Unknown Artist" (id=1)
    let artist_id: (i64,) =
        sqlx::query_as("SELECT artist_id FROM tracks WHERE file_path = '/music/no_artist.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(artist_id.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_empty_album_gets_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut file = make_scanned_file("/music/no_album.mp3", "No Album Track");
    file.metadata.album = None;

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[file],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Empty album name → upsert_album returns None → album_id should be NULL
    let album_id: (Option<i64>,) =
        sqlx::query_as("SELECT album_id FROM tracks WHERE file_path = '/music/no_album.mp3'")
            .fetch_one(db.read())
            .await?;
    assert!(album_id.0.is_none());
    Ok(())
}

#[tokio::test]
async fn ingest_mixed_new_and_existing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/existing.mp3", "Existing", "Art", "Alb", "Pop").await?;

    let files = vec![
        make_scanned_file("/music/existing.mp3", "Existing"),
        make_scanned_file("/music/new.mp3", "New Song"),
    ];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 1);
    assert_eq!(result.inserted_track_ids.len(), 1);
    Ok(())
}

#[tokio::test]
async fn ingest_same_album_different_artists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut f1 = make_scanned_file("/music/a.mp3", "A");
    f1.metadata.artist = Some("Artist 1".to_owned());
    f1.metadata.album = Some("Compilation".to_owned());
    let mut f2 = make_scanned_file("/music/b.mp3", "B");
    f2.metadata.artist = Some("Artist 2".to_owned());
    f2.metadata.album = Some("Compilation".to_owned());

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &[f1, f2],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Two-level album cache: same album name + different artist → separate album rows
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM albums WHERE name = 'Compilation'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(count.0, 2);
    Ok(())
}

// --- Moved file detection tests ---

#[tokio::test]
async fn ingest_detects_moved_file() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Insert a track with a known hash at the old path
    let old_id =
        insert_test_track(&db, "/music/old/song.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Get the hash that was inserted
    let hash: (String,) = sqlx::query_as("SELECT file_hash FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;

    // Now ingest the same content at a new path
    let files = vec![make_scanned_file_with_hash(
        "/music/new/song.mp3",
        "Song",
        &hash.0,
    )];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Should be detected as moved, not inserted
    assert_eq!(result.inserted_count, 0);
    assert_eq!(result.moved_count, 1);

    // Verify the track's path was updated
    let new_path: (String,) = sqlx::query_as("SELECT file_path FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(new_path.0, "/music/new/song.mp3");

    // Verify only one track exists (no duplicate)
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_batched_inserts_preserve_input_order_across_chunks() -> Result<(), AppError> {
    // More files than one multi-row INSERT chunk holds, so the batch path
    // flushes mid-loop at least once. `inserted_track_ids` must come back
    // in input order (the DnD import queues tracks in drop order), and
    // every id must point at the right row — the mapping goes through
    // `RETURNING id, file_path` because SQLite's RETURNING output order
    // is unspecified.
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let n = queries::scan::INSERT_CHUNK_ROWS + 5;
    let files: Vec<ScannedFile> = (0..n)
        .map(|i| {
            make_scanned_file_with_hash(
                &format!("/music/batch/{i:04}.mp3"),
                &format!("Track {i:04}"),
                &format!("{i:064}"),
            )
        })
        .collect();

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, u32::try_from(n).unwrap_or(u32::MAX));
    assert_eq!(result.inserted_track_ids.len(), n);

    // Each returned id (in input order) must resolve to the matching path.
    for (i, id) in result.inserted_track_ids.iter().enumerate() {
        let path: (String,) = sqlx::query_as("SELECT file_path FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(db.read())
            .await?;
        assert_eq!(path.0, format!("/music/batch/{i:04}.mp3"));
    }

    // Playback defaults from the literal block.
    let defaults: (i64, i64, bool) = sqlx::query_as(
        "SELECT play_count, last_position, is_favorite FROM tracks WHERE file_path = '/music/batch/0000.mp3'",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(defaults.0, 0);
    assert_eq!(defaults.1, 0);
    assert!(!defaults.2);
    Ok(())
}

#[tokio::test]
async fn ingest_duplicate_hash_repoints_once_inserts_second() -> Result<(), AppError> {
    // Two new files carrying the same content hash + one existing row
    // whose old path is gone from disk: the first file claims the move,
    // and the second must fall through to a fresh insert instead of
    // re-pointing (stealing) the same row again — consume-once, mirroring
    // the reconcile path's moved-candidates map.
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let old_id =
        insert_test_track(&db, "/music/old/song.mp3", "Song", "Art", "Alb", "Rock").await?;
    let hash: (String,) = sqlx::query_as("SELECT file_hash FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;

    let files = vec![
        make_scanned_file_with_hash("/music/new/copy-a.mp3", "Song", &hash.0),
        make_scanned_file_with_hash("/music/new/copy-b.mp3", "Song", &hash.0),
    ];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.moved_count, 1);
    assert_eq!(result.inserted_count, 1);

    // The existing row was re-pointed to the FIRST new path…
    let repointed: (String,) = sqlx::query_as("SELECT file_path FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(repointed.0, "/music/new/copy-a.mp3");

    // …and the second file got its own fresh row — no path lost.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count.0, 2);
    let second: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tracks WHERE file_path = '/music/new/copy-b.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(second.0, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_does_not_move_when_hash_differs() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Insert a track with hash "aaa..."
    insert_test_track(&db, "/music/existing.mp3", "Existing", "Art", "Alb", "Rock").await?;

    // Ingest a new file with a different hash
    let files = vec![make_scanned_file_with_hash(
        "/music/new.mp3",
        "New Song",
        &"b".repeat(64),
    )];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Should be a normal insert, not a move
    assert_eq!(result.inserted_count, 1);
    assert_eq!(result.moved_count, 0);

    // Two tracks should exist
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count.0, 2);
    Ok(())
}

#[tokio::test]
async fn ingest_moved_file_preserves_playback_state() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let old_id = insert_test_track(&db, "/music/old.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Set playback state on the existing track
    queries::track::update_play_count(&db, old_id).await?;
    queries::track::update_play_count(&db, old_id).await?;

    let hash: (String,) = sqlx::query_as("SELECT file_hash FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;

    // Ingest same hash at new path
    let files = vec![make_scanned_file_with_hash(
        "/music/new.mp3",
        "Song",
        &hash.0,
    )];
    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    // Verify play_count was preserved
    let play_count: (i32,) = sqlx::query_as("SELECT play_count FROM tracks WHERE id = ?")
        .bind(old_id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(play_count.0, 2);
    Ok(())
}

// --- Incremental scanning tests ---

#[tokio::test]
async fn ingest_skips_unchanged_file() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Ingest the same file with matching mtime and size
    let files = vec![make_scanned_file("/music/song.mp3", "Song")];

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 0);
    assert_eq!(result.updated_count, 0);
    Ok(())
}

#[tokio::test]
async fn ingest_updates_metadata_when_file_changed() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Old Title", "Art", "Alb", "Rock").await?;

    // Ingest with different mtime (simulates file modification)
    let mut file = make_scanned_file("/music/song.mp3", "New Title");
    file.metadata.date_modified = Some("2025-06-01T00:00:00+00:00".to_owned());

    let mut tx = db.write().begin().await?;
    let result = ingest_scanned_files(
        &mut tx,
        &[file],
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    assert_eq!(result.inserted_count, 0);
    assert_eq!(result.updated_count, 1);

    // Verify title was updated
    let title: (String,) =
        sqlx::query_as("SELECT title FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(title.0, "New Title");
    Ok(())
}

#[tokio::test]
async fn ingest_stores_file_hash() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let hash = "c".repeat(64);
    let files = vec![make_scanned_file_with_hash(
        "/music/song.mp3",
        "Song",
        &hash,
    )];

    let mut tx = db.write().begin().await?;
    ingest_scanned_files(
        &mut tx,
        &files,
        &FolderResolution::Fixed(1),
        "2024-01-01T00:00:00Z",
        false,
    )
    .await?;
    tx.commit().await?;

    let stored_hash: (Option<String>,) =
        sqlx::query_as("SELECT file_hash FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(stored_hash.0.as_deref(), Some(hash.as_str()));
    Ok(())
}
