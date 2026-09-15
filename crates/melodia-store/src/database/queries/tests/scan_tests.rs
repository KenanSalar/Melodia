use crate::database::DbPool;
use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use crate::database::queries::scan::NameCache;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::{CreditRole, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

/// The release side of what `upsert_album` reads, carrying nothing but the year.
///
/// It takes the whole `ExtractedMetadata` now rather than a bare year, the release-level tags
/// having moved onto `albums` — and every one of them defaults to absent, which is the state these
/// cases were already asserting against.
fn album_meta(year: Option<i32>) -> ExtractedMetadata {
    let mut meta = make_test_metadata("Track");
    meta.year = year;
    meta
}

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
    let credit = ArtistCredit::from_name("Artist");
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let album_id = queries::scan::upsert_album(
        &mut tx,
        "Album",
        artist_id,
        &credit,
        &album_meta(Some(2024)),
        &mut NameCache::default(),
    )
    .await?;
    assert!(album_id.is_some());
    Ok(())
}

#[tokio::test]
async fn upsert_album_duplicate_returns_same_id() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let credit = ArtistCredit::from_name("Artist");
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let meta = album_meta(Some(2024));
    let mut names = NameCache::default();
    let id1 = queries::scan::upsert_album(&mut tx, "Album", artist_id, &credit, &meta, &mut names)
        .await?;
    let id2 = queries::scan::upsert_album(&mut tx, "Album", artist_id, &credit, &meta, &mut names)
        .await?;
    assert_eq!(id1, id2);
    Ok(())
}

#[tokio::test]
async fn upsert_album_empty_returns_none() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let credit = ArtistCredit::default();
    let result = queries::scan::upsert_album(
        &mut tx,
        "",
        1,
        &credit,
        &album_meta(None),
        &mut NameCache::default(),
    )
    .await?;
    assert!(result.is_none());
    Ok(())
}

#[tokio::test]
async fn upsert_album_updates_year_on_conflict() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let credit = ArtistCredit::from_name("Artist");
    let artist_id = queries::scan::upsert_artist(&mut tx, "Artist", 1).await?;
    let mut names = NameCache::default();
    let id = queries::scan::upsert_album(
        &mut tx,
        "Album",
        artist_id,
        &credit,
        &album_meta(Some(2001)),
        &mut names,
    )
    .await?;

    // Re-upsert of the same (name, artist_id) with a new year updates it (P2).
    let same = queries::scan::upsert_album(
        &mut tx,
        "Album",
        artist_id,
        &credit,
        &album_meta(Some(2010)),
        &mut names,
    )
    .await?;
    assert_eq!(id, same);
    let album_id = id.ok_or_else(|| AppError::Validation("no album id".into()))?;

    let year: Option<i32> = sqlx::query_scalar("SELECT year FROM albums WHERE id = ?")
        .bind(album_id)
        .fetch_one(&mut *tx)
        .await?;
    assert_eq!(year, Some(2010));

    // Re-upsert with a NULL year preserves the stored value (the COALESCE arm).
    queries::scan::upsert_album(
        &mut tx,
        "Album",
        artist_id,
        &credit,
        &album_meta(None),
        &mut names,
    )
    .await?;
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
    let credit = ArtistCredit::from_name("Test Artist");
    let artist_id = queries::scan::upsert_artist(&mut tx, "Test Artist", 1).await?;
    let meta = make_test_metadata("My Song");
    let mut names = NameCache::default();
    let album_id =
        queries::scan::upsert_album(&mut tx, "Test Album", artist_id, &credit, &meta, &mut names)
            .await?;
    let genre_id = queries::scan::upsert_genre(&mut tx, "Rock").await?;
    let ids = queries::ResolvedIds { artist_id, album_id, genre_id, folder_id: 1 };
    let now = "2024-01-01T00:00:00+00:00";
    queries::scan::insert_track(&mut tx, "/music/my.mp3", "my.mp3", &meta, &ids, now, &mut names)
        .await?;
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

    let paths = vec!["/music/track1.mp3".to_owned(), "/music/track3.mp3".to_owned()];
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

// === The credit tables the scan writes beside a track ===

/// A track credited as `printed` to `credited`, which is what the join rows are built from.
fn credited(title: &str, printed: &str, names: &[&str]) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_tags(
        printed,
        &names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
    );
    meta
}

/// A track's artist join rows as (position, name, join phrase).
async fn track_artists(db: &DbPool, track_id: i64) -> Result<Vec<(i64, String, String)>, AppError> {
    let rows = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT ta.position, a.name, ta.join_phrase \
         FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
         WHERE ta.track_id = ? ORDER BY ta.position",
    )
    .bind(track_id)
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// **Position 0 is the row's own `tracks.artist_id`.** Album grouping and the sort indexes still
/// read that FK, so the join table agreeing with it at the head is what stops an artist-scoped
/// query and an album-scoped one disagreeing about the same track.
#[tokio::test]
async fn a_credit_writes_one_row_per_name_with_the_primary_at_the_head() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(
        track_artists(&db, id).await?,
        vec![(0, "Alice".to_owned(), " feat. ".to_owned()), (1, "Bob".to_owned(), String::new()),]
    );

    let head: (i64,) = sqlx::query_as(
        "SELECT CASE WHEN t.artist_id = ta.artist_id THEN 1 ELSE 0 END \
         FROM tracks t JOIN track_artists ta ON ta.track_id = t.id AND ta.position = 0 \
         WHERE t.id = ?",
    )
    .bind(id)
    .fetch_one(db.read())
    .await?;
    assert_eq!(head.0, 1, "position 0 has to be the row's own artist_id");
    Ok(())
}

/// **One row per artist, not per name.** `artists.name` is `UNIQUE COLLATE NOCASE`, so a credit
/// naming someone twice under two spellings resolves to one row — and the stats triggers on this
/// table would then count the track twice for them.
#[tokio::test]
async fn a_name_spelled_twice_in_one_credit_is_joined_once() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Bob & bob", &["Bob", "bob"]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(track_artists(&db, id).await?.len(), 1);

    let bob: (i64,) = sqlx::query_as("SELECT track_count FROM artists WHERE name = 'Bob'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(bob.0, 1, "the join rows are what a count reads");
    // The printed line keeps both, being what the release spelled.
    assert_eq!(meta.artist.line(), Some("Bob & bob"));
    Ok(())
}

/// A file with no artist tag resolves to the sentinel and still gets its row, rather than being
/// credited to nobody and dropping out of every artist-scoped query.
#[tokio::test]
async fn a_track_crediting_nobody_still_gets_a_join_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::default();
    let id = insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    assert_eq!(track_artists(&db, id).await?.len(), 1);
    Ok(())
}

/// **The three tables are one fact about the row**, so a re-ingest replaces all of them together —
/// a path that cleared two is a track displaying credits it cannot be found by. Driven through
/// `update_track_metadata`, which is the door a tag edit and a re-scan both come through.
#[tokio::test]
async fn re_ingesting_a_track_leaves_none_of_its_old_join_rows() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let mut before = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    before.genres = GenreList::new(vec!["Rock".to_owned(), "Metal".to_owned()]);
    before.credits = RoleCredits::new(vec![RoleCredit {
        role: CreditRole::Composer,
        name: "Carol".to_owned(),
        detail: String::new(),
    }]);
    let id = insert_tagged_track(&db, "/music/1.mp3", &before).await?;

    let mut after = credited("One", "Alice", &[]);
    after.genres = GenreList::from_name("Rock");
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::default();
    let ids = queries::ResolvedIds {
        artist_id: names.artist(&mut tx, "Alice", 1).await?,
        album_id: None,
        genre_id: names.genre(&mut tx, "Rock").await?,
        folder_id: 1,
    };
    queries::scan::update_track_metadata(&mut tx, "/music/1.mp3", &after, &ids, &mut names).await?;
    tx.commit().await?;

    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM track_artists WHERE track_id = ?), \
                (SELECT COUNT(*) FROM track_genres WHERE track_id = ?), \
                (SELECT COUNT(*) FROM track_credits WHERE track_id = ?)",
    )
    .bind(id)
    .bind(id)
    .bind(id)
    .fetch_one(db.read())
    .await?;
    assert_eq!(counts, (1, 1, 0));
    Ok(())
}

// === Where an album files itself ===

/// **The whole track credit is the wrong key.** A guest on one track is not an album artist, so
/// taking "X feat. Y" here would rename the album after whichever track reached the upsert first
/// and list it in Y's discography.
#[test]
fn an_album_files_under_the_primary_name_and_not_the_whole_credit() {
    let mut meta = make_test_metadata("One");
    meta.artist =
        ArtistCredit::from_tags("Alice feat. Bob", &["Alice".to_owned(), "Bob".to_owned()]);

    assert_eq!(queries::scan::album_artist_name_for(&meta), "Alice");
    assert_eq!(queries::scan::album_credit_for(&meta).line(), Some("Alice"));
}

/// Its own tag wins where there is one, which is what keeps a compilation together.
#[test]
fn an_album_artist_tag_outranks_the_track_credit() {
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::from_name("Alice");
    meta.album_artist = ArtistCredit::from_name("Various Artists");

    assert_eq!(queries::scan::album_artist_name_for(&meta), "Various Artists");
    assert_eq!(queries::scan::album_credit_for(&meta).line(), Some("Various Artists"));
}

#[test]
fn an_album_with_nothing_to_file_under_credits_nobody() {
    let mut meta = make_test_metadata("One");
    meta.artist = ArtistCredit::default();
    meta.album_artist = ArtistCredit::default();

    assert_eq!(queries::scan::album_artist_name_for(&meta), "");
    assert!(queries::scan::album_credit_for(&meta).is_empty());
}

/// **A one-name credit is left NULL rather than repeating the name it would.** `albums` has only
/// ever had the FK, so `album_stats` falls back to the artist row — and rendering both is how the
/// two would come to disagree about a name a rescan changed on one of them.
#[tokio::test]
async fn an_album_renders_only_a_credit_the_artist_row_cannot_state() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let solo = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    insert_tagged_track(&db, "/music/1.mp3", &solo).await?;

    let mut shared = make_test_metadata("Two");
    shared.album = Some("Split".to_owned());
    shared.album_artist =
        ArtistCredit::from_tags("Alice & Bob", &["Alice".to_owned(), "Bob".to_owned()]);
    insert_tagged_track(&db, "/music/2.mp3", &shared).await?;

    // The track's guest is not an album artist, so the album files under the primary alone.
    assert_eq!(album_credit(&db, "Test Album").await?, ("Alice".to_owned(), None));
    assert_eq!(
        album_credit(&db, "Split").await?,
        ("Alice".to_owned(), Some("Alice & Bob".to_owned()))
    );
    Ok(())
}

/// An album's filed-under artist and the credit it renders, if any.
async fn album_credit(db: &DbPool, name: &str) -> Result<(String, Option<String>), AppError> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT a.name, al.artist_credit \
         FROM albums al JOIN artists a ON a.id = al.artist_id WHERE al.name = ?",
    )
    .bind(name)
    .fetch_one(db.read())
    .await?;
    Ok(row)
}

// === The name cache ===

/// One upsert per distinct spelling per transaction. A guest repeats across a release and the join
/// writers ask once per credit per track, so without it a 200-track box set is 200 identical
/// upserts of the same name.
#[tokio::test]
async fn a_name_asked_for_twice_resolves_to_one_row() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::for_chunk(10);

    let first = names.artist(&mut tx, "Alice", 1).await?;
    let again = names.artist(&mut tx, "Alice", 1).await?;
    tx.commit().await?;

    assert_eq!(first, again);
    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM artists WHERE name = 'Alice'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(rows.0, 1);
    Ok(())
}

/// **Keyed on the exact spelling**, so two spellings share one row and the upsert leaves the
/// newest standing — and a spelling seen *again* after a different one no longer wins the row
/// back, which is how the primary-artist path has always behaved.
#[tokio::test]
async fn a_spelling_seen_again_after_another_does_not_win_the_row_back() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::default();

    names.artist(&mut tx, "Alice", 1).await?;
    names.artist(&mut tx, "ALICE", 1).await?;
    names.artist(&mut tx, "Alice", 1).await?;
    tx.commit().await?;

    let stored: (i64, String) = sqlx::query_as(
        "SELECT COUNT(*), MAX(name) FROM artists WHERE name = 'alice' COLLATE NOCASE",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(stored, (1, "ALICE".to_owned()));
    Ok(())
}

/// An empty name answers the sentinel without touching the table, so callers stay branch-free.
#[tokio::test]
async fn a_name_with_nothing_in_it_resolves_to_the_sentinel() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut tx = db.write().begin().await?;
    let mut names = queries::scan::NameCache::default();

    assert_eq!(names.artist(&mut tx, "", 1).await?, 1);
    assert_eq!(names.genre(&mut tx, "").await?, None);
    tx.commit().await?;

    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM artists").fetch_one(db.read()).await?;
    assert_eq!(rows.0, 1, "only the schema's own sentinel row");
    Ok(())
}

/// **The credit tables are part of what keeps an artist alive.** A predicate over the two FKs
/// alone deletes exactly the artists the credit tables exist to surface.
#[tokio::test]
async fn a_guest_who_is_nobodys_primary_artist_survives_the_prune() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let meta = credited("One", "Alice feat. Bob", &["Alice", "Bob"]);
    insert_tagged_track(&db, "/music/1.mp3", &meta).await?;

    let mut tx = db.write().begin().await?;
    queries::scan::prune_orphans(&mut tx).await?;
    tx.commit().await?;

    let bob: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM artists WHERE name = 'Bob'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(bob.0, 1);
    Ok(())
}
