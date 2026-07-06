#[allow(clippy::wildcard_imports)]
use crate::database::queries::tests::helpers::*;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::AppError;

async fn seed_db() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/alpha.mp3", "Alpha", "Zeta Artist", "B Album", "Pop").await?;
    insert_test_track(&db, "/music/beta.mp3", "Beta", "Alpha Artist", "A Album", "Rock").await?;
    insert_test_track(&db, "/music/gamma.mp3", "Gamma", "Alpha Artist", "A Album", "Rock").await?;
    Ok(db)
}

#[tokio::test]
async fn get_all_tracks_default_sort() -> Result<(), AppError> {
    let db = seed_db().await?;
    let tracks = queries::track::get_all_tracks(&db, None, None).await?;
    assert_eq!(tracks.len(), 3);
    // Default sort is by sort_key (natural sort of title), ascending
    assert_eq!(tracks[0].title, "Alpha");
    assert_eq!(tracks[1].title, "Beta");
    assert_eq!(tracks[2].title, "Gamma");
    Ok(())
}

#[tokio::test]
async fn get_all_tracks_sort_by_artist() -> Result<(), AppError> {
    let db = seed_db().await?;
    let tracks =
        queries::track::get_all_tracks(&db, Some("artist".to_owned()), None).await?;
    // "Alpha Artist" before "Zeta Artist"
    assert_eq!(tracks[0].artist.as_deref(), Some("Alpha Artist"));
    assert_eq!(tracks[2].artist.as_deref(), Some("Zeta Artist"));
    Ok(())
}

#[tokio::test]
async fn get_all_tracks_sort_desc() -> Result<(), AppError> {
    let db = seed_db().await?;
    let tracks =
        queries::track::get_all_tracks(&db, None, Some("desc".to_owned())).await?;
    assert_eq!(tracks[0].title, "Gamma");
    assert_eq!(tracks[2].title, "Alpha");
    Ok(())
}

#[tokio::test]
async fn get_tracks_by_album() -> Result<(), AppError> {
    let db = seed_db().await?;
    let album_id: i64 = sqlx::query_scalar("SELECT id FROM albums WHERE name = 'A Album'")
        .fetch_one(db.read())
        .await?;
    let tracks = queries::track::get_tracks_by_album(&db, album_id).await?;
    assert_eq!(tracks.len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_tracks_by_artist() -> Result<(), AppError> {
    let db = seed_db().await?;
    let artist_id: i64 =
        sqlx::query_scalar("SELECT id FROM artists WHERE name = 'Alpha Artist'")
            .fetch_one(db.read())
            .await?;
    let tracks = queries::track::get_tracks_by_artist(&db, artist_id).await?;
    assert_eq!(tracks.len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_tracks_by_genre() -> Result<(), AppError> {
    let db = seed_db().await?;
    let genre_id: i64 = sqlx::query_scalar("SELECT id FROM genres WHERE name = 'Rock'")
        .fetch_one(db.read())
        .await?;
    let tracks = queries::track::get_tracks_by_genre(&db, genre_id).await?;
    assert_eq!(tracks.len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_track_by_id_happy_path() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.id, id);
    Ok(())
}

#[tokio::test]
async fn get_track_by_id_not_found() {
    let db = DbPool::test_pool().await;
    let result = queries::track::get_track_by_id(&db, 99999).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn get_tracks_by_ids_preserves_order() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids: Vec<i64> = vec![all[2].id, all[0].id, all[1].id];
    let result = queries::track::get_tracks_by_ids(&db, &ids).await?;
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].id, all[2].id);
    assert_eq!(result[1].id, all[0].id);
    assert_eq!(result[2].id, all[1].id);
    Ok(())
}

#[tokio::test]
async fn get_tracks_by_ids_skips_missing() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids = vec![all[0].id, 99999];
    let result = queries::track::get_tracks_by_ids(&db, &ids).await?;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].id, all[0].id);
    Ok(())
}

#[tokio::test]
async fn update_play_count_increments() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    queries::track::update_play_count(&db, id).await?;
    queries::track::update_play_count(&db, id).await?;

    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.play_count, 2);
    assert!(t.last_played.is_some());
    Ok(())
}

#[tokio::test]
async fn update_skip_count_increments() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    queries::track::update_skip_count(&db, id).await?;

    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.skip_count, 1);
    Ok(())
}

#[tokio::test]
async fn update_last_position() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    queries::track::update_last_position(&db, id, 45_000).await?;

    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.last_position, 45_000);
    Ok(())
}

#[tokio::test]
async fn get_tracks_in_directory_direct_only() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    // Use the platform's native separator — `get_tracks_in_directory`
    // builds its LIKE pattern with `MAIN_SEPARATOR`, so Unix-style `/`
    // hardcoded test paths wouldn't match on Windows.
    let sep = std::path::MAIN_SEPARATOR_STR;
    let dir = format!("{sep}music");
    let direct = format!("{dir}{sep}song.mp3");
    let nested = format!("{dir}{sep}sub{sep}nested.mp3");

    queries::folder::insert_folder(&db, &dir, true).await?;
    insert_test_track(&db, &direct, "Direct", "A", "B", "C").await?;
    insert_test_track(&db, &nested, "Nested", "A", "B", "C").await?;

    let tracks = queries::track::get_tracks_in_directory(&db, &dir).await?;
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "Direct");
    Ok(())
}

#[tokio::test]
async fn get_track_ids_by_paths() -> Result<(), AppError> {
    let db = seed_db().await?;
    let paths = vec![
        "/music/alpha.mp3".to_owned(),
        "/music/beta.mp3".to_owned(),
        "/nonexistent.mp3".to_owned(),
    ];
    let map = queries::track::get_track_ids_by_paths(&db, &paths).await?;
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("/music/alpha.mp3"));
    assert!(map.contains_key("/music/beta.mp3"));
    assert!(!map.contains_key("/nonexistent.mp3"));
    Ok(())
}

#[tokio::test]
async fn get_track_ids_by_hashes() -> Result<(), AppError> {
    let db = seed_db().await?;
    // `make_test_metadata` sets file_hash = blake3(title), so each seeded
    // track's hash is deterministic from its title.
    let alpha = blake3::hash(b"Alpha").to_hex().to_string();
    let beta = blake3::hash(b"Beta").to_hex().to_string();
    let unknown = blake3::hash(b"Nope").to_hex().to_string();

    let map = queries::track::get_track_ids_by_hashes(
        &db,
        &[alpha.clone(), beta.clone(), unknown.clone()],
    )
    .await?;

    assert_eq!(map.len(), 2);
    assert!(map.contains_key(&alpha));
    assert!(map.contains_key(&beta));
    assert!(!map.contains_key(&unknown));

    // The alpha hash resolves to the same id as the alpha path.
    let by_path = queries::track::get_track_ids_by_paths(&db, &["/music/alpha.mp3".to_owned()])
        .await?;
    assert_eq!(map.get(&alpha), by_path.get("/music/alpha.mp3"));
    Ok(())
}

// --- Duplicate detection tests ---

#[tokio::test]
async fn get_duplicate_tracks_returns_groups() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Insert two tracks with the same hash (simulating duplicate files)
    let id1 = insert_test_track(&db, "/music/copy1.mp3", "Song", "Art", "Alb", "Rock").await?;
    let id2 = insert_test_track(&db, "/music/copy2.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Set both to the same hash
    let shared_hash = "d".repeat(64);
    sqlx::query("UPDATE tracks SET file_hash = ? WHERE id IN (?, ?)")
        .bind(&shared_hash)
        .bind(id1)
        .bind(id2)
        .execute(db.write())
        .await?;

    let groups = queries::track::get_duplicate_tracks(&db).await?;
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
    Ok(())
}

#[tokio::test]
async fn get_duplicate_tracks_empty_when_no_dupes() -> Result<(), AppError> {
    let db = seed_db().await?;
    // Each track from seed_db has a unique hash (default from make_test_metadata)
    let groups = queries::track::get_duplicate_tracks(&db).await?;
    assert!(groups.is_empty());
    Ok(())
}

// --- Batch hash update tests ---

#[tokio::test]
async fn batch_update_hashes_sets_values() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/song.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Clear the hash to simulate an old track
    sqlx::query("UPDATE tracks SET file_hash = NULL WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;

    let new_hash = "e".repeat(64);
    let mtime = Some("2025-01-01T00:00:00+00:00".to_owned());
    queries::track::batch_update_hashes(&db, &[(id, new_hash.clone(), mtime.clone())]).await?;

    let row: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT file_hash, date_modified FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(db.read())
            .await?;
    assert_eq!(row.0.as_deref(), Some(new_hash.as_str()));
    assert_eq!(row.1, mtime);
    Ok(())
}

#[tokio::test]
async fn get_unhashed_track_paths_finds_null_hashes() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/song.mp3", "Song", "Art", "Alb", "Rock").await?;

    // Clear the hash
    sqlx::query("UPDATE tracks SET file_hash = NULL WHERE id = ?")
        .bind(id)
        .execute(db.write())
        .await?;

    let unhashed = queries::track::get_unhashed_track_paths(&db).await?;
    assert_eq!(unhashed.len(), 1);
    assert_eq!(unhashed[0].0, id);
    assert_eq!(unhashed[0].1, "/music/song.mp3");
    Ok(())
}

// --- Favorites tests ---

#[tokio::test]
async fn set_favorite_flips_flag() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    // Initially not favorite
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert!(!t.is_favorite);

    // Set to favorite
    queries::track::set_favorite(&db, &[id], true).await?;
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert!(t.is_favorite);

    // Set back to not favorite
    queries::track::set_favorite(&db, &[id], false).await?;
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert!(!t.is_favorite);
    Ok(())
}

#[tokio::test]
async fn get_favorite_tracks_returns_only_favorites() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;

    // Favorite the first track
    queries::track::set_favorite(&db, &[all[0].id], true).await?;

    let favs = queries::track::get_favorite_tracks_for_list(&db, None, None).await?;
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0].id, all[0].id);
    assert!(favs[0].is_favorite);
    Ok(())
}

#[tokio::test]
async fn get_favorite_tracks_respects_sort() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();

    // Favorite all tracks
    queries::track::set_favorite(&db, &ids, true).await?;

    // Sort desc by title
    let favs = queries::track::get_favorite_tracks_for_list(
        &db,
        Some("title".to_owned()),
        Some("desc".to_owned()),
    )
    .await?;
    assert_eq!(favs.len(), 3);
    assert_eq!(favs[0].title, "Gamma");
    assert_eq!(favs[2].title, "Alpha");
    Ok(())
}

#[tokio::test]
async fn set_favorite_empty_ids_is_noop() -> Result<(), AppError> {
    let db = seed_db().await?;
    // Should not error
    queries::track::set_favorite(&db, &[], true).await?;
    Ok(())
}

#[tokio::test]
async fn set_rating_updates_value() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    // Default rating is 0 (unrated).
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.rating, 0);

    queries::track::set_rating(&db, &[id], 4).await?;
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.rating, 4);

    // Clearing back to 0 works.
    queries::track::set_rating(&db, &[id], 0).await?;
    let t = queries::track::get_track_by_id(&db, id).await?;
    assert_eq!(t.rating, 0);
    Ok(())
}

#[tokio::test]
async fn set_rating_targets_only_given_ids() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    assert!(all.len() >= 2);

    queries::track::set_rating(&db, &[all[0].id], 5).await?;

    let rated = queries::track::get_track_by_id(&db, all[0].id).await?;
    assert_eq!(rated.rating, 5);
    // Untargeted rows stay at the default 0.
    for other in &all[1..] {
        let t = queries::track::get_track_by_id(&db, other.id).await?;
        assert_eq!(t.rating, 0, "id {} must be untouched", other.id);
    }
    Ok(())
}

#[tokio::test]
async fn set_rating_empty_ids_is_noop() -> Result<(), AppError> {
    let db = seed_db().await?;
    // Should not error.
    queries::track::set_rating(&db, &[], 3).await?;
    Ok(())
}

#[tokio::test]
async fn get_favorite_stats_orders_artwork_by_play_count() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    // Distinct artworks — first pass returns these in play_count DESC order.
    sqlx::query("UPDATE tracks SET artwork_path = '/art/alpha.jpg', play_count = 1 WHERE title = 'Alpha'")
        .execute(db.write()).await?;
    sqlx::query("UPDATE tracks SET artwork_path = '/art/beta.jpg', play_count = 9 WHERE title = 'Beta'")
        .execute(db.write()).await?;
    sqlx::query("UPDATE tracks SET artwork_path = '/art/gamma.jpg', play_count = 5 WHERE title = 'Gamma'")
        .execute(db.write()).await?;

    let stats = queries::track::get_favorite_stats(&db).await?;
    assert_eq!(stats.count, 3);
    // Distinct artworks only, ordered by play_count DESC. The Slint
    // `CoverMosaic` paints placeholder tiles for slots beyond
    // `artwork_paths.len()` when `pad-to-four: true` is set — the SQL
    // does *not* duplicate paths to reach 4.
    assert_eq!(
        stats.artwork_paths,
        vec![
            "/art/beta.jpg".to_owned(),
            "/art/gamma.jpg".to_owned(),
            "/art/alpha.jpg".to_owned(),
        ],
        "artwork_paths must be the distinct set in play_count DESC order"
    );
    Ok(())
}

#[tokio::test]
async fn get_favorite_stats_returns_distinct_artworks_no_duplicates() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    // All three seed tracks share one artwork_path (single-album-heavy
    // library). The SQL must return that one distinct artwork *once*,
    // not duplicate it to fill 4 slots — the Slint `CoverMosaic`'s
    // `pad-to-four` flag is what paints placeholder tiles for the
    // missing 3 slots.
    sqlx::query(
        "UPDATE tracks SET artwork_path = '/art/single.jpg' WHERE title IN ('Alpha', 'Beta', 'Gamma')",
    )
    .execute(db.write())
    .await?;

    let stats = queries::track::get_favorite_stats(&db).await?;
    assert_eq!(stats.count, 3);
    assert_eq!(
        stats.artwork_paths,
        vec!["/art/single.jpg".to_owned()],
        "only the one distinct artwork is returned; the mosaic component handles padding"
    );
    Ok(())
}

#[tokio::test]
async fn get_favorite_stats_empty_when_favorites_have_no_artwork() -> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db, None, None).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    // Seed leaves artwork_path as NULL — favorites exist but none have
    // covers. The mosaic should render no tiles so the FavoritesView's
    // outer `favorite_border` placeholder is what the user sees.
    let stats = queries::track::get_favorite_stats(&db).await?;
    assert_eq!(stats.count, 3);
    assert!(
        stats.artwork_paths.is_empty(),
        "no artworks among favorites ⇒ empty list, got {:?}",
        stats.artwork_paths
    );
    Ok(())
}

#[tokio::test]
async fn get_recently_played_orders_newest_first_and_excludes_null() -> Result<(), AppError> {
    let db = seed_db().await?;

    // Distinct, lexically-ordered RFC-3339 timestamps on two tracks; leave
    // Gamma's `last_played` NULL (never played) so it must be excluded.
    sqlx::query("UPDATE tracks SET last_played = '2026-01-01T00:00:00+00:00' WHERE title = 'Alpha'")
        .execute(db.write())
        .await?;
    sqlx::query("UPDATE tracks SET last_played = '2026-06-01T00:00:00+00:00' WHERE title = 'Beta'")
        .execute(db.write())
        .await?;

    let rows = queries::track::get_recently_played(&db, 200).await?;
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Beta", "Alpha"],
        "newest-played first; the NULL-last_played track is excluded"
    );
    Ok(())
}

#[tokio::test]
async fn get_recently_played_respects_limit() -> Result<(), AppError> {
    let db = seed_db().await?;
    sqlx::query("UPDATE tracks SET last_played = '2026-01-01T00:00:00+00:00' WHERE title = 'Alpha'")
        .execute(db.write())
        .await?;
    sqlx::query("UPDATE tracks SET last_played = '2026-06-01T00:00:00+00:00' WHERE title = 'Beta'")
        .execute(db.write())
        .await?;

    let rows = queries::track::get_recently_played(&db, 1).await?;
    assert_eq!(rows.len(), 1, "LIMIT caps the result set");
    assert_eq!(rows[0].title, "Beta", "the single row is the most recent");
    Ok(())
}

#[tokio::test]
async fn get_most_played_orders_by_count_and_excludes_zero() -> Result<(), AppError> {
    let db = seed_db().await?;

    // Beta highest, Alpha lower, Gamma left at play_count 0 (must be excluded).
    sqlx::query("UPDATE tracks SET play_count = 3 WHERE title = 'Alpha'")
        .execute(db.write())
        .await?;
    sqlx::query("UPDATE tracks SET play_count = 9 WHERE title = 'Beta'")
        .execute(db.write())
        .await?;

    let rows = queries::track::get_most_played(&db, 200).await?;
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Beta", "Alpha"],
        "play_count DESC; the play_count == 0 track is excluded (no favorite filter)"
    );
    Ok(())
}

#[tokio::test]
async fn get_most_played_respects_limit() -> Result<(), AppError> {
    let db = seed_db().await?;
    sqlx::query("UPDATE tracks SET play_count = 3 WHERE title = 'Alpha'")
        .execute(db.write())
        .await?;
    sqlx::query("UPDATE tracks SET play_count = 9 WHERE title = 'Beta'")
        .execute(db.write())
        .await?;

    let rows = queries::track::get_most_played(&db, 1).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Beta");
    Ok(())
}
