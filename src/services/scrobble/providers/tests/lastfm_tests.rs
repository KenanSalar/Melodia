use std::collections::BTreeMap;

use super::{LastfmError, classify, love_params, now_playing_params, scrobble_params, sign};
use crate::services::scrobble::model::ScrobbleTrack;

fn make_track(artist: &str, title: &str) -> ScrobbleTrack {
    ScrobbleTrack {
        artist: artist.to_owned(),
        track: title.to_owned(),
        album: Some("Album".to_owned()),
        album_artist: None,
        duration_secs: Some(200),
        track_number: Some(3),
        recording_mbid: None,
        release_mbid: None,
    }
}

#[test]
fn sign_matches_known_vector() {
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getToken".to_owned());
    params.insert("api_key", "KEY".to_owned());

    // Sorted by name → "api_key" + "KEY" + "method" + "auth.getToken", then the
    // shared secret appended. The MD5 of that string is a fixed, precomputed hex.
    let signature = sign(&params, "SECRET");
    assert_eq!(
        signature,
        format!("{:x}", md5::compute("api_keyKEYmethodauth.getTokenSECRET"))
    );
    assert_eq!(signature, "66ee63a18da3c919f987b342697d913c");
}

#[test]
fn now_playing_params_omit_signature_and_format() {
    let params = now_playing_params("KEY", "SK", &make_track("Artist", "Song"));

    assert_eq!(
        params.get("method").map(String::as_str),
        Some("track.updateNowPlaying")
    );
    assert_eq!(params.get("artist").map(String::as_str), Some("Artist"));
    // `format` and `api_sig` are added only after signing — the builder omits them.
    assert!(!params.contains_key("format"));
    assert!(!params.contains_key("api_sig"));
}

#[test]
fn scrobble_params_use_bracketed_indices() {
    let first = make_track("Artist A", "Song A");
    let second = make_track("Artist B", "Song B");
    let batch = [(&first, 1_700_000_000_i64), (&second, 1_700_000_100_i64)];
    let params = scrobble_params("KEY", "SK", &batch);

    assert_eq!(
        params.get("method").map(String::as_str),
        Some("track.scrobble")
    );
    assert_eq!(params.get("artist[0]").map(String::as_str), Some("Artist A"));
    assert_eq!(params.get("track[1]").map(String::as_str), Some("Song B"));
    assert_eq!(
        params.get("timestamp[0]").map(String::as_str),
        Some("1700000000")
    );
    assert_eq!(params.get("album[1]").map(String::as_str), Some("Album"));
    assert!(!params.contains_key("format"));
    assert!(!params.contains_key("api_sig"));
}

#[test]
fn love_params_select_method_by_flag() {
    let track = make_track("Artist", "Song");

    let loved = love_params("KEY", "SK", &track, true);
    assert_eq!(loved.get("method").map(String::as_str), Some("track.love"));

    let unloved = love_params("KEY", "SK", &track, false);
    assert_eq!(
        unloved.get("method").map(String::as_str),
        Some("track.unlove")
    );
}

#[test]
fn error_codes_classify_by_retryability() {
    assert!(matches!(
        classify(9, String::new()),
        LastfmError::InvalidSession
    ));
    assert!(matches!(
        classify(11, String::new()),
        LastfmError::Transient { .. }
    ));
    assert!(matches!(
        classify(16, String::new()),
        LastfmError::Transient { .. }
    ));
    assert!(matches!(
        classify(29, String::new()),
        LastfmError::Transient { .. }
    ));
    assert!(matches!(
        classify(6, String::new()),
        LastfmError::Api { code: 6, .. }
    ));
}
