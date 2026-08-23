//! What the table is easy to get wrong: which columns a re-import is allowed to touch, that a
//! station with no uuid conflicts with nothing, and the two orderings the lists are read through.

use crate::database::DbPool;
use crate::entities::radio;
use crate::error::AppError;

use super::{
    clear_play_history, delete_station, get_favorite_stations, get_recent_stations,
    get_station_by_id, mark_played, save_station, set_artwork, set_favorite, station_id_with_url,
    update_station,
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

/// The inverse of `mark_played`, and it has to undo **both** columns: the stamp is what the
/// recents list filters on, and a count left behind would show a station seven plays deep on a
/// Favorites tab sorted by plays, with no history to back it.
///
/// The star is deliberately untouched — clearing a history is the Recently Played tab's action and
/// says nothing about whether the station is a favorite.
#[tokio::test]
async fn clearing_a_history_drops_the_station_out_of_recents_and_leaves_the_star()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let station = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    set_favorite(&db, station.id, true).await?;
    mark_played(&db, station.id).await?;

    clear_play_history(&db, station.id).await?;

    let cleared = get_station_by_id(&db, station.id).await?;
    assert!(cleared.last_played.is_none());
    assert_eq!(cleared.play_count, 0);
    assert!(cleared.is_favorite, "the star belongs to the other tab");

    assert!(get_recent_stations(&db, 10).await?.is_empty());
    assert_eq!(get_favorite_stations(&db).await?.len(), 1);
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

/// Every column the insert binds, in the order the statement lists them. A bind-order slip is
/// invisible at compile time and lands a station's country in its language column — this is the
/// only place the whole projection is compared at once, so it is spelled out rather than sampled.
#[tokio::test]
async fn every_column_a_save_binds_comes_back_where_it_was_put() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let station = radio::NewRadioStation {
        station_uuid: Some("uuid-1".to_owned()),
        name: "Radio Example".to_owned(),
        stream_url: "http://example.invalid/stream".to_owned(),
        homepage: Some("http://example.invalid/".to_owned()),
        favicon_url: Some("http://example.invalid/favicon.ico".to_owned()),
        tags: "jazz,blues".to_owned(),
        country: "Germany".to_owned(),
        country_code: "DE".to_owned(),
        language: "german".to_owned(),
        codec: "MP3".to_owned(),
        bitrate: 128,
        hls: true,
    };

    let saved = save_station(&db, &station).await?;

    assert_eq!(saved.station_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(saved.name, "Radio Example");
    assert_eq!(saved.stream_url, "http://example.invalid/stream");
    assert_eq!(saved.homepage.as_deref(), Some("http://example.invalid/"));
    assert_eq!(saved.favicon_url.as_deref(), Some("http://example.invalid/favicon.ico"));
    assert_eq!(saved.tags, "jazz,blues");
    assert_eq!(saved.country, "Germany", "the country name landed in the wrong column");
    assert_eq!(saved.country_code, "DE");
    assert_eq!(saved.language, "german");
    assert_eq!(saved.codec, "MP3");
    assert_eq!(saved.bitrate, 128);
    assert!(saved.hls);
    Ok(())
}

/// The editor rewrites seven columns and re-derives an eighth. `sort_key` is the one worth
/// pinning: it is the name's shadow and the list is ordered by it, so a rename that left it
/// standing sorts the station under a name nothing on screen shows any more. The rest of the set
/// is what a moved URL replaces — a station repointed at a new mount that kept the old one's
/// homepage link or logo URL looks right and sends the user somewhere else.
#[tokio::test]
async fn editing_a_station_moves_its_sort_key_with_its_name() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let saved = save_station(&db, &custom_station("Zed FM", "http://example.invalid/zed")).await?;
    set_favorite(&db, saved.id, true).await?;
    mark_played(&db, saved.id).await?;

    let edit = radio::StationEdit {
        name: "Alpha FM".to_owned(),
        stream_url: "http://example.invalid/alpha".to_owned(),
        homepage: Some("https://alpha.invalid".to_owned()),
        favicon_url: Some("https://alpha.invalid/logo.png".to_owned()),
        tags: "jazz,lounge".to_owned(),
        codec: "AAC".to_owned(),
        bitrate: 192,
    };
    update_station(&db, saved.id, &edit).await?;

    let edited = get_station_by_id(&db, saved.id).await?;
    assert_eq!(edited.name, "Alpha FM");
    assert_eq!(edited.stream_url, "http://example.invalid/alpha");
    assert_eq!(edited.homepage.as_deref(), Some("https://alpha.invalid"));
    assert_eq!(edited.favicon_url.as_deref(), Some("https://alpha.invalid/logo.png"));
    assert_eq!(edited.tags, "jazz,lounge");
    assert_eq!(edited.codec, "AAC");
    assert_eq!(edited.bitrate, 192);
    assert_eq!(edited.sort_key, "alpha fm", "the sort key did not follow the rename");

    assert!(edited.is_favorite, "the edit un-favorited the station");
    assert_eq!(edited.play_count, 1, "the edit reset the play count");
    assert_eq!(edited.date_added, saved.date_added, "the edit re-dated the station");
    assert!(
        edited.artwork_path.is_none(),
        "the stored logo is `set_artwork`'s column — the editor must not blank it as a side effect"
    );
    Ok(())
}

/// The duplicate guard has to be a query rather than the `UNIQUE` constraint: a hand-typed station
/// carries no `station_uuid`, and `SQLite` treats NULLs as distinct, so the constraint that stops a
/// directory station arriving twice says nothing at all about this one. Without the guard,
/// re-importing a list the user has grown duplicates everything already in it, and a re-pasted URL
/// adds a second card nothing can tell apart.
///
/// The id rather than a bool is what lets the add merge onto the row it finds instead of refusing.
#[tokio::test]
async fn a_stream_url_already_kept_is_recognised_whether_or_not_it_has_a_uuid()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let url = "http://example.invalid/one";

    assert!(station_id_with_url(&db, url).await?.is_none(), "an empty table matches nothing");

    let kept = save_station(&db, &custom_station("One", url)).await?;
    assert_eq!(
        station_id_with_url(&db, url).await?,
        Some(kept.id),
        "the caller merges onto this row, so it has to be the row that holds the URL"
    );
    assert!(
        station_id_with_url(&db, "http://example.invalid/other").await?.is_none(),
        "the match is on the whole URL, not a prefix of it"
    );

    let mut browsed = directory_station("uuid-2", "Two");
    browsed.stream_url = "http://example.invalid/two".to_owned();
    save_station(&db, &browsed).await?;
    assert!(
        station_id_with_url(&db, "http://example.invalid/two").await?.is_some(),
        "a browsed station occupies its URL too — importing a file naming it must skip, not \
         add a second row nothing can tell apart"
    );
    Ok(())
}
