use crate::database::DbPool;
use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use crate::error::AppError;

// === Pure unit tests for to_natural_sort_key ===

#[test]
fn sort_key_pads_numbers() {
    assert_eq!(queries::scan::to_natural_sort_key("Track 2"), "track 00000002");
}

#[test]
fn sort_key_leading_digits() {
    assert_eq!(queries::scan::to_natural_sort_key("10 Songs"), "00000010 songs");
}

#[test]
fn sort_key_no_digits() {
    assert_eq!(queries::scan::to_natural_sort_key("abc"), "abc");
}

#[test]
fn sort_key_empty() {
    assert_eq!(queries::scan::to_natural_sort_key(""), "");
}

#[test]
fn sort_key_mixed_alpha_numeric() {
    assert_eq!(queries::scan::to_natural_sort_key("a1b2c3"), "a00000001b00000002c00000003");
}

#[test]
fn sort_key_large_number_not_truncated() {
    assert_eq!(queries::scan::to_natural_sort_key("track 123456789"), "track 123456789");
}

#[test]
fn sort_key_preserves_ordering() {
    let mut keys: Vec<String> = vec!["Track 10", "Track 2", "Track 1", "Track 20"]
        .into_iter()
        .map(queries::scan::to_natural_sort_key)
        .collect();
    keys.sort();
    assert_eq!(keys[0], queries::scan::to_natural_sort_key("Track 1"));
    assert_eq!(keys[1], queries::scan::to_natural_sort_key("Track 2"));
    assert_eq!(keys[2], queries::scan::to_natural_sort_key("Track 10"));
    assert_eq!(keys[3], queries::scan::to_natural_sort_key("Track 20"));
}

// === Async DB tests ===

#[tokio::test]
async fn track_exists_by_path_false_when_empty() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let exists = queries::scan::track_exists_by_path(&mut tx, "/nonexistent.mp3").await?;
    assert!(!exists);
    Ok(())
}

#[tokio::test]
async fn track_exists_by_path_true_after_insert() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let exists = queries::scan::track_exists_by_path(&mut tx, "/music/song.mp3").await?;
    assert!(exists);
    Ok(())
}

#[tokio::test]
async fn upsert_artist_new_returns_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id = queries::scan::upsert_artist(&mut tx, "New Artist", 1).await?;
    assert!(id > 1); // 1 is the sentinel "Unknown Artist"
    Ok(())
}

#[tokio::test]
async fn upsert_artist_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id1 = queries::scan::upsert_artist(&mut tx, "Duplicate", 1).await?;
    let id2 = queries::scan::upsert_artist(&mut tx, "Duplicate", 1).await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_artist_empty_returns_unknown() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id = queries::scan::upsert_artist(&mut tx, "", 1).await?;
    assert_eq!(id, 1);
    Ok(())
}

#[tokio::test]
async fn upsert_album_new_returns_some() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let album_id = queries::scan::upsert_album(&mut tx, "Album", artist_id, Some(2024)).await?;
    assert!(album_id.is_some());
    Ok(())
}

#[tokio::test]
async fn upsert_album_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let id1 = queries::scan::upsert_album(&mut tx, "Album", artist_id, Some(2024)).await?;
    let id2 = queries::scan::upsert_album(&mut tx, "Album", artist_id, Some(2024)).await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_album_empty_returns_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let result = queries::scan::upsert_album(&mut tx, "", 1, None).await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test]
async fn upsert_album_updates_year_on_conflict() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let id = queries::scan::upsert_album(&mut tx, "Album", artist_id, Some(2001)).await?;

    // Re-upsert of the same (name, artist_id) with a new year updates it (P2).
    let same = queries::scan::upsert_album(&mut tx, "Album", artist_id, Some(2010)).await?;
    assert_eq!(id, same);
    let album_id = id.ok_or_else(|| AppError::Validation("no album id".into()))?;

    let year: Option<i32> = sqlx::query_scalar("SELECT year FROM albums WHERE id = ?")
        .bind(album_id)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(year, Some(2010));

    // Re-upsert with a NULL year preserves the stored value (the COALESCE arm).
    queries::scan::upsert_album(&mut tx, "Album", artist_id, None).await?;
    let year: Option<i32> = sqlx::query_scalar("SELECT year FROM albums WHERE id = ?")
        .bind(album_id)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(year, Some(2010));
    Ok(())
}

#[tokio::test]
async fn upsert_genre_new_returns_some() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let genre_id = queries::scan::upsert_genre(&mut tx, "Rock").await?;
    assert!(genre_id.is_some());
    Ok(())
}

#[tokio::test]
async fn upsert_genre_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id1 = queries::scan::upsert_genre(&mut tx, "Rock").await?;
    let id2 = queries::scan::upsert_genre(&mut tx, "Rock").await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_genre_empty_returns_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let result = queries::scan::upsert_genre(&mut tx, "").await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test]
async fn insert_track_stores_correct_fields() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut tx = db.write().begin().await?;
    let artist_id = queries::scan::upsert_artist(&mut tx, "Test Artist", 1).await?;
    let album_id =
        queries::scan::upsert_album(&mut tx, "Test Album", artist_id, Some(2024)).await?;
    let genre_id = queries::scan::upsert_genre(&mut tx, "Rock").await?;

    let meta = make_test_metadata("My Song");
    let ids = queries::ResolvedIds {
        artist_id,
        album_id,
        genre_id,
        folder_id: 1,
    };
    let now = "2024-01-01T00:00:00+00:00";
    queries::scan::insert_track(&mut tx, "/music/my.mp3", "my.mp3", &meta, &ids, now).await?;
    tx.commit().await?;

    // Verify via raw query
    let row: (String, i64, String) = sqlx::query_as(
        "SELECT title, duration_ms, date_added FROM tracks WHERE file_path = '/music/my.mp3'",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(row.0, "My Song");
    assert_eq!(row.1, 180_000);
    assert_eq!(row.2, now);
    Ok(())
}

#[tokio::test]
async fn update_track_artwork_if_missing_sets_when_null() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    queries::scan::update_track_artwork_if_missing(&mut tx, "/music/song.mp3", "/art/cover.jpg")
        .await?;
    tx.commit().await?;

    let artwork: Option<String> =
        sqlx::query_scalar("SELECT artwork_path FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(artwork.as_deref(), Some("/art/cover.jpg"));
    Ok(())
}

#[tokio::test]
async fn update_track_artwork_if_missing_preserves_existing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    // Set artwork first
    sqlx::query(
        "UPDATE tracks SET artwork_path = '/art/original.jpg' WHERE file_path = '/music/song.mp3'",
    )
    .execute(db.write())
    .await?;

    // Try to overwrite — should not change
    let mut tx = db.write().begin().await?;
    queries::scan::update_track_artwork_if_missing(&mut tx, "/music/song.mp3", "/art/new.jpg")
        .await?;
    tx.commit().await?;

    let artwork: Option<String> =
        sqlx::query_scalar("SELECT artwork_path FROM tracks WHERE file_path = '/music/song.mp3'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(artwork.as_deref(), Some("/art/original.jpg"));
    Ok(())
}

#[tokio::test]
async fn update_album_artwork_from_tracks_fills_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    // Set artwork on track
    sqlx::query(
        "UPDATE tracks SET artwork_path = '/art/cover.jpg' WHERE file_path = '/music/song.mp3'",
    )
    .execute(db.write())
    .await?;

    let mut tx = db.write().begin().await?;
    queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
    tx.commit().await?;

    let artwork: Option<String> =
        sqlx::query_scalar("SELECT artwork_path FROM albums WHERE name = 'Album'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(artwork.as_deref(), Some("/art/cover.jpg"));
    Ok(())
}

// === Tests for delete_track_by_path ===

#[tokio::test]
async fn delete_track_by_path_returns_true_when_exists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let deleted = queries::scan::delete_track_by_path(&mut tx, "/music/song.mp3").await?;
    tx.commit().await?;

    assert!(deleted);

    // Verify track is gone
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count, 0);
    Ok(())
}

#[tokio::test]
async fn delete_track_by_path_returns_false_when_not_found() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let deleted = queries::scan::delete_track_by_path(&mut tx, "/nonexistent.mp3").await?;
    assert!(!deleted);
    Ok(())
}

// === Tests for delete_tracks_by_paths_batch ===

#[tokio::test]
async fn delete_tracks_batch_deletes_multiple() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let paths = vec![
        "/music/track1.mp3".to_owned(),
        "/music/track3.mp3".to_owned(),
    ];
    let mut tx = db.write().begin().await?;
    let deleted = queries::scan::delete_tracks_by_paths_batch(&mut tx, &paths).await?;
    tx.commit().await?;

    assert_eq!(deleted, 2);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks").fetch_one(db.read()).await?;
    assert_eq!(count, 1); // only track2 remains
    Ok(())
}

#[tokio::test]
async fn delete_tracks_batch_empty_returns_zero() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let deleted = queries::scan::delete_tracks_by_paths_batch(&mut tx, &[]).await?;
    assert_eq!(deleted, 0);
    Ok(())
}

// === Tests for find_folder_for_path ===

#[tokio::test]
async fn find_folder_for_path_matches_parent() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut tx = db.write().begin().await?;
    let folder_id = queries::scan::find_folder_for_path(&mut tx, "/music/song.mp3").await?;
    assert!(folder_id.is_some());
    Ok(())
}

/// Every other test here spells a POSIX path, which is every path on the CI runner and none
/// on Windows — where a library folder is `C:\Music` and its tracks `C:\Music\a.mp3`. Building
/// the pair from `MAIN_SEPARATOR_STR` asks the question each platform actually faces; a literal
/// backslash would only ever fail on Linux, which is why the gap survived.
#[tokio::test]
async fn find_folder_for_path_matches_a_native_separator() -> Result<(), AppError> {
    use std::path::MAIN_SEPARATOR_STR as SEP;

    let db = DbPool::test_pool().await?;
    let folder = format!("{SEP}music");
    queries::folder::insert_folder(&db, &folder, true).await?;

    let mut tx = db.write().begin().await?;
    let folder_id =
        queries::scan::find_folder_for_path(&mut tx, &format!("{folder}{SEP}song.mp3")).await?;
    assert!(folder_id.is_some(), "a path spelled the way the OS spells it must resolve");
    Ok(())
}

#[tokio::test]
async fn find_folder_for_path_matches_nested() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::insert_folder(&db, "/music/rock", true).await?;

    // Get the ID of /music/rock folder before opening write tx
    let rock_id: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE path = '/music/rock'")
        .fetch_one(db.read())
        .await?;

    let mut tx = db.write().begin().await?;
    // Should match the longer prefix "/music/rock"
    let folder_id = queries::scan::find_folder_for_path(&mut tx, "/music/rock/song.mp3").await?;
    assert_eq!(folder_id, Some(rock_id));
    Ok(())
}

#[tokio::test]
async fn find_folder_for_path_returns_none_for_unknown() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut tx = db.write().begin().await?;
    let folder_id = queries::scan::find_folder_for_path(&mut tx, "/other/song.mp3").await?;
    assert!(folder_id.is_none());
    Ok(())
}

// === Tests for get_all_track_paths_for_folder ===

#[tokio::test]
async fn get_all_track_paths_for_folder_returns_correct_paths() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let mut tx = db.write().begin().await?;
    let paths = queries::scan::get_all_track_paths_for_folder(&mut tx, 1).await?;

    assert_eq!(paths.len(), 3);
    assert!(paths.contains(&"/music/track1.mp3".to_owned()));
    assert!(paths.contains(&"/music/track2.mp3".to_owned()));
    assert!(paths.contains(&"/music/track3.mp3".to_owned()));
    Ok(())
}

#[tokio::test]
async fn get_all_track_paths_for_folder_empty_for_nonexistent() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let paths = queries::scan::get_all_track_paths_for_folder(&mut tx, 999).await?;
    assert!(paths.is_empty());
    Ok(())
}

// === Tests for get_track_id_by_path ===

#[tokio::test]
async fn get_track_id_by_path_returns_id_when_exists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let expected_id =
        insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let id = queries::scan::get_track_id_by_path(&mut tx, "/music/song.mp3").await?;
    assert_eq!(id, Some(expected_id));
    Ok(())
}

#[tokio::test]
async fn get_track_id_by_path_returns_none_when_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let id = queries::scan::get_track_id_by_path(&mut tx, "/nonexistent.mp3").await?;
    assert!(id.is_none());
    Ok(())
}

// === Tests for update_track_location ===

#[tokio::test]
async fn update_track_location_repoints_existing_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let folder = queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/old.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let repointed = queries::scan::update_track_location(
        &mut tx,
        id,
        "/music/new.mp3",
        "new.mp3",
        folder.id,
        None,
    )
    .await?;
    assert!(repointed);
    let moved = queries::scan::get_track_id_by_path(&mut tx, "/music/new.mp3").await?;
    assert_eq!(moved, Some(id));
    Ok(())
}

#[tokio::test]
async fn update_track_location_false_when_row_deleted_in_tx() -> Result<(), AppError> {
    // The reconcile move-detection candidate map is resolved before the
    // write transaction opens; a Removed event processed earlier in the
    // same batch can delete the candidate row. The re-point must report
    // "no row hit" so `handle_created` falls back to a fresh insert
    // instead of silently dropping the track.
    let db = DbPool::test_pool().await?;
    let folder = queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/old.mp3", "Song", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    assert!(queries::scan::delete_track_by_path(&mut tx, "/music/old.mp3").await?);
    let repointed = queries::scan::update_track_location(
        &mut tx,
        id,
        "/music/new.mp3",
        "new.mp3",
        folder.id,
        None,
    )
    .await?;
    assert!(!repointed);
    Ok(())
}
