use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::fixtures::*;
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
