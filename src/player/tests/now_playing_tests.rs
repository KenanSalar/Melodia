//! What each surface is told is playing.
//!
//! The ladder here is the one MPRIS, the tray, the Discord card and the Slint bridge all read, so
//! a change to it moves four surfaces at once and none of them tests the song-or-station question
//! for itself. Assertions compare the whole `Option` rather than unwrapping one: a source that
//! went missing is the failure worth reading, and `panic!` is denied here as everywhere.

use std::sync::Arc;

use super::{SourceId, SourceSummary};
use crate::entities::track::TrackSummary;
use crate::player::state::PlayerViewModelLight;
use crate::player::tests::helpers::test_station;
use crate::player::types::RadioNowPlaying;

const STREAM_URL: &str = "http://example.test/live.mp3";

fn test_track(title: &str, artist: Option<&str>, album: Option<&str>) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id: 7,
        file_path: String::new(),
        file_name: String::new(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album: album.map(str::to_owned),
        duration_ms: 200_000,
        artwork_path: Some("cover.jpg".to_owned()),
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// [`test_station`] with the two fields this suite varies filled in.
fn tuned_to(name: &str, live_title: Option<&str>) -> Arc<RadioNowPlaying> {
    let mut station = (*test_station(name)).clone();
    station.live_title = live_title.map(str::to_owned);
    station.artwork_path = Some("logo.png".to_owned());
    Arc::new(station)
}

fn deck(
    current_track: Option<Arc<TrackSummary>>,
    radio: Option<Arc<RadioNowPlaying>>,
    duration_ms: u64,
) -> PlayerViewModelLight {
    PlayerViewModelLight {
        status: "playing",
        current_track,
        position_ms: 0,
        duration_ms,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        radio,
        has_next: false,
        has_previous: false,
    }
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
    let mut track = (*test_track("Nocturne", Some("  "), Some(""))).clone();
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
    let mut first = (*tuned_to("First", None)).clone();
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
