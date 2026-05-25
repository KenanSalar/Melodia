use super::*;

fn make_track() -> Track {
    Track {
        id: 42,
        file_path: "/music/song.flac".into(),
        file_name: "song.flac".into(),
        file_hash: Some("abc123".into()),
        title: "Test Song".into(),
        artist: Some("Artist".into()),
        album_artist: Some("Album Artist".into()),
        album: Some("Album".into()),
        genre: Some("Rock".into()),
        track_number: Some(3),
        disc_number: Some(1),
        year: Some(2024),
        composer: Some("Composer".into()),
        comment: None,
        bpm: Some(120.0),
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
        label: None,
        original_year: None,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
        duration_ms: 210_000,
        file_size: Some(5_000_000),
        codec: Some("flac".into()),
        bitrate: Some(1411),
        channels: Some(2),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        artwork_path: Some("/art/cover.jpg".into()),
        play_count: 5,
        skip_count: 1,
        rating: 4,
        is_favorite: true,
        last_played: Some("2024-06-01T12:00:00Z".into()),
        last_position: 45000,
        album_id: Some(10),
        artist_id: Some(20),
        genre_id: Some(30),
        folder_id: 1,
        date_added: "2024-01-01T00:00:00Z".into(),
        date_modified: None,
        sort_key: None,
    }
}

#[test]
fn from_track_maps_all_summary_fields() {
    let track = make_track();
    let summary = TrackSummary::from(track.clone());

    assert_eq!(summary.id, track.id);
    assert_eq!(summary.file_path, track.file_path);
    assert_eq!(summary.file_name, track.file_name);
    assert_eq!(summary.title, track.title);
    assert_eq!(summary.artist, track.artist);
    assert_eq!(summary.album, track.album);
    assert_eq!(summary.duration_ms, track.duration_ms);
    assert_eq!(summary.artwork_path, track.artwork_path);
    assert_eq!(summary.track_number, track.track_number);
    assert_eq!(summary.disc_number, track.disc_number);
    assert_eq!(summary.last_position, track.last_position);
    assert_eq!(summary.is_favorite, track.is_favorite);
}

#[test]
fn from_ref_produces_same_result() {
    let track = make_track();
    let from_owned = TrackSummary::from(track.clone());
    let from_ref = TrackSummary::from(&track);
    assert_eq!(from_owned, from_ref);
}

#[test]
fn from_track_preserves_none_optionals() {
    let track = Track {
        artist: None,
        album: None,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        ..make_track()
    };
    let summary = TrackSummary::from(track);
    assert_eq!(summary.artist, None);
    assert_eq!(summary.album, None);
    assert_eq!(summary.artwork_path, None);
    assert_eq!(summary.track_number, None);
    assert_eq!(summary.disc_number, None);
}
