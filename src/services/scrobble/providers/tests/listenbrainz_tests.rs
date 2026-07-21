use super::{feedback_payload, listens_payload, playing_now_payload};
use crate::services::scrobble::model::ScrobbleTrack;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn make_track() -> ScrobbleTrack {
    ScrobbleTrack {
        artist: "Artist".to_owned(),
        track: "Song".to_owned(),
        album: Some("Album".to_owned()),
        album_artist: None,
        duration_secs: Some(180),
        track_number: Some(3),
        recording_mbid: None,
        release_mbid: None,
    }
}

#[test]
fn playing_now_omits_listened_at() -> TestResult {
    let track = make_track();
    let value = serde_json::to_value(playing_now_payload(&track))?;

    assert_eq!(value["listen_type"], "playing_now");
    let item = &value["payload"][0];
    // "now playing" carries no timestamp — the field is skipped, not null.
    assert!(item.get("listened_at").is_none());
    assert_eq!(item["track_metadata"]["track_name"], "Song");
    Ok(())
}

#[test]
fn single_listen_includes_timestamp_and_client_info() -> TestResult {
    let track = make_track();
    let batch = [(&track, 1_700_000_000_i64)];
    let value = serde_json::to_value(listens_payload(&batch))?;

    assert_eq!(value["listen_type"], "single");
    let item = &value["payload"][0];
    assert_eq!(item["listened_at"], 1_700_000_000_i64);

    let info = &item["track_metadata"]["additional_info"];
    assert_eq!(info["submission_client"], "Melodia");
    assert_eq!(info["submission_client_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["duration_ms"], 180_000_i64); // 180 s → ms
    Ok(())
}

#[test]
fn multiple_listens_use_import_type() -> TestResult {
    let track = make_track();
    let batch = [(&track, 1_i64), (&track, 2_i64)];
    let value = serde_json::to_value(listens_payload(&batch))?;

    assert_eq!(value["listen_type"], "import");
    assert_eq!(value["payload"].as_array().map(Vec::len), Some(2));
    Ok(())
}

#[test]
fn feedback_payload_maps_love_state_to_score() -> TestResult {
    let loved = serde_json::to_value(feedback_payload("mbid-1", 1))?;
    assert_eq!(loved["recording_mbid"], "mbid-1");
    assert_eq!(loved["score"], 1);

    // Unlove clears the feedback with score 0.
    let cleared = serde_json::to_value(feedback_payload("mbid-1", 0))?;
    assert_eq!(cleared["score"], 0);
    Ok(())
}
