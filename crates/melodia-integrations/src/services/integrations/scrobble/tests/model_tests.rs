use super::{ScrobbleTrack, scrobble_threshold_ms};
use melodia_core::entities::track::ScrobbleRow;

/// A fully-populated row for the `from_row` conversion tests.
fn row() -> ScrobbleRow {
    ScrobbleRow {
        id: 7,
        title: "Song".to_owned(),
        artist: Some("Artist".to_owned()),
        album: Some("Album".to_owned()),
        album_artist: Some("Album Artist".to_owned()),
        duration_ms: 183_000,
        track_number: Some(4),
        musicbrainz_track_id: Some("rec-mbid".to_owned()),
        musicbrainz_release_id: Some("rel-mbid".to_owned()),
    }
}

#[test]
fn from_row_maps_every_field() {
    assert_eq!(
        ScrobbleTrack::from_row(&row()),
        Some(ScrobbleTrack {
            artist: "Artist".to_owned(),
            track: "Song".to_owned(),
            album: Some("Album".to_owned()),
            album_artist: Some("Album Artist".to_owned()),
            duration_secs: Some(183),
            track_number: Some(4),
            recording_mbid: Some("rec-mbid".to_owned()),
            release_mbid: Some("rel-mbid".to_owned()),
        })
    );
}

#[test]
fn from_row_needs_an_artist_and_a_title() {
    let mut blank_artist = row();
    blank_artist.artist = Some("   ".to_owned());
    assert_eq!(ScrobbleTrack::from_row(&blank_artist), None);

    let mut no_artist = row();
    no_artist.artist = None;
    assert_eq!(ScrobbleTrack::from_row(&no_artist), None);

    let mut blank_title = row();
    blank_title.title = "  ".to_owned();
    assert_eq!(ScrobbleTrack::from_row(&blank_title), None);
}

#[test]
fn from_row_drops_unknown_duration_and_blank_optionals() {
    let mut r = row();
    r.duration_ms = 0;
    r.album = Some(String::new());
    r.track_number = None;
    r.musicbrainz_track_id = Some("   ".to_owned());

    assert_eq!(
        ScrobbleTrack::from_row(&r),
        Some(ScrobbleTrack {
            artist: "Artist".to_owned(),
            track: "Song".to_owned(),
            album: None,
            album_artist: Some("Album Artist".to_owned()),
            duration_secs: None,
            track_number: None,
            recording_mbid: None,
            release_mbid: Some("rel-mbid".to_owned()),
        })
    );
}

#[test]
fn unknown_duration_falls_back_to_four_minutes() {
    assert_eq!(scrobble_threshold_ms(0), Some(240_000));
}

#[test]
fn tracks_at_or_under_thirty_seconds_never_scrobble() {
    assert_eq!(scrobble_threshold_ms(30_000), None);
    assert_eq!(scrobble_threshold_ms(15_000), None);
    assert_eq!(scrobble_threshold_ms(1), None);
}

#[test]
fn just_over_thirty_seconds_uses_half() {
    assert_eq!(scrobble_threshold_ms(30_001), Some(15_000));
}

#[test]
fn medium_track_uses_half_its_duration() {
    assert_eq!(scrobble_threshold_ms(100_000), Some(50_000));
}

#[test]
fn long_track_caps_at_four_minutes() {
    // Half of 8 minutes is exactly the 4-minute cap.
    assert_eq!(scrobble_threshold_ms(480_000), Some(240_000));
    assert_eq!(scrobble_threshold_ms(600_000), Some(240_000));
}
