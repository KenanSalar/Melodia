//! What the table is easy to get wrong: which columns a re-import is allowed to touch, that a
//! station with no uuid conflicts with nothing, and the two orderings the lists are read through.

use crate::database::DbPool;
use crate::entities::radio;
use crate::error::AppError;

use super::{
    delete_station, get_favorite_stations, get_recent_stations, get_station_by_id, mark_played,
    save_station, set_artwork, set_favorite,
};

fn directory_station(uuid: &str, name: &str) -> radio::NewRadioStation {
    radio::NewRadioStation {
        station_uuid: Some(uuid.to_owned()),
        name: name.to_owned(),
        stream_url: format!("http://example.invalid/{uuid}"),
        ..Default::default()
    }
}

fn custom_station(name: &str, stream_url: &str) -> radio::NewRadioStation {
    radio::NewRadioStation {
        name: name.to_owned(),
        stream_url: stream_url.to_owned(),
        ..Default::default()
    }
}

async fn station_count(db: &DbPool) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM radio_stations")
        .fetch_one(db.read())
        .await?)
}

/// The whole reason the conflict list is spelled out rather than blanket: a directory refresh
/// re-sends every station the user already keeps, and the row carries what they did with it.
#[tokio::test]
async fn re_importing_a_station_updates_it_without_touching_the_user_side() -> Result<(), AppError>
{
    let db = DbPool::test_pool().await?;
    let logo = "/data/artwork/33fb807d1f1b7cbb.jpg";

    let first = save_station(&db, &directory_station("uuid-1", "Old Name")).await?;
    set_favorite(&db, first.id, true).await?;
    mark_played(&db, first.id).await?;
    set_artwork(&db, first.id, Some(logo)).await?;

    let mut refreshed = directory_station("uuid-1", "New Name");
    refreshed.stream_url = "http://example.invalid/moved".to_owned();
    refreshed.bitrate = 320;
    let second = save_station(&db, &refreshed).await?;

    assert_eq!(station_count(&db).await?, 1, "the re-import inserted a second row");
    assert_eq!(second.id, first.id);
    assert_eq!(second.name, "New Name");
    assert_eq!(second.stream_url, "http://example.invalid/moved");
    assert_eq!(second.bitrate, 320);
    assert_eq!(second.sort_key, "new name", "the sort key did not follow the rename");

    assert!(second.is_favorite, "the re-import un-favorited the station");
    assert_eq!(second.play_count, 1, "the re-import reset the play count");
    assert!(second.last_played.is_some(), "the re-import cleared the last-played stamp");
    assert_eq!(second.artwork_path.as_deref(), Some(logo), "the re-import blanked the logo");
    assert_eq!(second.date_added, first.date_added, "the re-import re-dated the station");
    Ok(())
}

/// `SQLite` treats NULLs as distinct under UNIQUE, which is what lets a user keep any number of
/// hand-typed stations and why the save drops its conflict clause for one.
#[tokio::test]
async fn hand_typed_stations_coexist_with_no_uuid() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let first = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    let second = save_station(&db, &custom_station("B", "http://b.invalid/")).await?;

    assert_ne!(first.id, second.id);
    assert_eq!(station_count(&db).await?, 2);
    assert!(first.station_uuid.is_none());
    assert!(second.station_uuid.is_none());
    Ok(())
}

/// What the stored `sort_key` buys: a plain `ORDER BY name` puts "Radio 10" first.
#[tokio::test]
async fn favorites_come_back_in_natural_name_order() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    for name in ["Radio 10", "Radio 2", "Radio 1"] {
        let station = save_station(&db, &custom_station(name, "http://x.invalid/")).await?;
        set_favorite(&db, station.id, true).await?;
    }
    // Favorited by nobody, so it must not appear.
    save_station(&db, &custom_station("Radio 3", "http://y.invalid/")).await?;

    let names: Vec<String> =
        get_favorite_stations(&db).await?.into_iter().map(|s| s.name).collect();

    assert_eq!(names, ["Radio 1", "Radio 2", "Radio 10"]);
    Ok(())
}

#[tokio::test]
async fn marking_a_play_counts_it_and_stamps_the_time() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let station = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    assert_eq!(station.play_count, 0);
    assert!(station.last_played.is_none());

    mark_played(&db, station.id).await?;
    mark_played(&db, station.id).await?;

    let played = get_station_by_id(&db, station.id).await?;
    assert_eq!(played.play_count, 2);
    assert!(played.last_played.is_some());
    Ok(())
}

/// Stamped by hand rather than through `mark_played`, so the ordering is asserted against known
/// values instead of against how fast two calls to the clock land.
#[tokio::test]
async fn recents_come_back_newest_first_and_skip_the_unplayed() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let older = save_station(&db, &custom_station("Older", "http://a.invalid/")).await?;
    let newer = save_station(&db, &custom_station("Newer", "http://b.invalid/")).await?;
    save_station(&db, &custom_station("Never", "http://c.invalid/")).await?;

    for (id, stamp) in [
        (older.id, "2026-08-01T00:00:00+00:00"),
        (newer.id, "2026-08-02T00:00:00+00:00"),
    ] {
        sqlx::query("UPDATE radio_stations SET last_played = ? WHERE id = ?")
            .bind(stamp)
            .bind(id)
            .execute(db.write())
            .await?;
    }

    let names: Vec<String> =
        get_recent_stations(&db, 10).await?.into_iter().map(|s| s.name).collect();

    assert_eq!(names, ["Newer", "Older"]);
    Ok(())
}

#[tokio::test]
async fn a_station_that_was_deleted_between_render_and_click_is_not_found() -> Result<(), AppError>
{
    let db = DbPool::test_pool().await?;

    let station = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    delete_station(&db, station.id).await?;

    let Err(err) = get_station_by_id(&db, station.id).await else {
        return Err(AppError::Validation("expected a deleted station to be NotFound".into()));
    };
    assert!(matches!(err, AppError::NotFound(_)), "got: {err}");
    Ok(())
}
