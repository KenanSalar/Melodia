use std::sync::Arc;

use melodia_core::entities::radio::{
    DirectoryStation, Facet, FacetKind, RadioStation, StationPage,
};
use melodia_core::error::AppError;

use super::authoring::{ensure_editable, resolve_station_name, website_url};
use super::directory::{hide_segmented, hide_segmented_codecs, names_segmented};
use super::is_listed;

/// One directory row, segmented or not. Spelled out rather than defaulted: `DirectoryStation` has
/// no `Default`, a station with no uuid and no URL being one nothing may keep.
fn station(name: &str, hls: bool) -> DirectoryStation {
    DirectoryStation {
        station_uuid: format!("uuid-{name}"),
        name: name.to_owned(),
        stream_url: format!("https://example.test/{name}"),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        state: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls,
        votes: 0,
        click_count: 0,
        last_check_ok: true,
    }
}

fn mixed_page() -> StationPage {
    StationPage {
        stations: vec![station("a", false), station("b", true), station("c", false)],
        has_more: true,
    }
}

#[test]
fn hiding_segmented_stations_drops_them_and_keeps_the_rest() {
    let mut page = mixed_page();
    hide_segmented(&mut page, true);

    let kept: Vec<&str> = page.stations.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(kept, ["a", "c"]);
}

/// The directory served and counted the rows this drops, so paging has to step over them. Rewound
/// here, a query whose answers are mostly HLS stops after its first page.
#[test]
fn hiding_segmented_stations_leaves_the_paging_flag_alone() {
    let mut page = mixed_page();
    hide_segmented(&mut page, true);
    assert!(page.has_more, "the drop is the client's, and `has_more` is the directory's answer");

    let mut ended = StationPage {
        has_more: false,
        ..mixed_page()
    };
    hide_segmented(&mut ended, true);
    assert!(!ended.has_more, "and it must not be invented either");
}

#[test]
fn leaving_them_shown_changes_nothing() {
    let mut page = mixed_page();
    hide_segmented(&mut page, false);
    assert_eq!(page, mixed_page());
}

/// A station carrying a `station_uuid` and one without, and nothing else that matters here.
fn stored(station_uuid: Option<&str>) -> RadioStation {
    RadioStation {
        id: 1,
        station_uuid: station_uuid.map(str::to_owned),
        name: "Test Station".to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        local_homepage: None,
        favicon_url: None,
        local_favicon_url: None,
        local_tags: None,
        local_country: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: true,
        sort_key: "test station".to_owned(),
        date_added: "2026-08-21T00:00:00.000+00:00".to_owned(),
        last_played: None,
        play_count: 0,
    }
}

/// The gate exists because the *revert* it prevents is silent: the edit commits, the card shows
/// the new name, and the next play of that station rewrites both fields from the directory with
/// nothing on screen naming the cause. The card only offers Edit on a custom station, so this is
/// the half that holds when a second surface asks.
#[test]
fn only_a_hand_typed_station_can_be_edited() {
    assert!(ensure_editable(&stored(None)).is_ok(), "a station with no uuid is the user's own");
    assert!(
        matches!(ensure_editable(&stored(Some("uuid-1"))), Err(AppError::Validation(_))),
        "a browsed station must be refused, and as a `Validation` — the form maps that arm onto \
         the line that says why rather than onto the generic save failure"
    );
}

/// The Format chip is built from counts the directory took before the page filter ran, so a codec
/// whose stations are all segmented offers a filter that returns an empty grid.
#[test]
fn hiding_segmented_stations_drops_the_codecs_only_they_use() {
    let facets: Arc<[Facet]> = ["MP3", "AAC+", "UNKNOWN", "OGG", "AAC,H.264", "MP4", "FLV"]
        .iter()
        .map(|name| Facet {
            code: None,
            name: (*name).to_owned(),
            station_count: 1,
        })
        .collect();

    let kept = hide_segmented_codecs(Arc::clone(&facets), FacetKind::Codecs, true);
    let names: Vec<&str> = kept.iter().map(|facet| facet.name.as_str()).collect();
    assert_eq!(names, ["MP3", "AAC+", "OGG", "FLV"]);

    assert!(
        names_segmented("UNKNOWN,H.264") && !names_segmented("FLAC"),
        "a comma means a picture track beside the audio; `FLAC` is a mount like any other"
    );
}

/// The tag list runs to tens of thousands of entries and this is called on every chip open, so
/// every other kind has to come back as the same allocation rather than a rebuilt one.
#[test]
fn no_other_facet_list_is_rebuilt() {
    let facets: Arc<[Facet]> = Arc::from(vec![Facet {
        code: None,
        name: "UNKNOWN".to_owned(),
        station_count: 1,
    }]);

    for (kind, hide) in [
        (FacetKind::Tags, true),
        (FacetKind::Countries, true),
        (FacetKind::Codecs, false),
    ] {
        let kept = hide_segmented_codecs(Arc::clone(&facets), kind, hide);
        assert!(Arc::ptr_eq(&facets, &kept), "{kind:?} with hide={hide} was rebuilt");
    }
}

/// The whole truth table behind removal, and the only place it is stated once.
///
/// A wrong arm here is silent both ways and each way loses something: read as listed, an unstarred
/// never-played row survives forever with nothing able to show it; read as unlisted, removing a
/// station from one tab deletes it out of the other — the play history a favorite's trash used to
/// take with it.
#[test]
fn a_row_survives_exactly_while_one_of_the_two_tabs_still_shows_it() {
    let mut station = stored(Some("uuid-1"));
    station.is_favorite = false;
    station.last_played = None;
    assert!(!is_listed(&station), "neither tab holds it, so the row is the user's to lose");

    station.is_favorite = true;
    assert!(is_listed(&station), "Favorites filters on the star");

    station.is_favorite = false;
    station.last_played = Some("2026-08-23T00:00:00.000+00:00".to_owned());
    assert!(is_listed(&station), "Recently Played filters on the stamp, star or no star");

    station.is_favorite = true;
    assert!(is_listed(&station), "and both at once is still one row worth keeping");
}

/// The one field a directory-owned row takes from the user, so it is the one that has to be
/// checked before it is stored.
///
/// What lands here goes behind a button that opens the browser, so a bare hostname or a `file://`
/// is refused while the user is still looking at the field. Blank is not a refusal: it is how the
/// link is removed again.
#[test]
fn a_typed_website_is_normalized_or_refused_and_blank_clears_it() {
    assert!(matches!(website_url(""), Ok(None)), "blank clears the link");
    assert!(matches!(website_url("   "), Ok(None)), "so does whitespace");

    assert_eq!(
        website_url("https://nidaa.fm").ok().flatten().as_deref(),
        Some("https://nidaa.fm/"),
        "stored through `Url`, so one site has one spelling"
    );
    assert_eq!(
        website_url("  http://example.com/live  ").ok().flatten().as_deref(),
        Some("http://example.com/live"),
        "cleartext is admitted for the reason the logo fetch admits it"
    );

    for refused in [
        "nidaa.fm",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://",
    ] {
        assert!(website_url(refused).is_err(), "{refused} must not reach a browser launch");
    }
}

/// Which fields the pencil offers, across the three states each one can be in.
///
/// The gate is the whole safety of the feature and it is silent both ways: too open and a
/// directory value the user never chose is one misclick from a typo, too closed and a field they
/// filled in themselves can never be corrected or cleared.
#[test]
fn a_field_is_the_users_only_while_the_directory_has_not_answered_for_it() {
    let mut listed = stored(Some("uuid-1"));
    assert!(listed.can_set_website(), "the directory sent no homepage, so the field is open");
    assert!(listed.can_set_genre() && listed.can_set_country() && listed.can_set_logo());

    listed.homepage = Some("https://listed.example/".to_owned());
    listed.tags = "Jazz".to_owned();
    assert!(!listed.can_set_website(), "a link the directory supplied is not theirs to overwrite");
    assert!(!listed.can_set_genre());
    assert!(listed.can_set_country(), "and the fields it still says nothing about stay open");
    assert!(listed.is_editable(), "so the pencil is still worth drawing");

    // Overridden, the field reopens: a typo has to be correctable and the dialog promises a blank
    // field removes the value again.
    listed.local_homepage = Some("https://mine.example/".to_owned());
    assert!(listed.can_set_website());
    assert_eq!(listed.website(), Some("https://mine.example/"), "and the override is what reads");

    // Every field answered by the directory, none overridden: nothing left to offer.
    let mut complete = stored(Some("uuid-1"));
    complete.homepage = Some("https://listed.example/".to_owned());
    complete.favicon_url = Some("https://listed.example/logo.png".to_owned());
    complete.tags = "Jazz".to_owned();
    complete.country = "Tunisia".to_owned();
    assert!(!complete.is_editable(), "the pencil goes away rather than offering a misclick");

    // A hand-typed station has no directory behind it to disagree with, so it is always the
    // user's whole to edit however much it already holds.
    let mut typed = stored(None);
    typed.homepage = Some("https://probed.example/".to_owned());
    typed.tags = "Talk".to_owned();
    typed.country = "Tunisia".to_owned();
    assert!(typed.is_editable() && typed.can_set_website() && typed.can_set_genre());
}

/// A hand-typed station is listed by its star alone, and the stamp does not stand in for one.
///
/// The card refuses a star to a row no directory names, so an unstarred one is stranded rather
/// than merely unstarred: Recently Played goes on showing it and nothing anywhere can put it back
/// in Favorites. Removing it there has to be the delete it was before the two tabs split.
#[test]
fn a_hand_typed_station_is_listed_by_its_star_and_never_by_its_plays() {
    let mut station = stored(None);
    station.last_played = Some("2026-08-23T00:00:00.000+00:00".to_owned());

    assert!(is_listed(&station), "the star still lists it");

    station.is_favorite = false;
    assert!(
        !is_listed(&station),
        "no page can name it and the card offers no star, so the plays list nothing"
    );

    // The directory's own rows keep the play history that removal takes from a hand-typed one.
    let mut from_directory = stored(Some("uuid-1"));
    from_directory.is_favorite = false;
    from_directory.last_played = Some("2026-08-23T00:00:00.000+00:00".to_owned());
    assert!(is_listed(&from_directory), "Browse can restore this one, so the plays still count");
}

/// Three ways a station ends up named, in the order they win. The host fallback is what stops a
/// row being titled with a whole stream URL, which is unreadable in a card and sorts under
/// `https`; the middle arm is the only reason a blank name field is worth offering at all.
#[test]
fn a_station_takes_the_best_name_on_offer() {
    let url = "https://stream.example.test:8000/live?token=abc";

    assert_eq!(resolve_station_name("  My Station  ", Some("Server Name"), url), "My Station");
    assert_eq!(resolve_station_name("", Some("  Server Name "), url), "Server Name");
    assert_eq!(resolve_station_name("   ", None, url), "stream.example.test");
    assert_eq!(
        resolve_station_name("", Some("   "), url),
        "stream.example.test",
        "a server that sends a blank name has not named itself"
    );
    assert_eq!(
        resolve_station_name("", None, "not a url"),
        "not a url",
        "an unparseable URL has no host to fall back to, and losing the text entirely would \
         leave the row with no name at all"
    );
}
