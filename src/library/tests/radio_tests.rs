use crate::entities::radio::{DirectoryStation, RadioStation, StationPage};
use crate::error::AppError;

use super::{
    ensure_editable, ensure_playable, hide_hls, is_listed, resolve_station_name, website_url,
};

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
    hide_hls(&mut page, true);

    let kept: Vec<&str> = page.stations.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(kept, ["a", "c"]);
}

/// The directory served and counted the rows this drops, so paging has to step over them. Rewound
/// here, a query whose answers are mostly HLS stops after its first page.
#[test]
fn hiding_segmented_stations_leaves_the_paging_flag_alone() {
    let mut page = mixed_page();
    hide_hls(&mut page, true);
    assert!(page.has_more, "the drop is the client's, and `has_more` is the directory's answer");

    let mut ended = StationPage {
        has_more: false,
        ..mixed_page()
    };
    hide_hls(&mut ended, true);
    assert!(!ended.has_more, "and it must not be invented either");
}

#[test]
fn leaving_them_shown_changes_nothing() {
    let mut page = mixed_page();
    hide_hls(&mut page, false);
    assert_eq!(page, mixed_page());
}

/// The count records that the user *chose* a station, so it must not be conditional on the server
/// being up — and the natural spelling, `player_play_station(..).await?` ahead of `mark_played`,
/// makes it exactly that. Pinned by reading the source because the alternative needs an
/// `AppState`, a socket and a station that is reliably down; the ordering is the whole invariant
/// and it is legible from the text.
#[test]
fn a_station_that_cannot_be_reached_is_still_counted_as_played() {
    let source = include_str!("../radio.rs");
    let body = source
        .split_once("pub async fn play_station")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(!body.is_empty(), "`play_station` moved or changed shape, so this pin reads nothing");
    assert!(
        matches!(
            (body.find("mark_played"), body.find("player_play_station")),
            (Some(counted), Some(opened)) if counted < opened
        ),
        "`play_station` must count the play before it opens the stream, or a station that is down \
         today never reaches the recents list that would let the user find it again"
    );
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

/// Both play doors take the same gate. A segmented station can be starred out of Browse, so it
/// reaches the kept tabs and is playable there by id — where the browsed refusal, being on the
/// other door, says nothing at all. The card hides its play button either way; this is the half
/// that holds when something else asks.
#[test]
fn a_segmented_station_is_refused_at_either_play_door() {
    assert!(ensure_playable(false).is_ok());
    assert!(
        matches!(ensure_playable(true), Err(AppError::Validation(_))),
        "Symphonia has no MPEG-TS demuxer, so this is a refusal rather than a decode failure"
    );

    let source = include_str!("../radio.rs");
    for door in [
        "pub async fn play_station",
        "pub async fn play_directory_station",
    ] {
        let body = source
            .split_once(door)
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map_or("", |(body, _)| body);
        assert!(
            body.contains("ensure_playable("),
            "`{door}` reaches the decoder and must go through the gate"
        );
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

/// **"Off" means no traffic, and this file is the only place that can be true.**
///
/// D15's switch is enforced at the facade rather than at the sidebar row, because a row that
/// disappears stops nothing a stale callback or an in-flight fetch has already started. What makes
/// one guard enough is that every outbound call reaches its client through
/// [`super::directory_client`], which is [`super::ensure_enabled`] plus the handle — so the check
/// is unskippable rather than remembered per call site.
///
/// `services::radio_browser::tests::only_the_radio_facade_reaches_the_directory_client` holds the
/// other direction, that nothing *outside* this module reaches the directory at all. Neither test
/// covers the other's half: that one would pass with every call here on a raw client, and this one
/// would pass with a second module fetching on its own.
///
/// A source walk because the alternative is asserting a network call did *not* happen, and the
/// tree has no network tests.
#[test]
fn every_outbound_call_takes_its_client_from_behind_the_switch() {
    const FACADE: &str = include_str!("../radio.rs");
    let src = crate::test_support::strip_line_comments(FACADE);

    // Receiver-agnostic: counting `state.http_client()` would leave a reach spelled off any other
    // binding uncounted, which is the one thing this test is for.
    let handles = src.matches(".http_client()").count();
    assert_eq!(
        handles, 1,
        "`http_client()` may be named exactly once in `library::radio`, inside `directory_client` \
         — every other reach past the guard is traffic a user who switched Radio off still pays"
    );

    let seam = src
        .split_once("fn directory_client")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);
    assert!(!seam.is_empty(), "`directory_client` moved or changed shape");
    assert!(
        seam.contains("ensure_enabled(state)?"),
        "the seam is only a seam while it asks `ensure_enabled` first"
    );
    assert!(
        seam.contains("http_client()"),
        "the one `http_client()` this test counts must be the one inside the seam"
    );
}
