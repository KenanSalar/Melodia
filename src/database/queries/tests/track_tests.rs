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

/// [`seed_db`]'s three tracks, inserted **back-to-front** so that rowid order
/// is the reverse of `sort_key` order. Identical in every other respect.
///
/// The two ordering pins below need it, and neither works without it.
/// `seed_db` inserts already sorted, so there rowid order and `sort_key` order
/// are the same sequence and an assertion over it is satisfied by a bare table
/// scan — the `ORDER BY` can go missing with nothing failing, which is exactly
/// what happened to the test this one replaced.
///
/// `seed_db` itself can't just be reversed:
/// [`the_hero_mosaic_leads_with_the_covers_the_most_played_tab_shows`] reads its
/// insertion order as the rowid order its tiebreakers have to *beat*, so
/// flipping it would hand that test the answer it exists to prove.
async fn seed_db_inserted_backwards() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/gamma.mp3", "Gamma", "Alpha Artist", "A Album", "Rock").await?;
    insert_test_track(&db, "/music/beta.mp3", "Beta", "Alpha Artist", "A Album", "Rock").await?;
    insert_test_track(&db, "/music/alpha.mp3", "Alpha", "Zeta Artist", "B Album", "Pop").await?;
    Ok(db)
}

/// The whole-table fetches hand back one fixed order, and both retained-row
/// views document it as the order they permute *from*. It stopped being a
/// default when `track_list_order_by`'s other arms went — nothing asks for
/// anything else — so this is now the only ordering claim SQL makes about a
/// track list, and the one worth pinning.
///
/// Seeded backwards, because the ordinary fixture cannot pin it — see
/// [`seed_db_inserted_backwards`].
#[tokio::test]
async fn a_whole_table_fetch_comes_back_in_sort_key_order() -> Result<(), AppError> {
    let db = seed_db_inserted_backwards().await?;
    let tracks = queries::track::get_all_tracks(&db).await?;
    let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Alpha", "Beta", "Gamma"]);
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
    let all = queries::track::get_all_tracks(&db).await?;
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
    let all = queries::track::get_all_tracks(&db).await?;
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
    let all = queries::track::get_all_tracks(&db).await?;

    // Favorite the first track
    queries::track::set_favorite(&db, &[all[0].id], true).await?;

    let favs = queries::track::get_favorite_tracks_for_list(&db).await?;
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0].id, all[0].id);
    assert!(favs[0].is_favorite);
    Ok(())
}

#[tokio::test]
async fn the_favorites_fetch_shares_the_whole_table_order() -> Result<(), AppError> {
    // The Songs tab hands its own permutation to `store_in_order`, computed
    // before the section guards let it store — so the two orders only line up
    // because this fetch and `get_all_tracks_for_list` share one clause.
    // Backwards-seeded for the reason the whole-table pin above is.
    let db = seed_db_inserted_backwards().await?;
    let all = queries::track::get_all_tracks(&db).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    let favs = queries::track::get_favorite_tracks_for_list(&db).await?;
    let titles: Vec<&str> = favs.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Alpha", "Beta", "Gamma"]);
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
    let all = queries::track::get_all_tracks(&db).await?;
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
    let all = queries::track::get_all_tracks(&db).await?;
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
    let all = queries::track::get_all_tracks(&db).await?;
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
    let all = queries::track::get_all_tracks(&db).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    // Seed leaves artwork_path as NULL — favorites exist but none have
    // covers. The mosaic should render no tiles so the FavoritesView's
    // outer `favorite_border` placeholder is what the user sees.
    //
    // One of them carries `''` instead: the scan path treats an empty
    // artwork_path as "no cover" in the same breath as NULL
    // (`scan::mutations::update_track_artwork_if_missing`), so it is a value
    // that reaches this table — and it must not take a slot it can't paint.
    sqlx::query("UPDATE tracks SET artwork_path = '' WHERE title = 'Alpha'")
        .execute(db.write())
        .await?;

    let stats = queries::track::get_favorite_stats(&db).await?;
    assert_eq!(stats.count, 3);
    assert!(
        stats.artwork_paths.is_empty(),
        "no artworks among favorites ⇒ empty list, got {:?}",
        stats.artwork_paths
    );
    Ok(())
}

/// The hero mosaic and the Most Played tab are the same list seen two ways, so
/// they have to resolve a tie the same way. The mosaic used to rank distinct
/// covers by `MAX(play_count)` under a tiebreaker of its own, and the grid broke
/// ties not at all — so on a tie they picked different winners, and the grid's
/// own order could move between refreshes. Both now read `MOST_PLAYED_ORDER`.
#[tokio::test]
async fn the_hero_mosaic_leads_with_the_covers_the_most_played_tab_shows()
-> Result<(), AppError> {
    let db = seed_db().await?;
    let all = queries::track::get_all_tracks(&db).await?;
    let ids: Vec<i64> = all.iter().map(|t| t.id).collect();
    queries::track::set_favorite(&db, &ids, true).await?;

    // Alpha and Beta are level on plays and Beta was played more recently, so
    // Beta leads. Gamma is a favorite nobody has played, carrying a cover of
    // its own — it's what pads the mosaic once the played covers run out. Every
    // cover here is distinct, so "first four" and "first four distinct" coincide
    // and the assertions below can stay about ordering.
    //
    // Which of the pair is the recent one is the whole fixture, because both
    // orders this has to reject put *Alpha* first. Drop the tiebreakers and
    // `SQLite` sorts the tied pair into a temp B-tree in rowid order, i.e. the
    // order `seed_db` inserted them. Restore the mosaic's old `MAX(date_added)
    // DESC` and it follows insertion too — so `date_added` is written here
    // against recency rather than left to the seed, and a fixture with Alpha as
    // the recent one would pass against all three queries and pin nothing.
    sqlx::query(
        "UPDATE tracks SET artwork_path = '/art/alpha.jpg', play_count = 4, \
         last_played = '2026-01-01T00:00:00+00:00', date_added = '2026-05-01T00:00:00+00:00' \
         WHERE title = 'Alpha'",
    )
    .execute(db.write())
    .await?;
    sqlx::query(
        "UPDATE tracks SET artwork_path = '/art/beta.jpg', play_count = 4, \
         last_played = '2026-06-01T00:00:00+00:00', date_added = '2026-01-01T00:00:00+00:00' \
         WHERE title = 'Beta'",
    )
    .execute(db.write())
    .await?;
    sqlx::query("UPDATE tracks SET artwork_path = '/art/gamma.jpg' WHERE title = 'Gamma'")
        .execute(db.write())
        .await?;

    let grid = queries::track::get_most_played_favorites(&db).await?;
    let titles: Vec<&str> = grid.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Beta", "Alpha"],
        "a tie on play_count breaks toward the track played most recently"
    );

    // Derived from the grid rather than restated, so the day the two clauses
    // drift apart again this fails here instead of only on screen.
    let mut expected: Vec<String> = grid.iter().filter_map(|t| t.artwork_path.clone()).collect();
    expected.push("/art/gamma.jpg".to_owned());

    let stats = queries::track::get_favorite_stats(&db).await?;
    assert_eq!(
        stats.artwork_paths, expected,
        "the mosaic must lead with the Most Played tab's covers in its order, then pad from the \
         favorites that tab excludes"
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
    insert_test_track(&db, "/music/delta.mp3", "Delta", "Zeta Artist", "B Album", "Pop").await?;

    // Beta highest; Alpha and Delta tie on count and are separated by recency
    // alone; Gamma left at play_count 0 (must be excluded). The tie is the part
    // worth having — `play_count DESC` on its own leaves it to the planner, and
    // this strip re-fetches on every `stats_changed` tick, so the cards could
    // reshuffle with nothing about the library having moved.
    //
    // Alpha is the recent one on purpose. Without the tiebreakers this query
    // walks the partial `idx_tracks_play_count` backwards, which hands back a
    // tied group newest-rowid-first — so Delta, inserted last, is what the
    // un-tiebroken order puts ahead, and only a fixture pointing the other way
    // can tell the two apart.
    sqlx::query(
        "UPDATE tracks SET play_count = 3, last_played = '2026-06-01T00:00:00+00:00' \
         WHERE title = 'Alpha'",
    )
    .execute(db.write())
    .await?;
    sqlx::query(
        "UPDATE tracks SET play_count = 3, last_played = '2026-01-01T00:00:00+00:00' \
         WHERE title = 'Delta'",
    )
    .execute(db.write())
    .await?;
    sqlx::query("UPDATE tracks SET play_count = 9 WHERE title = 'Beta'")
        .execute(db.write())
        .await?;

    let rows = queries::track::get_most_played(&db).await?;
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Beta", "Alpha", "Delta"],
        "play_count DESC, then most-recently-played first; the play_count == 0 track is \
         excluded (no favorite filter)"
    );
    Ok(())
}

/// The tab this feeds is a virtualized grid, so it takes the whole set — the
/// `LIMIT 10` it carried was sized for the ten-card carousel it replaced, and
/// re-adding one is a ceiling the user can scroll into with nothing saying why
/// the list stops. Seeded past that old cap so a reintroduced `LIMIT` fails here
/// rather than only on a library large enough to notice.
#[tokio::test]
async fn get_most_played_is_not_capped() -> Result<(), AppError> {
    const PLAYED: i64 = 12;

    let db = seed_db().await?;
    for n in 0..PLAYED {
        let path = format!("/music/played-{n}.mp3");
        insert_test_track(&db, &path, &format!("Played {n}"), "Zeta Artist", "B Album", "Pop")
            .await?;
        sqlx::query("UPDATE tracks SET play_count = ? WHERE file_path = ?")
            .bind(n + 1)
            .bind(&path)
            .execute(db.write())
            .await?;
    }

    let rows = queries::track::get_most_played(&db).await?;
    assert_eq!(
        i64::try_from(rows.len()).unwrap_or(-1),
        PLAYED,
        "every played track must come back — the seeded set is `play_count > 0` and the three \
         `seed_db` rows are left at zero"
    );
    Ok(())
}

#[tokio::test]
async fn both_most_played_queries_project_what_the_filter_searches() -> Result<(), AppError> {
    // The cards render title + artist, so a `SELECT` that drops one of the
    // other four still compiles and still paints correctly — it only stops
    // the hero search bar narrowing the card grid the way it narrows the
    // track list beside it. `FromRow` catches a missing column at run time,
    // which is why this asserts the values rather than just the row count.
    let db = seed_db().await?;
    sqlx::query(
        "UPDATE tracks SET play_count = 5, is_favorite = TRUE, album_artist = 'Various Artists' \
         WHERE title = 'Alpha'",
    )
    .execute(db.write())
    .await?;

    let favorites = queries::track::get_most_played_favorites(&db).await?;
    let all = queries::track::get_most_played(&db).await?;

    for (label, rows) in [("favorites", favorites), ("all", all)] {
        let card = rows.iter().find(|t| t.title == "Alpha");
        assert_eq!(
            card.map(|c| (
                c.artist.as_deref(),
                c.album_artist.as_deref(),
                c.album.as_deref(),
                c.genre.as_deref(),
                c.year,
            )),
            Some((
                Some("Zeta Artist"),
                Some("Various Artists"),
                Some("B Album"),
                Some("Pop"),
                Some(2024),
            )),
            "{label}: the most-played projection dropped a field the filter searches"
        );
    }
    Ok(())
}

// --- Tag-edit query tests ---

#[tokio::test]
async fn get_tag_edit_rows_by_ids_projects_and_preserves_order() -> Result<(), AppError> {
    let db = seed_db().await?;
    // ids ascending == insert order: Alpha, Beta, Gamma.
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tracks ORDER BY id")
        .fetch_all(db.read())
        .await?;

    // Patch the fields `make_test_metadata` leaves empty so the projection is exercised.
    sqlx::query(
        "UPDATE tracks SET composer = ?, comment = ?, bpm = ?, original_year = ? WHERE id = ?",
    )
    .bind("Composer X")
    .bind("A comment")
    .bind(128.5_f64)
    .bind(1999_i32)
    .bind(ids[0])
    .execute(db.write())
    .await?;

    // Request reversed so a plain re-read couldn't accidentally pass.
    let rows = queries::track::get_tag_edit_rows_by_ids(&db, &[ids[1], ids[0]]).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, ids[1]);
    assert_eq!(rows[1].id, ids[0]);

    let alpha = &rows[1];
    assert_eq!(alpha.title, "Alpha");
    assert_eq!(alpha.composer.as_deref(), Some("Composer X"));
    assert_eq!(alpha.comment.as_deref(), Some("A comment"));
    assert_eq!(alpha.original_year, Some(1999));
    assert!(matches!(alpha.bpm, Some(b) if (b - 128.5).abs() < 1e-9));
    // A technical column reads straight off `tracks`.
    assert_eq!(alpha.codec.as_deref(), Some("Mpeg"));
    assert_eq!(alpha.bitrate, Some(320));
    assert_eq!(alpha.duration_ms, 180_000);
    Ok(())
}

#[tokio::test]
async fn get_track_paths_by_ids_returns_pairs_in_input_order() -> Result<(), AppError> {
    let db = seed_db().await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tracks ORDER BY id")
        .fetch_all(db.read())
        .await?;

    let pairs = queries::track::get_track_paths_by_ids(&db, &[ids[2], ids[0]]).await?;
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0], (ids[2], "/music/gamma.mp3".to_owned()));
    assert_eq!(pairs[1], (ids[0], "/music/alpha.mp3".to_owned()));

    let empty = queries::track::get_track_paths_by_ids(&db, &[]).await?;
    assert!(empty.is_empty());
    Ok(())
}

#[tokio::test]
async fn set_track_artwork_overwrites_and_nulls() -> Result<(), AppError> {
    let db = seed_db().await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM tracks LIMIT 1")
        .fetch_one(db.read())
        .await?;

    // Set an authoritative path within a transaction.
    let mut tx = db.write().begin().await?;
    queries::track::set_track_artwork(&mut tx, &[id], Some("/covers/new.jpg")).await?;
    tx.commit().await?;

    let art: Option<String> = sqlx::query_scalar("SELECT artwork_path FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    assert_eq!(art.as_deref(), Some("/covers/new.jpg"));

    // `None` genuinely nulls it — no COALESCE keeping the old value.
    let mut tx = db.write().begin().await?;
    queries::track::set_track_artwork(&mut tx, &[id], None).await?;
    tx.commit().await?;

    let art: Option<String> = sqlx::query_scalar("SELECT artwork_path FROM tracks WHERE id = ?")
        .bind(id)
        .fetch_one(db.read())
        .await?;
    assert!(art.is_none());
    Ok(())
}

#[tokio::test]
async fn get_tracks_missing_mbid_filters_tagged_and_metadata_less_rows() -> Result<(), AppError> {
    let db = seed_db().await?;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM tracks ORDER BY file_path")
        .fetch_all(db.read())
        .await?;
    // alpha/beta/gamma all start eligible (NULL mbid, non-empty artist + title).
    // Tag one, and blank another's artist — both must drop out of the work-list.
    sqlx::query("UPDATE tracks SET musicbrainz_track_id = 'rec-1' WHERE id = ?")
        .bind(ids[0])
        .execute(db.write())
        .await?;
    sqlx::query("UPDATE tracks SET artist = '' WHERE id = ?")
        .bind(ids[1])
        .execute(db.write())
        .await?;

    let missing = queries::track::get_tracks_missing_mbid(&db).await?;
    let returned: Vec<i64> = missing.iter().map(|(id, ..)| *id).collect();
    assert_eq!(returned, vec![ids[2]], "only the untagged, artist-bearing track");
    // The row carries the fields the lookup needs.
    let (_id, path, artist, title, _album) = &missing[0];
    assert!(path.ends_with("gamma.mp3"));
    assert_eq!(artist, "Alpha Artist");
    assert_eq!(title, "Gamma");
    Ok(())
}
