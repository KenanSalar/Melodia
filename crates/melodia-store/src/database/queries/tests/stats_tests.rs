use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::fixtures::*;
use melodia_core::entities::artist::{self, ArtistCredit};
use melodia_core::entities::genre::{self, GenreList};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

#[tokio::test]
async fn recalculate_stats_produces_correct_counts() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Disable triggers so inserts don't update stats
    let mut tx = db.write().begin().await?;
    queries::stats::disable_stats_triggers(&mut tx).await?;
    tx.commit().await?;

    // Insert tracks — stats will remain at 0 since triggers are disabled
    insert_test_track(&db, "/music/t1.mp3", "Song A", "Artist X", "Album P", "Rock").await?;
    insert_test_track(&db, "/music/t2.mp3", "Song B", "Artist X", "Album P", "Rock").await?;
    insert_test_track(&db, "/music/t3.mp3", "Song C", "Artist Y", "Album Q", "Pop").await?;

    // Verify stats are 0 (triggers disabled)
    let artist_x: (i64, i64, i64) = sqlx::query_as(
        "SELECT track_count, total_duration_ms, album_count FROM artists WHERE name = 'Artist X'",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(artist_x, (0, 0, 0), "stats should be zero with triggers disabled");

    // Recalculate
    let mut tx = db.write().begin().await?;
    queries::stats::recalculate_all_stats(&mut tx).await?;
    tx.commit().await?;

    // Verify artist stats
    let artist_x: (i64, i64, i64) = sqlx::query_as(
        "SELECT track_count, total_duration_ms, album_count FROM artists WHERE name = 'Artist X'",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(artist_x.0, 2, "Artist X should have 2 tracks");
    assert_eq!(artist_x.1, 360_000, "Artist X total_duration_ms = 2 * 180_000");
    assert_eq!(artist_x.2, 1, "Artist X should have 1 album");

    let artist_y: (i64, i64, i64) = sqlx::query_as(
        "SELECT track_count, total_duration_ms, album_count FROM artists WHERE name = 'Artist Y'",
    )
    .fetch_one(db.read())
    .await?;
    assert_eq!(artist_y.0, 1);
    assert_eq!(artist_y.2, 1);

    // Verify album stats
    let album_p: (i64, i64) =
        sqlx::query_as("SELECT track_count, total_duration_ms FROM albums WHERE name = 'Album P'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(album_p.0, 2, "Album P should have 2 tracks");
    assert_eq!(album_p.1, 360_000);

    // Verify genre stats
    let rock: (i64, i64) =
        sqlx::query_as("SELECT track_count, total_duration_ms FROM genres WHERE name = 'Rock'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(rock.0, 2, "Rock should have 2 tracks");
    Ok(())
}

#[tokio::test]
async fn triggers_work_after_re_enable() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Disable, then re-enable triggers
    let mut tx = db.write().begin().await?;
    queries::stats::disable_stats_triggers(&mut tx).await?;
    queries::stats::enable_stats_triggers(&mut tx).await?;
    tx.commit().await?;

    // Insert a track — trigger should fire and update stats
    insert_test_track(&db, "/music/t1.mp3", "Song A", "Artist X", "Album P", "Rock").await?;

    let artist_x: (i64, i64) =
        sqlx::query_as("SELECT track_count, album_count FROM artists WHERE name = 'Artist X'")
            .fetch_one(db.read())
            .await?;
    assert_eq!(artist_x.0, 1, "trigger should increment track_count");
    assert_eq!(artist_x.1, 1, "trigger should set album_count");
    Ok(())
}

#[tokio::test]
async fn disable_enable_recalculate_full_cycle() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    // Full cycle: disable → bulk insert → recalculate → enable
    let mut tx = db.write().begin().await?;
    queries::stats::disable_stats_triggers(&mut tx).await?;
    tx.commit().await?;

    insert_test_track(&db, "/music/t1.mp3", "S1", "Art", "Alb", "Genre").await?;
    insert_test_track(&db, "/music/t2.mp3", "S2", "Art", "Alb", "Genre").await?;

    let mut tx = db.write().begin().await?;
    queries::stats::recalculate_all_stats(&mut tx).await?;
    queries::stats::enable_stats_triggers(&mut tx).await?;
    tx.commit().await?;

    // Stats should be correct from recalculate
    let art: (i64,) = sqlx::query_as("SELECT track_count FROM artists WHERE name = 'Art'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(art.0, 2);

    // Now insert one more track — trigger should fire normally
    insert_test_track(&db, "/music/t3.mp3", "S3", "Art", "Alb", "Genre").await?;

    let art: (i64,) = sqlx::query_as("SELECT track_count FROM artists WHERE name = 'Art'")
        .fetch_one(db.read())
        .await?;
    assert_eq!(art.0, 3, "trigger should increment from recalculated base");
    Ok(())
}

// === The join tables the stats moved onto ===

/// A track carrying two genres and a guest credit — the shape every count below is maintained
/// off, and the one `insert_test_track`'s flat arguments cannot spell.
fn multi_tagged(
    title: &str,
    genres: &[&str],
    printed: &str,
    credited: &[&str],
) -> ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.genres = GenreList::new(genres.iter().map(|g| (*g).to_owned()).collect());
    meta.artist = ArtistCredit::from_tags(
        printed,
        &credited.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>(),
    );
    meta
}

/// A page's length in the same type its count is stored as, so the two can be asserted together.
fn listed(rows: &[melodia_core::entities::track::TrackListRow]) -> i32 {
    i32::try_from(rows.len()).unwrap_or(i32::MAX)
}

async fn genre_named(db: &DbPool, name: &str) -> Result<genre::GenreStats, AppError> {
    let all = queries::genre::get_all_genres(db).await?;
    all.into_iter()
        .find(|g| g.name == name)
        .ok_or_else(|| AppError::Validation(format!("no genre named {name}")))
}

/// A genre's count straight off the table, for the rows `genre_stats` filters out at zero.
async fn stored_genre_count(db: &DbPool, name: &str) -> Result<i64, AppError> {
    let count: (i64,) = sqlx::query_as("SELECT track_count FROM genres WHERE name = ?")
        .bind(name)
        .fetch_one(db.read())
        .await?;
    Ok(count.0)
}

async fn artist_named(db: &DbPool, name: &str) -> Result<artist::ArtistStats, AppError> {
    let all = queries::artist::get_all_artists(db).await?;
    all.into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| AppError::Validation(format!("no artist named {name}")))
}

/// **A count and the page under it have to describe the same set.** `genres.track_count` is
/// maintained off `track_genres`, so a reader on `tracks.genre_id` stated a number over a list
/// holding a subset of it — a card claiming tracks the page then failed to show.
#[tokio::test]
async fn a_genre_counts_exactly_the_tracks_its_page_lists() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let both = multi_tagged("Song A", &["Rock", "Metal"], "Artist X", &[]);
    insert_tagged_track(&db, "/music/t1.mp3", &both).await?;
    let rock_only = multi_tagged("Song B", &["Rock"], "Artist X", &[]);
    insert_tagged_track(&db, "/music/t2.mp3", &rock_only).await?;

    let rock = genre_named(&db, "Rock").await?;
    let metal = genre_named(&db, "Metal").await?;
    let rock_page = queries::track::get_tracks_by_genre_for_list(&db, rock.id).await?;
    let metal_page = queries::track::get_tracks_by_genre_for_list(&db, metal.id).await?;

    assert_eq!((rock.track_count, listed(&rock_page)), (2, 2));
    // The secondary genre is the one a `tracks.genre_id` reader listed nothing at all for.
    assert_eq!((metal.track_count, listed(&metal_page)), (1, 1));
    Ok(())
}

/// The same agreement one entity over: an artist who is never anybody's primary credit is exactly
/// what the credit tables were added to surface, so a count that reaches them and a page that
/// doesn't is the same defect wearing a different name.
#[tokio::test]
async fn a_guest_credit_is_counted_and_listed_under_the_artist_it_names() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let featured = multi_tagged("Song A", &["Rock"], "Alice feat. Bob", &["Alice", "Bob"]);
    insert_tagged_track(&db, "/music/t1.mp3", &featured).await?;

    let bob = artist_named(&db, "Bob").await?;
    let bob_page = queries::track::get_tracks_by_artist_for_list(&db, bob.id).await?;

    assert_eq!((bob.track_count, listed(&bob_page)), (1, 1));
    // The whole credit line is not an artist — that row is what keying on it used to create.
    assert!(artist_named(&db, "Alice feat. Bob").await.is_err());
    Ok(())
}

/// Two implementations of one arithmetic, and nothing else can check either: the triggers
/// accumulate row by row while the recompute reads the join tables in bulk.
#[tokio::test]
async fn the_recompute_reproduces_what_the_triggers_accumulated() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    for (n, genres, printed, credited) in [
        (1, &["Rock", "Metal"][..], "Alice feat. Bob", &["Alice", "Bob"][..]),
        (2, &["Rock"][..], "Alice", &[][..]),
        (3, &["Metal", "Rock"][..], "Bob & Carol", &["Bob", "Carol"][..]),
    ] {
        let meta = multi_tagged(&format!("Song {n}"), genres, printed, credited);
        insert_tagged_track(&db, &format!("/music/t{n}.mp3"), &meta).await?;
    }

    let accumulated = every_stat(&db).await?;

    let mut tx = db.write().begin().await?;
    queries::stats::recalculate_all_stats(&mut tx).await?;
    tx.commit().await?;

    assert_eq!(every_stat(&db).await?, accumulated);
    Ok(())
}

/// Every maintained stat, ordered by name so the two reads are comparable.
async fn every_stat(db: &DbPool) -> Result<Vec<(String, i64, i64, i64)>, AppError> {
    let rows = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "SELECT 'artist:' || name, track_count, total_duration_ms, album_count FROM artists \
         UNION ALL \
         SELECT 'genre:' || name, track_count, total_duration_ms, 0 FROM genres \
         UNION ALL \
         SELECT 'album:' || name, track_count, total_duration_ms, 0 FROM albums \
         ORDER BY 1",
    )
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}

/// The credit tables carry no `ON DELETE CASCADE` — a cascade fires after the parent row is gone,
/// leaving the stats trigger no duration to subtract — so a `BEFORE DELETE` trigger clears them
/// while the track still exists. Lose either half and a track becomes undeletable.
#[tokio::test]
async fn deleting_a_track_subtracts_its_credits_and_leaves_no_join_rows() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let featured = multi_tagged("Song A", &["Rock", "Metal"], "Alice feat. Bob", &["Alice", "Bob"]);
    let doomed = insert_tagged_track(&db, "/music/t1.mp3", &featured).await?;
    let survivor = multi_tagged("Song B", &["Rock"], "Alice", &[]);
    insert_tagged_track(&db, "/music/t2.mp3", &survivor).await?;

    let mut tx = db.write().begin().await?;
    queries::scan::delete_track_by_path(&mut tx, "/music/t1.mp3").await?;
    tx.commit().await?;

    assert_eq!(artist_named(&db, "Alice").await?.track_count, 1);
    assert_eq!(genre_named(&db, "Rock").await?.track_count, 1);
    // Off the table rather than off the view: `genre_stats` hides a genre at zero, so reading it
    // through there would pass on a count the trigger never touched.
    assert_eq!(stored_genre_count(&db, "Metal").await?, 0);

    let orphans: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM track_artists WHERE track_id = ?), \
                (SELECT COUNT(*) FROM track_genres WHERE track_id = ?)",
    )
    .bind(doomed)
    .bind(doomed)
    .fetch_one(db.read())
    .await?;
    assert_eq!(orphans, (0, 0));
    Ok(())
}

/// **Two copies of the same SQL.** The migration installs these triggers and `STATS_TRIGGERS`
/// puts them back after a bulk scan, so a drift between them replaces a trigger mid-scan with a
/// version nothing else in the tree agrees with — invisible until a count comes out wrong.
#[tokio::test]
async fn the_triggers_a_bulk_scan_restores_are_the_ones_the_migration_installed()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let installed = trigger_sql(&db).await?;
    assert!(!installed.is_empty(), "the migration installs the stats triggers");

    let mut tx = db.write().begin().await?;
    queries::stats::disable_stats_triggers(&mut tx).await?;
    queries::stats::enable_stats_triggers(&mut tx).await?;
    tx.commit().await?;

    assert_eq!(trigger_sql(&db).await?, installed);
    Ok(())
}

/// Every stats trigger as `SQLite` stored it, by name.
async fn trigger_sql(db: &DbPool) -> Result<Vec<(String, String)>, AppError> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT name, sql FROM sqlite_master \
         WHERE type = 'trigger' AND name LIKE '%_stats_%' ORDER BY name",
    )
    .fetch_all(db.read())
    .await?;
    Ok(rows)
}
