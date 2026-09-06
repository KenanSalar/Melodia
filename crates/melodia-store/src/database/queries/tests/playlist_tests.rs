use crate::database::queries;
#[allow(clippy::wildcard_imports)]
use crate::database::queries::fixtures::*;
use melodia_core::error::AppError;

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

/// One id through the batch path, which is the only remover there is — the singular
/// `remove_track_from_playlist` went with its last caller.
#[tokio::test]
async fn remove_one_track_leaves_the_rest_renumbered() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let pl = queries::playlist::create_playlist(&db, "Test", None).await?;
    let all_tracks = queries::track::get_all_tracks(&db).await?;
    let track_ids: Vec<i64> = all_tracks.iter().map(|t| t.id).collect();

    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    queries::playlist::remove_tracks_from_playlist_batch(&db, pl.id, &track_ids[0..1]).await?;

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

/// Stage artwork on one track, which the scan helper leaves null.
async fn set_artwork(db: &crate::database::DbPool, id: i64, path: &str) -> Result<(), AppError> {
    sqlx::query("UPDATE tracks SET artwork_path = ? WHERE id = ?")
        .bind(path)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// The positions a playlist's rows actually hold, in id order.
async fn positions(db: &crate::database::DbPool, playlist_id: i64) -> Result<Vec<i64>, AppError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT position FROM playlist_items WHERE playlist_id = ? ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(db.read())
    .await?;
    Ok(rows.into_iter().map(|(p,)| p).collect())
}

/// **Nothing in this tree reads `playlist_items.position` directly**, so the renumber that runs
/// after a removal was pinned only by the *order* rows come back in — which a gapped sequence
/// satisfies exactly as well. A gap is invisible until an insert lands on a position two rows
/// already share.
#[tokio::test]
async fn removing_a_middle_track_closes_the_gap_it_left() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    let pl = queries::playlist::create_playlist(&db, "Ordered", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;
    assert_eq!(positions(&db, pl.id).await?, [0, 1, 2]);

    queries::playlist::remove_tracks_from_playlist_batch(&db, pl.id, &track_ids[1..2]).await?;

    assert_eq!(positions(&db, pl.id).await?, [0, 1], "the survivors must renumber from zero");
    Ok(())
}

/// The mosaic's candidate list: distinct covers, in the order the playlist first reaches them.
/// Reading it off `position` rather than off `MIN(position)` would order a deduplicated group by
/// whichever row the grouping happened to keep, so a cover shared by an early and a late track
/// could sort last.
#[tokio::test]
async fn the_mosaic_candidates_are_distinct_and_in_playlist_order() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    // The first and last track share a cover; the middle one has its own.
    set_artwork(&db, track_ids[0], "/artwork/shared.jpg").await?;
    set_artwork(&db, track_ids[1], "/artwork/middle.jpg").await?;
    set_artwork(&db, track_ids[2], "/artwork/shared.jpg").await?;
    let pl = queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let candidates = queries::playlist::get_playlist_artwork_paths(&db, pl.id, 4).await?;

    assert_eq!(candidates, ["/artwork/shared.jpg", "/artwork/middle.jpg"]);
    Ok(())
}

/// A track with no cover contributes nothing, and the blank string is the second spelling of
/// that — the column carries both. Without the `!= ''` half an empty path takes a mosaic tile
/// and paints a hole.
#[tokio::test]
async fn a_track_with_no_cover_offers_no_mosaic_candidate() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    set_artwork(&db, track_ids[0], "").await?;
    set_artwork(&db, track_ids[1], "/artwork/real.jpg").await?;
    let pl = queries::playlist::create_playlist(&db, "Sparse", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let candidates = queries::playlist::get_playlist_artwork_paths(&db, pl.id, 4).await?;

    assert_eq!(candidates, ["/artwork/real.jpg"], "the blank and the null both contribute nothing");
    Ok(())
}

/// The cap the mosaic asks with, which is what stops a thousand-track playlist reading a
/// thousand rows to draw four tiles.
#[tokio::test]
async fn the_mosaic_candidate_list_stops_at_the_limit_it_was_given() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    for (index, id) in track_ids.iter().enumerate() {
        set_artwork(&db, *id, &format!("/artwork/{index}.jpg")).await?;
    }
    let pl = queries::playlist::create_playlist(&db, "Capped", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids).await?;

    let candidates = queries::playlist::get_playlist_artwork_paths(&db, pl.id, 2).await?;

    assert_eq!(candidates.len(), 2);
    Ok(())
}

/// What the "add to playlist" dialog puts beside each row: how many of the selected tracks that
/// playlist already holds. A count keyed on the wrong column, or summed across the wrong group,
/// tells the user a playlist is full of their selection when it holds none of it.
#[tokio::test]
async fn the_selection_count_is_per_playlist() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    let holds_all = queries::playlist::create_playlist(&db, "All", None).await?;
    let holds_one = queries::playlist::create_playlist(&db, "One", None).await?;
    let holds_none = queries::playlist::create_playlist(&db, "None", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, holds_all.id, &track_ids).await?;
    queries::playlist::add_tracks_to_playlist(&db, holds_one.id, &track_ids[..1]).await?;

    let counts =
        queries::playlist::count_tracks_in_playlists_for_selection(&db, &track_ids).await?;

    assert_eq!(counts.get(&holds_all.id), Some(&3));
    assert_eq!(counts.get(&holds_one.id), Some(&1));
    assert_eq!(counts.get(&holds_none.id), None, "a playlist holding none of them is absent");
    Ok(())
}

/// A smart playlist is a row with criteria and **no** `playlist_items`, so the stats view reports
/// it as empty however many tracks it resolves to. Worth pinning because the number is real
/// everywhere else on that struct, and a reader reaching for it here gets a plausible zero.
#[tokio::test]
async fn a_smart_playlist_keeps_its_criteria_and_holds_no_items() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let created =
        queries::playlist::create_smart_playlist(&db, "Top Rated", None, r#"{"v":1}"#).await?;
    assert!(created.is_smart);
    assert_eq!(created.smart_criteria.as_deref(), Some(r#"{"v":1}"#));

    let updated = queries::playlist::update_smart_criteria(&db, created.id, r#"{"v":2}"#).await?;
    assert_eq!(updated.smart_criteria.as_deref(), Some(r#"{"v":2}"#));

    let stats = queries::playlist::get_playlist_by_id(&db, created.id).await?;
    assert_eq!(stats.track_count, 0, "membership is resolved live, never from the junction table");
    Ok(())
}

/// Rewriting the criteria of a playlist that is not there is an error rather than a silent
/// no-op: the dialog would otherwise report a save that wrote nothing.
#[tokio::test]
async fn rewriting_the_criteria_of_a_missing_playlist_is_reported() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;

    let missing = queries::playlist::update_smart_criteria(&db, 9_999, r#"{"v":1}"#).await;

    assert!(matches!(missing, Err(AppError::NotFound(_))), "{missing:?}");
    Ok(())
}

/// The other renumber, and the one nothing reached: an add is `INSERT OR IGNORE` against
/// positions computed from the old maximum, so re-adding a track the playlist already holds
/// burns the position it was given and leaves a hole behind the duplicate it skipped.
#[tokio::test]
async fn re_adding_a_track_leaves_no_hole_where_it_was_skipped() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_ids: Vec<i64> =
        queries::track::get_all_tracks(&db).await?.into_iter().map(|t| t.id).collect();
    let pl = queries::playlist::create_playlist(&db, "Re-added", None).await?;
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids[..1]).await?;

    // The first id is already seated at 0, so its slot in this batch is ignored and the second
    // id lands at 2.
    queries::playlist::add_tracks_to_playlist(&db, pl.id, &track_ids[..2]).await?;

    assert_eq!(positions(&db, pl.id).await?, [0, 1]);
    Ok(())
}
