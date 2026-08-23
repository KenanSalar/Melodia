//! The two local tabs' order and their needle.
//!
//! Both are in-memory walks over a list the user filled by hand, so nothing on screen reports a
//! mistake in either: a wrong comparator is a list in a plausible-looking order, and a field
//! missing from the haystack is a station the box can never find.

use super::*;

/// A kept station with the four fields the order and the needle read, and nothing else that
/// matters here.
fn station(name: &str, added: &str, plays: i32, last_played: Option<&str>) -> RadioStation {
    RadioStation {
        id: 1,
        station_uuid: None,
        name: name.to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        favicon_url: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: true,
        // What `to_natural_sort_key` writes for these names, spelled out rather than derived: the
        // order under test is the column's, and re-deriving it here would hide a comparator that
        // sorted on `name` instead.
        sort_key: name.to_lowercase(),
        date_added: added.to_owned(),
        last_played: last_played.map(str::to_owned),
        play_count: plays,
    }
}

/// The names `sort_stations` leaves behind, in order.
fn ordered<'a>(stations: &'a [RadioStation], field: &str, dir: SortDir) -> Vec<&'a str> {
    let mut refs: Vec<&RadioStation> = stations.iter().collect();
    sort_stations(&mut refs, field, dir);
    refs.into_iter().map(|s| s.name.as_str()).collect()
}

/// Four stations that disagree on every axis, so no two fields can produce the same order by
/// accident. `Beta` and `Delta` share a play count, which is what the tiebreak is for.
fn sample() -> Vec<RadioStation> {
    vec![
        station("Delta", "2026-01-04T00:00:00.000+00:00", 5, Some("2026-02-01T00:00:00.000+00:00")),
        station("Alpha", "2026-01-03T00:00:00.000+00:00", 1, None),
        station(
            "Charlie",
            "2026-01-02T00:00:00.000+00:00",
            9,
            Some("2026-03-01T00:00:00.000+00:00"),
        ),
        station("Beta", "2026-01-01T00:00:00.000+00:00", 5, Some("2026-01-01T00:00:00.000+00:00")),
    ]
}

/// Each field, and each of them reversed. The default arm is name order rather than "leave it
/// alone": a `views.json` written by a build with a field this one doesn't know must land on
/// something the arrow beside it agrees with.
#[test]
fn every_sort_field_orders_by_itself_and_reverses_cleanly() {
    let stations = sample();

    assert_eq!(ordered(&stations, "name", SortDir::Asc), ["Alpha", "Beta", "Charlie", "Delta"]);
    assert_eq!(ordered(&stations, "name", SortDir::Desc), ["Delta", "Charlie", "Beta", "Alpha"]);

    assert_eq!(ordered(&stations, "added", SortDir::Asc), ["Beta", "Charlie", "Alpha", "Delta"]);
    assert_eq!(
        ordered(&stations, "added", SortDir::Desc),
        ["Delta", "Alpha", "Charlie", "Beta"],
        "most recently added first is what the descending arrow has to mean"
    );

    assert_eq!(
        ordered(&stations, "unknown-field", SortDir::Asc),
        ["Alpha", "Beta", "Charlie", "Delta"],
        "an unrecognised field falls through to name order rather than to whatever SQL returned"
    );
}

/// The tiebreak is the whole reason these compare two keys. Without it `Beta` and `Delta` — equal
/// on plays — swap places between refreshes, which reads as the list reshuffling itself.
#[test]
fn stations_level_on_a_field_break_the_tie_by_name() {
    let stations = sample();
    assert_eq!(ordered(&stations, "plays", SortDir::Asc), ["Alpha", "Beta", "Delta", "Charlie"]);
    assert_eq!(
        ordered(&stations, "plays", SortDir::Desc),
        ["Charlie", "Delta", "Beta", "Alpha"],
        "reversing has to reverse the tiebreak too, or the two orders disagree about the pair"
    );
}

/// A station never played has no stamp at all, and `None` sorts below every `Some` — so the
/// never-played end of the list gathers at one end rather than scattering through it.
#[test]
fn a_station_never_played_sorts_below_every_station_that_was() {
    let stations = sample();
    assert_eq!(
        ordered(&stations, "played", SortDir::Asc),
        ["Alpha", "Beta", "Delta", "Charlie"],
        "the unplayed station leads the ascending order"
    );
    assert_eq!(
        ordered(&stations, "played", SortDir::Desc),
        ["Charlie", "Delta", "Beta", "Alpha"],
        "and trails the descending one, which is where 'most recently played' wants it"
    );
}

/// A kept station carrying a directory uuid and a stored logo, which is the pair Browse falls
/// back on.
fn logo_row(id: i64, uuid: &str, artwork_path: Option<&str>) -> RadioStation {
    let mut station = station("Kept", "2026-01-01T00:00:00.000+00:00", 0, None);
    station.id = id;
    station.station_uuid = Some(uuid.to_owned());
    station.artwork_path = artwork_path.map(str::to_owned);
    station
}

/// The map is a projection of the two caches, and the heal is what breaks a copy that isn't.
///
/// A station played out of Browse is kept with no logo, found on its own site a moment later, and
/// patched into the caches by `adopt_logo_path` — so a map built before that lands describes a
/// list that has since moved, and Browse paints a monogram beside a tab drawing the real thing.
/// Only a second play fixed it, because only a second play re-read the row.
#[test]
fn a_logo_found_after_the_refresh_still_reaches_browse() {
    let radio_ui = RadioUi::new(false);
    radio_ui.recent.lock().stations = vec![logo_row(7, "uuid-7", None)];
    remember_logos(&radio_ui);
    assert!(radio_ui.known_logos.lock().is_empty(), "the row had no logo yet");

    adopt_logo_path(&radio_ui, 7, "/store/7.png");
    remember_logos(&radio_ui);

    assert_eq!(radio_ui.known_logos.lock().get("uuid-7").map(String::as_str), Some("/store/7.png"));
}

/// The other half, and the one the bug actually lived in: the helper was right and the heal never
/// called it.
///
/// A walk rather than a driven `heal_logos`, that one being network work behind a `JoinSet`. Both
/// write sites are asserted, since a refresh that skipped it would leave Browse a whole section
/// behind instead of one heal behind.
#[test]
fn every_write_to_the_two_caches_re_derives_the_map() {
    let source = include_str!("../kept.rs");
    for site in ["pub fn refresh", "async fn heal_logos"] {
        let body = source
            .split_once(site)
            .and_then(|(_, rest)| rest.split_once("\n}\n"))
            .map_or("", |(body, _)| body);
        assert!(
            body.contains("remember_logos("),
            "`{site}` moves what a row holds, and Browse reads the map rather than the caches"
        );
    }
}

/// Both lists feed it, and a station the user typed in feeds neither: Browse asks the directory,
/// which has no name for a row that was never in it.
#[test]
fn the_map_spans_both_tabs_and_skips_what_no_directory_page_can_name() {
    let radio_ui = RadioUi::new(false);
    radio_ui.kept.lock().stations = vec![logo_row(1, "uuid-1", Some("/store/1.png")), {
        let mut typed = logo_row(2, "uuid-2", Some("/store/2.png"));
        typed.station_uuid = None;
        typed
    }];
    radio_ui.recent.lock().stations = vec![logo_row(3, "uuid-3", Some("/store/3.png"))];

    remember_logos(&radio_ui);

    let known = radio_ui.known_logos.lock();
    assert_eq!(known.len(), 2);
    assert!(known.contains_key("uuid-1"), "a favorite");
    assert!(known.contains_key("uuid-3"), "and a station only ever played");
}

/// A path the refresh already blanked must not come back through the map — the file is gone, and
/// a card handed a dead path drew nothing at all where a monogram was the honest answer.
#[test]
fn a_row_whose_logo_went_missing_contributes_nothing() {
    let radio_ui = RadioUi::new(false);
    radio_ui.kept.lock().stations = vec![logo_row(1, "uuid-1", Some("/store/1.png"))];
    remember_logos(&radio_ui);
    assert_eq!(radio_ui.known_logos.lock().len(), 1);

    radio_ui.kept.lock().stations = vec![logo_row(1, "uuid-1", None)];
    remember_logos(&radio_ui);
    assert!(radio_ui.known_logos.lock().is_empty());
}

/// What the box can reach. The card shows name, country, codec and tags, and the needle covers
/// all four — a field drawn but not searchable is the shape of miss nobody reports, since the
/// station is visibly right there while the filter says it is not.
#[test]
fn the_needle_reaches_every_field_the_card_draws() {
    let mut station = station("Jazz FM", "2026-01-01T00:00:00.000+00:00", 0, None);
    station.tags = "smooth jazz, lounge".to_owned();
    station.country = "Germany".to_owned();
    station.codec = "MP3".to_owned();

    for needle in ["jazz", "lounge", "germany", "mp3", "FM"] {
        assert!(
            station_matches(&station, &row_match::fold_needle(needle)),
            "{needle:?} names something the card draws and must match"
        );
    }
    assert!(!station_matches(&station, &row_match::fold_needle("classical")));
    assert!(
        !station_matches(&station, &row_match::fold_needle("https")),
        "the stream URL is not on the card and must not be searchable — every station would match"
    );
}

/// The fold is what makes the box work on the directory's own data, which is full of accents and
/// of names the user will type in lower case.
#[test]
fn the_needle_ignores_case_and_accents() {
    let mut station = station("Radio Días", "2026-01-01T00:00:00.000+00:00", 0, None);
    station.country = "España".to_owned();

    assert!(station_matches(&station, &row_match::fold_needle("dias")));
    assert!(station_matches(&station, &row_match::fold_needle("ESPANA")));
    assert!(
        station_matches(&station, &row_match::fold_needle("")),
        "an empty needle matches everything, which is what lets the walk run unconditionally"
    );
}
