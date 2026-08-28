//! What each surface is told is playing.
//!
//! The ladder here is the one MPRIS, the tray, the Discord card and the Slint bridge all read, so
//! a change to it moves four surfaces at once and none of them tests the song-or-station question
//! for itself. Assertions compare the whole `Option` rather than unwrapping one: a source that
//! went missing is the failure worth reading, and `panic!` is denied here as everywhere.

use std::sync::Arc;

use super::{SourceId, SourceSummary};
use crate::player::tests::helpers::{test_station, test_track, test_view_model as deck};
use crate::player::types::RadioNowPlaying;

/// Mirrors the stream URL [`test_station`] hands back; spelled here because the assertions below
/// compare a whole `SourceSummary` and the id is one of its fields.
const STREAM_URL: &str = "http://example.test/live.mp3";

/// [`test_station`] with the two fields this suite varies filled in.
fn tuned_to(name: &str, live_title: Option<&str>) -> Arc<RadioNowPlaying> {
    let mut station = Arc::unwrap_or_clone(test_station(name));
    station.live_title = live_title.map(str::to_owned);
    station.artwork_path = Some("logo.png".to_owned());
    Arc::new(station)
}

/// The two lines a surface draws, so a test naming the ladder asserts on nothing else.
fn lines(source: Option<SourceSummary<'_>>) -> Option<(&str, Option<&str>)> {
    source.map(|s| (s.title, s.secondary))
}

#[test]
fn nothing_on_the_deck_has_no_source() {
    assert_eq!(deck(None, None, 0).source(), None);
}

#[test]
fn a_track_reports_its_own_tags() {
    let vm = deck(Some(test_track("Nocturne", Some("Field"), Some("Airs"))), None, 200_000);

    assert_eq!(
        vm.source(),
        Some(SourceSummary {
            id: SourceId::Track(7),
            title: "Nocturne",
            secondary: Some("Field"),
            album: Some("Airs"),
            artwork_path: Some("cover.jpg"),
            duration_ms: Some(200_000),
        })
    );
}

#[test]
fn a_station_lends_its_name_until_it_announces_a_song() {
    let vm = deck(None, Some(tuned_to("Night Radio", None)), 0);

    assert_eq!(
        vm.source(),
        Some(SourceSummary {
            id: SourceId::Station(STREAM_URL),
            title: "Night Radio",
            // Nothing to add while the line above is the station itself.
            secondary: None,
            album: None,
            artwork_path: Some("logo.png"),
            duration_ms: None,
        })
    );
}

#[test]
fn an_announced_song_takes_the_title_and_the_station_drops_a_line() {
    let vm = deck(None, Some(tuned_to("Night Radio", Some("Field - Nocturne"))), 0);

    assert_eq!(lines(vm.source()), Some(("Field - Nocturne", Some("Night Radio"))));
}

#[test]
fn a_station_never_reports_an_album() {
    let announced = deck(None, Some(tuned_to("Night Radio", Some("Field - Nocturne"))), 0);
    let silent = deck(None, Some(tuned_to("Night Radio", None)), 0);

    assert_eq!(announced.source().map(|s| s.album), Some(None));
    assert_eq!(silent.source().map(|s| s.album), Some(None));
}

#[test]
fn a_live_source_reports_no_length_where_an_untimed_track_reports_zero() {
    // MPRIS renders an absent length and a zero one differently, so the two may not collapse.
    let station = deck(None, Some(tuned_to("Night Radio", None)), 300_000);
    assert_eq!(station.source().map(|s| s.duration_ms), Some(None));

    let untimed = deck(Some(test_track("Nocturne", None, None)), None, 0);
    assert_eq!(untimed.source().map(|s| s.duration_ms), Some(Some(0)));
}

#[test]
fn a_blank_field_arrives_absent_rather_than_empty() {
    let mut track = Arc::unwrap_or_clone(test_track("Nocturne", Some("  "), Some("")));
    track.artwork_path = Some("   ".to_owned());
    let vm = deck(Some(Arc::new(track)), None, 200_000);

    assert_eq!(
        vm.source().map(|s| (s.secondary, s.album, s.artwork_path)),
        Some((None, None, None))
    );
}

#[test]
fn a_whitespace_announcement_leaves_the_station_lending_its_name() {
    let vm = deck(None, Some(tuned_to("Night Radio", Some("   "))), 0);

    assert_eq!(lines(vm.source()), Some(("Night Radio", None)));
}

#[test]
fn a_station_is_keyed_on_its_stream_url() {
    // Every station the user has only browsed to carries `station_id == 0`, so the id cannot tell
    // two of them apart and the identity a consumer dedupes on is the URL.
    let mut first = Arc::unwrap_or_clone(tuned_to("First", None));
    first.station_id = 0;
    let mut second = first.clone();
    second.name = "Second".to_owned();
    second.stream_url = "http://example.test/other.mp3".to_owned();

    let first = deck(None, Some(Arc::new(first)), 0);
    let second = deck(None, Some(Arc::new(second)), 0);

    assert_ne!(first.source().map(|s| s.id), second.source().map(|s| s.id));
}

#[test]
fn the_station_wins_when_both_halves_are_set() {
    // Unreachable through the state machine, which clears one as it sets the other. Pinned because
    // the ladder promises the more specific of the two rather than whichever it reads first.
    let vm = deck(
        Some(test_track("Nocturne", Some("Field"), Some("Airs"))),
        Some(tuned_to("Night Radio", None)),
        200_000,
    );

    assert_eq!(
        vm.source().map(|s| (s.id, s.title, s.duration_ms)),
        Some((SourceId::Station(STREAM_URL), "Night Radio", None))
    );
}
