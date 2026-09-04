//! What the table is easy to get wrong: which columns a re-import is allowed to touch, that a
//! station with no uuid conflicts with nothing, and the two orderings the lists are read through.

use crate::database::DbPool;
use melodia_core::entities::radio;
use melodia_core::error::AppError;

use super::{
    clear_play_history, delete_station, get_favorite_stations, get_recent_stations,
    get_station_by_id, logo_answers, logo_miss_attempts, mark_played, prune_logo_answers,
    record_logo_hit, record_logo_miss, save_station, set_artwork, set_favorite, set_local_fields,
    station_id_with_url, update_station,
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

/// The two homepage columns have one writer each, which is the whole reason there are two.
///
/// Roughly one directory entry in fifteen carries no homepage, and nothing can be derived from a
/// stream URL that is usually a shared host, so the field has to be fillable by hand. Folded into
/// one column it is a choice between the re-import blanking what the user typed and the directory
/// never being able to correct a site that moved; kept apart, both hold at once.
#[tokio::test]
async fn a_re_import_rewrites_the_directory_column_and_never_the_user_s() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = save_station(&db, &directory_station("uuid-1", "Nidaa")).await?;
    let listed_first = get_station_by_id(&db, id).await?;
    assert!(
        listed_first.homepage.is_none(),
        "the directory sent none, which is the case under test"
    );

    let mine = radio::StationOverrides {
        website: Some("https://nidaa.fm/".to_owned()),
        genre: Some("Talk".to_owned()),
        ..Default::default()
    };
    set_local_fields(&db, id, &mine).await?;

    // A play or a star re-sends the same directory row, still carrying none of this.
    save_station(&db, &directory_station("uuid-1", "Nidaa")).await?;
    let kept = get_station_by_id(&db, id).await?;
    assert_eq!(
        (kept.local_homepage.as_deref(), kept.local_tags.as_deref()),
        (Some("https://nidaa.fm/"), Some("Talk")),
        "the re-import blanked what the user typed"
    );
    assert_eq!(kept.website(), Some("https://nidaa.fm/"));
    assert_eq!(kept.genre(), Some("Talk"));
    assert!(kept.can_set_website(), "their own answer stays theirs to correct");

    // The directory catching up writes its own columns, and does not touch theirs.
    let mut listed = directory_station("uuid-1", "Nidaa");
    listed.homepage = Some("https://www.nidaa.fm/".to_owned());
    listed.tags = "News".to_owned();
    save_station(&db, &listed).await?;
    let updated = get_station_by_id(&db, id).await?;
    assert_eq!(updated.homepage.as_deref(), Some("https://www.nidaa.fm/"));
    assert_eq!(updated.local_homepage.as_deref(), Some("https://nidaa.fm/"));
    assert_eq!(updated.website(), Some("https://nidaa.fm/"), "the user's answer wins the read");
    assert_eq!(updated.genre(), Some("Talk"));

    // Cleared, the directory's own values are what the card falls back to — and with nothing of
    // the user's left over them, those fields close rather than offering them up for editing.
    set_local_fields(&db, id, &radio::StationOverrides::default()).await?;
    let cleared = get_station_by_id(&db, id).await?;
    assert_eq!(cleared.website(), Some("https://www.nidaa.fm/"));
    assert_eq!(cleared.genre(), Some("News"));
    assert!(!cleared.can_set_website(), "a directory link is not the user's to overwrite");
    assert!(!cleared.can_set_genre(), "nor is a genre it supplied");
    assert!(cleared.can_set_country(), "but a field it still says nothing about stays open");
    Ok(())
}

/// The whole reason the conflict list is spelled out rather than blanket: a directory refresh
/// re-sends every station the user already keeps, and the row carries what they did with it.
#[tokio::test]
async fn re_importing_a_station_updates_it_without_touching_the_user_side() -> Result<(), AppError>
{
    let db = DbPool::test_pool().await?;
    let logo = "/data/artwork/33fb807d1f1b7cbb.jpg";

    let first_id = save_station(&db, &directory_station("uuid-1", "Old Name")).await?;
    set_favorite(&db, first_id, true).await?;
    mark_played(&db, first_id).await?;
    set_artwork(&db, first_id, Some(logo)).await?;
    let first = get_station_by_id(&db, first_id).await?;

    let mut refreshed = directory_station("uuid-1", "New Name");
    refreshed.stream_url = "http://example.invalid/moved".to_owned();
    refreshed.bitrate = 320;
    let second_id = save_station(&db, &refreshed).await?;
    let second = get_station_by_id(&db, second_id).await?;

    assert_eq!(station_count(&db).await?, 1, "the re-import inserted a second row");
    assert_eq!(second_id, first_id);
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

    let first_id = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    let second_id = save_station(&db, &custom_station("B", "http://b.invalid/")).await?;

    assert_ne!(first_id, second_id);
    assert_eq!(station_count(&db).await?, 2);
    assert!(get_station_by_id(&db, first_id).await?.station_uuid.is_none());
    assert!(get_station_by_id(&db, second_id).await?.station_uuid.is_none());
    Ok(())
}

/// What the stored `sort_key` buys: a plain `ORDER BY name` puts "Radio 10" first.
#[tokio::test]
async fn favorites_come_back_in_natural_name_order() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    for name in ["Radio 10", "Radio 2", "Radio 1"] {
        let id = save_station(&db, &custom_station(name, "http://x.invalid/")).await?;
        set_favorite(&db, id, true).await?;
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

    let id = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    let fresh = get_station_by_id(&db, id).await?;
    assert_eq!(fresh.play_count, 0);
    assert!(fresh.last_played.is_none());

    mark_played(&db, id).await?;
    mark_played(&db, id).await?;

    let played = get_station_by_id(&db, id).await?;
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

    let id = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    set_favorite(&db, id, true).await?;
    mark_played(&db, id).await?;

    clear_play_history(&db, id).await?;

    let cleared = get_station_by_id(&db, id).await?;
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
        (older, "2026-08-01T00:00:00+00:00"),
        (newer, "2026-08-02T00:00:00+00:00"),
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

    let id = save_station(&db, &custom_station("A", "http://a.invalid/")).await?;
    delete_station(&db, id).await?;

    let Err(err) = get_station_by_id(&db, id).await else {
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

    let saved = get_station_by_id(&db, save_station(&db, &station).await?).await?;

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
    let id = save_station(&db, &custom_station("Zed FM", "http://example.invalid/zed")).await?;
    set_favorite(&db, id, true).await?;
    mark_played(&db, id).await?;
    let saved = get_station_by_id(&db, id).await?;

    let edit = radio::StationEdit {
        name: "Alpha FM".to_owned(),
        stream_url: "http://example.invalid/alpha".to_owned(),
        homepage: Some("https://alpha.invalid".to_owned()),
        favicon_url: Some("https://alpha.invalid/logo.png".to_owned()),
        tags: "jazz,lounge".to_owned(),
        codec: "AAC".to_owned(),
        bitrate: 192,
    };
    update_station(&db, id, &edit).await?;

    let edited = get_station_by_id(&db, id).await?;
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
        Some(kept),
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

/// The uuid outranks the URL, which is the whole reason the statement carries an `ORDER BY`
/// rather than two `WHERE`s the caller picks between.
///
/// A directory row repointed at a new mount is the case: its uuid still names it, and the URL it
/// used to hold can meanwhile belong to a station the user typed in by hand. Answered by URL that
/// entry lands on the wrong row and the re-import rewrites a station nobody asked it to touch.
/// The two ranks used to be two calls in a fixed order, where the order was on the page; folded
/// into one statement it is a sort key, which is a thing a later edit can quietly simplify away.
#[tokio::test]
async fn a_repointed_directory_row_is_found_by_its_uuid_and_not_by_its_old_url()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let moved = "http://example.invalid/moved";

    let listed = save_station(&db, &directory_station("uuid-1", "Listed")).await?;
    set_favorite(&db, listed, true).await?;
    let hand_typed = save_station(&db, &custom_station("Hand Typed", moved)).await?;

    assert_eq!(
        super::kept_station_matching(db.read(), Some("uuid-1"), moved).await?,
        Some((listed, true)),
        "both keys match, and a different row each — the uuid is the directory's own identity \
         for a station, so it is the one that answers"
    );
    assert_eq!(
        super::kept_station_matching(db.read(), None, moved).await?,
        Some((hand_typed, false)),
        "an entry carrying no uuid still has to find its row, that being every hand-typed one"
    );
    assert_eq!(
        super::kept_station_matching(db.read(), Some("uuid-1"), "http://example.invalid/new")
            .await?,
        Some((listed, true)),
        "the uuid alone is enough — a mount the row has never held is what a repoint looks like"
    );
    assert_eq!(
        super::kept_station_matching(db.read(), Some("uuid-9"), "http://example.invalid/new")
            .await?,
        None,
        "neither key matches, so the entry is new and the import inserts it"
    );
    Ok(())
}

/// Timestamps the retention rules compare, spelled rather than derived: the pass is a string
/// comparison against the clock, so a test that built its dates the same way the code does would
/// only be reading the format back to itself.
const OLD: &str = "2026-01-01T00:00:00.000+00:00";
const RECENT: &str = "2026-08-20T00:00:00.000+00:00";
const NEWER: &str = "2026-08-21T00:00:00.000+00:00";

/// One row per URL, holding whichever answer that URL last gave. The hit half is what makes the
/// store a cache: without a path on the row nothing can know where a URL's bytes landed, the file
/// being named by a hash of its own content, so every browsed logo was re-fetched every launch.
#[tokio::test]
async fn a_url_comes_back_with_whichever_answer_it_last_gave() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let (hit, miss) = ("https://a.invalid/logo.png", "https://b.invalid/logo.png");

    record_logo_hit(&db, hit, "/store/a.png", 4_096, RECENT).await?;
    record_logo_miss(&db, miss, 2, NEWER, RECENT).await?;

    let answers = logo_answers(&db, &[hit.to_owned(), miss.to_owned()]).await?;
    assert_eq!(answers.len(), 2);

    let found = |url: &str| answers.iter().find(|a| a.favicon_url == url).cloned();
    let Some(hit) = found(hit) else {
        return Err(AppError::Validation("the hit did not come back".into()));
    };
    assert_eq!(hit.artwork_path.as_deref(), Some("/store/a.png"));
    assert!(hit.retry_after.is_none(), "a hit suppresses nothing");

    let Some(miss) = found(miss) else {
        return Err(AppError::Validation("the miss did not come back".into()));
    };
    assert!(miss.artwork_path.is_none());
    assert_eq!(miss.retry_after.as_deref(), Some(NEWER));
    Ok(())
}

/// A host that starts answering again must not stay suppressed by a schedule it earned while it
/// was down — and the escalating backoff means that schedule can be a week out.
#[tokio::test]
async fn a_hit_clears_the_backoff_the_same_url_earned_while_it_was_down() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let url = "https://a.invalid/logo.png";

    record_logo_miss(&db, url, 5, NEWER, RECENT).await?;
    assert_eq!(logo_miss_attempts(&db, url).await?, Some(5));

    record_logo_hit(&db, url, "/store/a.png", 4_096, NEWER).await?;

    let answers = logo_answers(&db, &[url.to_owned()]).await?;
    let Some(answer) = answers.first() else {
        return Err(AppError::Validation("the row went missing".into()));
    };
    assert_eq!(answer.artwork_path.as_deref(), Some("/store/a.png"));
    assert!(answer.retry_after.is_none(), "the backoff outlived the recovery");
    assert_eq!(logo_miss_attempts(&db, url).await?, Some(0));
    Ok(())
}

/// The bound that actually holds. A TTL alone lets the store run up as far as the user's own
/// browsing rate takes it, so what has to be true is that the newest hits survive and the tail
/// goes — the row crossing the line being the first dropped, since the running total includes it.
#[tokio::test]
async fn the_byte_cap_keeps_the_newest_hits_and_drops_the_tail() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    for (url, stamp) in [
        ("a", NEWER),
        ("b", RECENT),
        ("c", "2026-08-19T00:00:00.000+00:00"),
    ] {
        record_logo_hit(&db, url, &format!("/store/{url}.png"), 400, stamp).await?;
    }

    // Two rows' worth: the third crosses it and is the first to go.
    assert_eq!(prune_logo_answers(&db, OLD, OLD, 800).await?, 1);

    let kept: Vec<String> = logo_answers(&db, &["a".to_owned(), "b".to_owned(), "c".to_owned()])
        .await?
        .into_iter()
        .map(|answer| answer.favicon_url)
        .collect();
    assert_eq!(kept.len(), 2, "got {kept:?}");
    assert!(kept.contains(&"a".to_owned()) && kept.contains(&"b".to_owned()), "got {kept:?}");
    Ok(())
}

/// The staleness half, and it reads a different column per kind: a miss is done once its retry
/// time has passed, a hit once it has gone unasked-for long enough to be worth one request again.
/// A cutoff that caught the wrong kind would either re-ask a dead host every launch or evict a
/// warm cache on every pass.
#[tokio::test]
async fn each_kind_of_answer_ages_out_on_its_own_clock() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    record_logo_hit(&db, "fresh-hit", "/store/a.png", 400, NEWER).await?;
    record_logo_hit(&db, "stale-hit", "/store/b.png", 400, OLD).await?;
    record_logo_miss(&db, "live-miss", 1, NEWER, NEWER).await?;
    record_logo_miss(&db, "spent-miss", 1, OLD, OLD).await?;

    // A cutoff between the two stamps, and a cap far above what these rows occupy.
    assert_eq!(prune_logo_answers(&db, RECENT, RECENT, 1 << 20).await?, 2);

    let urls = ["fresh-hit", "stale-hit", "live-miss", "spent-miss"].map(str::to_owned);
    let kept: Vec<String> =
        logo_answers(&db, &urls).await?.into_iter().map(|a| a.favicon_url).collect();
    assert_eq!(kept.len(), 2, "got {kept:?}");
    assert!(kept.contains(&"fresh-hit".to_owned()), "a warm cache entry was evicted");
    assert!(kept.contains(&"live-miss".to_owned()), "a backoff still in force was forgotten");
    Ok(())
}
