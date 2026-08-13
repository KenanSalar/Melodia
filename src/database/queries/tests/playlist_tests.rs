use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::tests::helpers::*;
use crate::error::AppError;

#[tokio::test]
async fn create_playlist_returns_correct_fields() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "My Playlist", Some("A description")).await?;
    assert_eq!(pl.name, "My Playlist");
    assert_eq!(pl.description.as_deref(), Some("A description"));
    assert!(pl.id > 0);
    Ok(())
}

#[tokio::test]
async fn get_all_playlists_empty() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let playlists = queries::playlist::get_all_playlists(&db).await?;
    assert!(playlists.is_empty());
    Ok(())
}

#[tokio::test]
async fn get_all_playlists_ordered_by_updated_at() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let _pl1 = queries::playlist::create_playlist(&db, "First", None).await?;
    let _pl2 = queries::playlist::create_playlist(&db, "Second", None).await?;

    let playlists = queries::playlist::get_all_playlists(&db).await?;
    assert_eq!(playlists.len(), 2);
    // Most recently updated first
    assert!(playlists[0].updated_at >= playlists[1].updated_at);
    Ok(())
}

#[tokio::test]
async fn get_playlist_by_id_happy_path() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let found = queries::playlist::get_playlist_by_id(&db, pl.id).await?;
    assert_eq!(found.name, "Test");
    Ok(())
}

#[tokio::test]
async fn get_playlist_by_id_not_found() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let result = queries::playlist::get_playlist_by_id(&db, 99999).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn update_playlist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Original", None).await?;
    let updated =
        queries::playlist::update_playlist(&db, pl.id, "Renamed", Some("New desc"), false).await?;
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.description.as_deref(), Some("New desc"));
    Ok(())
}

#[tokio::test]
async fn delete_playlist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "ToDelete", None).await?;
    queries::playlist::delete_playlist(&db, pl.id).await?;
    let result = queries::playlist::get_playlist_by_id(&db, pl.id).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn add_and_get_playlist_tracks() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;

    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let tracks = queries::playlist::get_playlist_tracks(&db, pl.id).await?;
    assert_eq!(tracks.len(), 3);
    Ok(())
}

#[tokio::test]
async fn add_tracks_empty_is_noop() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &[]).await?;
    let tracks = queries::playlist::get_playlist_tracks(&db, pl.id).await?;
    assert!(tracks.is_empty());
    Ok(())
}

#[tokio::test]
async fn remove_track_from_playlist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    queries::playlist::remove_track_from_playlist(&db, pl.id, track_ids[0]).await?;

    let tracks = queries::playlist::get_playlist_tracks(&db, pl.id).await?;
    assert_eq!(tracks.len(), 2);
    assert!(!tracks.iter().any(|t| t.id == track_ids[0]));
    Ok(())
}

#[tokio::test]
async fn remove_tracks_batch() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    queries::playlist::remove_tracks_from_playlist_batch(&db, pl.id, &track_ids[0..2]).await?;

    let tracks = queries::playlist::get_playlist_tracks(&db, pl.id).await?;
    assert_eq!(tracks.len(), 1);
    Ok(())
}

#[tokio::test]
async fn remove_tracks_batch_empty_is_noop() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    queries::playlist::remove_tracks_from_playlist_batch(&db, pl.id, &[]).await?;
    Ok(())
}

#[tokio::test]
async fn reorder_playlist_track() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    // Move first track to last position
    queries::playlist::reorder_playlist_track(&db, pl.id, 0, 2).await?;

    let tracks = queries::playlist::get_playlist_tracks(&db, pl.id).await?;
    assert_eq!(tracks.len(), 3);
    // First track should now be at the end
    assert_eq!(tracks[2].id, track_ids[0]);
    Ok(())
}

#[tokio::test]
async fn reorder_playlist_track_invalid_index() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let result = queries::playlist::reorder_playlist_track(&db, pl.id, 0, 99).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn playlist_stats_track_count() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let stats = queries::playlist::get_playlist_by_id(&db, pl.id).await?;
    assert_eq!(stats.track_count, 3);
    assert!(stats.total_duration_ms > 0);
    Ok(())
}

/// Regression: clearing a playlist's thumbnail must persist across subsequent
/// add-track operations. Previously `update_playlist` with `clear_thumbnail`
/// set `custom_thumbnail = FALSE`, which allowed `add_tracks_to_playlist` to
/// auto-repopulate `thumbnail_path` from the first track's artwork.
#[tokio::test]
async fn clearing_thumbnail_persists_after_adding_tracks() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    // Give every seeded track an artwork path so auto-regeneration has
    // something to latch onto.
    sqlx::query("UPDATE tracks SET artwork_path = '/artwork/cover.jpg'")
        .execute(db.write())
        .await?;

    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();
    assert!(track_ids.len() >= 2, "test expects seeded_db to have >= 2 tracks");

    // Create playlist and add the first track — this auto-populates
    // thumbnail_path from the track's artwork via the WHERE custom_thumbnail=FALSE branch.
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids[..1]).await?;

    let after_first_add = queries::playlist::get_playlist_by_id(&db, pl.id).await?;
    assert_eq!(after_first_add.thumbnail_path.as_deref(), Some("/artwork/cover.jpg"));
    assert!(!after_first_add.custom_thumbnail);

    // User clears the thumbnail via the edit dialog.
    let cleared = queries::playlist::update_playlist(&db, pl.id, "Test", None, true).await?;
    assert!(cleared.thumbnail_path.is_none());
    assert!(
        cleared.custom_thumbnail,
        "custom_thumbnail must be TRUE after clearing so auto-regen is skipped"
    );

    // Add another track — must NOT bring the artwork back.
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids[1..2]).await?;

    let after_second_add = queries::playlist::get_playlist_by_id(&db, pl.id).await?;
    assert!(
        after_second_add.thumbnail_path.is_none(),
        "thumbnail_path must remain NULL after adding a track to a cleared playlist"
    );
    assert!(after_second_add.custom_thumbnail);
    Ok(())
}
