//! What a Copy entry puts on the clipboard.
//!
//! The menus hand over ids and a field token, so the two trees meet at the token: a spelling Rust
//! doesn't parse copies nothing and says so only in the log. The menus are read here rather than
//! their tokens restated, so a flyout row added without a parser arm fails.
//!
//! The lookups under `track_lines` and `entity_lines` are the store's to pin; what is pinned here
//! is what becomes of a row once it has been read.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use super::*;
use melodia_engine::player::engine::fixtures::test_track;
use melodia_testkit::strip_line_comments;

const TRACK_COPY_ITEMS: &str =
    include_str!("../../../../melodia-ui/ui/components/track-copy-items.slint");
const CARD_CONTEXT_MENU: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/card-context-menu.slint");
const STATION_CONTEXT_MENU: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/station-context-menu.slint");

const TRACK_TOKENS: [(&str, TrackField); 5] = [
    ("title", TrackField::Title),
    ("artist", TrackField::Artist),
    ("album", TrackField::Album),
    ("artist-title", TrackField::ArtistTitle),
    ("path", TrackField::Path),
];

const ENTITY_TOKENS: [(&str, EntityField); 3] = [
    ("name", EntityField::Name),
    ("artist", EntityField::Artist),
    ("artist-name", EntityField::ArtistName),
];

const STATION_TOKENS: [(&str, StationField); 3] = [
    ("name", StationField::Name),
    ("stream-url", StationField::StreamUrl),
    ("website", StationField::Website),
];

/// The string literal opening each `needle` in `src`, comments dropped.
fn literals_after(src: &str, needle: &str) -> Vec<String> {
    strip_line_comments(src)
        .split(needle)
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(literal, _)| literal.to_owned()))
        .collect()
}

/// The field each `CopyActions.copy-entities(kind, field, ids)` call passes. Its kind is a literal
/// at some calls and a property at others, so the field is read by position.
fn entity_copy_fields(src: &str) -> BTreeSet<String> {
    strip_line_comments(src)
        .split("CopyActions.copy-entities(")
        .skip(1)
        .filter_map(|call| call.split(',').nth(1))
        .map(|field| field.trim().trim_matches('"').to_owned())
        .collect()
}

#[test]
fn every_copy_token_resolves_to_its_own_field() {
    for (token, field) in TRACK_TOKENS {
        assert_eq!(TrackField::from_token(token), Some(field), "track token {token:?}");
    }
    for (token, field) in ENTITY_TOKENS {
        assert_eq!(EntityField::from_token(token), Some(field), "card token {token:?}");
    }
    for (token, field) in STATION_TOKENS {
        assert_eq!(StationField::from_token(token), Some(field), "station token {token:?}");
    }
}

#[test]
fn a_token_no_menu_spells_resolves_to_nothing() {
    for token in ["", "Title", "titles", "artist_title", "file-path", "url", "homepage"] {
        assert_eq!(TrackField::from_token(token), None, "track token {token:?}");
        assert_eq!(EntityField::from_token(token), None, "card token {token:?}");
        assert_eq!(StationField::from_token(token), None, "station token {token:?}");
    }
}

/// In the flyout's own order, which is the order a host reserves its height against.
#[test]
fn the_track_flyout_offers_exactly_the_fields_rust_parses() {
    assert_eq!(literals_after(TRACK_COPY_ITEMS, "token: \""), TRACK_TOKENS.map(|(token, _)| token));
}

#[test]
fn the_card_menu_copies_only_fields_rust_parses() {
    assert_eq!(
        entity_copy_fields(CARD_CONTEXT_MENU),
        ENTITY_TOKENS.iter().map(|(token, _)| (*token).to_owned()).collect::<BTreeSet<_>>()
    );
}

#[test]
fn the_station_menu_offers_exactly_the_fields_rust_parses() {
    assert_eq!(
        literals_after(STATION_CONTEXT_MENU, "root.copy(\""),
        STATION_TOKENS.map(|(token, _)| token)
    );
}

fn track(title: &str, artist: Option<&str>, album: Option<&str>) -> TrackSummary {
    Arc::unwrap_or_clone(test_track(title, artist, album))
}

#[test]
fn each_track_field_copies_its_own_column() {
    let path =
        Path::new("Music").join("Field").join("Nocturne.flac").to_string_lossy().into_owned();
    let mut nocturne = track("Nocturne", Some("Field"), Some("Airs"));
    nocturne.file_path.clone_from(&path);

    let cases = [
        (TrackField::Title, "Nocturne"),
        (TrackField::Artist, "Field"),
        (TrackField::Album, "Airs"),
        (TrackField::ArtistTitle, "Field - Nocturne"),
        (TrackField::Path, path.as_str()),
    ];
    for (field, expected) in cases {
        assert_eq!(track_line(&nocturne, field).as_deref(), Some(expected), "{field:?}");
    }
}

/// A pasted list should need no cleaning, so a blank field is as missing as an absent one.
#[test]
fn a_track_without_the_field_asked_for_contributes_no_line() {
    let bare = track("Nocturne", Some("   "), None);

    for field in [TrackField::Artist, TrackField::Album, TrackField::Path] {
        assert_eq!(track_line(&bare, field), None, "{field:?}");
    }
}

#[test]
fn a_copied_field_arrives_trimmed() {
    let padded = track("  Nocturne ", Some(" Field  "), Some(" Airs "));

    let cases = [
        (TrackField::Title, "Nocturne"),
        (TrackField::Artist, "Field"),
        (TrackField::Album, "Airs"),
        (TrackField::ArtistTitle, "Field - Nocturne"),
    ];
    for (field, expected) in cases {
        assert_eq!(track_line(&padded, field).as_deref(), Some(expected), "{field:?}");
    }
}

/// The credit is the half that can be missing; a line with no title says nothing, credit or not.
#[test]
fn artist_and_title_fall_back_to_the_title_alone() {
    let cases = [
        (Some("Field"), "Nocturne", Some("Field - Nocturne")),
        (None, "Nocturne", Some("Nocturne")),
        (Some("   "), "Nocturne", Some("Nocturne")),
        (Some("Field"), "   ", None),
    ];
    for (artist, title, expected) in cases {
        let line = track_line(&track(title, artist, None), TrackField::ArtistTitle);
        assert_eq!(line.as_deref(), expected, "artist {artist:?}, title {title:?}");
    }
}

fn label(name: &str, artist: Option<&str>) -> EntityLabel {
    EntityLabel { name: name.to_owned(), artist: artist.map(str::to_owned) }
}

#[test]
fn an_album_card_copies_its_name_its_credit_or_both() {
    let album = label("Duets", Some("First & Second"));

    let cases = [
        (EntityField::Name, "Duets"),
        (EntityField::Artist, "First & Second"),
        (EntityField::ArtistName, "First & Second - Duets"),
    ];
    for (field, expected) in cases {
        assert_eq!(entity_line(&album, field).as_deref(), Some(expected), "{field:?}");
    }
}

/// An artist, genre or playlist card credits nobody, and Copy Artist on a mixed selection must
/// not hand back blank lines for them.
#[test]
fn a_card_crediting_no_artist_copies_no_artist_line() {
    let genre = label("Shoegaze", None);

    assert_eq!(entity_line(&genre, EntityField::Artist), None);
    assert_eq!(entity_line(&genre, EntityField::ArtistName).as_deref(), Some("Shoegaze"));
}

/// The ids are in the order the selection was picked, which is the order Play queues it in.
#[test]
fn cards_copy_in_the_order_they_were_handed_over() {
    let labels =
        HashMap::from([(1, label("A", None)), (2, label("B", None)), (3, label("C", None))]);

    assert_eq!(lines_in_order(&[3, 1, 2], labels, EntityField::Name), "C\nA\nB");
}

#[test]
fn a_card_deleted_since_the_grid_painted_contributes_nothing() {
    let labels = HashMap::from([(1, label("A", None)), (2, label("B", None))]);

    assert_eq!(lines_in_order(&[1, 99, 2], labels, EntityField::Name), "A\nB");
}

/// The empty string is what the clipboard writer reads as nothing to copy, so a set with nothing
/// in the field must come back as exactly that rather than as a run of line breaks.
#[test]
fn a_set_with_nothing_in_the_field_copies_the_empty_string() {
    let labels = HashMap::from([(1, label("A", None)), (2, label("B", None))]);

    assert_eq!(lines_in_order(&[1, 2], labels, EntityField::Artist), "");
}

fn station<'a>(name: &'a str, website: Option<&'a str>) -> StationText<'a> {
    StationText { name, stream_url: "https://example.test/live.mp3", website }
}

#[test]
fn each_station_field_copies_its_own_text() {
    let night = station("Night Radio", Some("https://night.example"));

    let cases = [
        (StationField::Name, "Night Radio"),
        (StationField::StreamUrl, "https://example.test/live.mp3"),
        (StationField::Website, "https://night.example"),
    ];
    for (field, expected) in cases {
        assert_eq!(station_lines([night], field), expected, "{field:?}");
    }
}

/// A name holds spaces of its own, so only a line break can say where one station ends.
#[test]
fn stations_copy_one_per_line_in_the_order_handed_over() {
    let stations = [station("Night Radio", None), station("Day Radio", None)];

    assert_eq!(station_lines(stations, StationField::Name), "Night Radio\nDay Radio");
}

#[test]
fn a_station_with_no_website_adds_no_line() {
    let stations = [
        station("Night Radio", None),
        station("Day Radio", Some("  ")),
        station("Dawn Radio", Some("https://dawn.example")),
    ];

    assert_eq!(station_lines(stations, StationField::Website), "https://dawn.example");
}
