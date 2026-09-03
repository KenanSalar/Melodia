use super::{
    BulkLookupResult, LookupQuery, align_bulk_results, bulk_lookup_payload, feedback_payload,
    listens_payload, mbid_match, playing_now_payload,
};
use crate::services::integrations::scrobble::model::ScrobbleTrack;

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

#[test]
fn bulk_lookup_payload_omits_absent_release() -> TestResult {
    let queries = [
        LookupQuery {
            artist: "50 Cent",
            title: "Candy Shop",
            release: Some("The Massacre"),
        },
        LookupQuery {
            artist: "Nas",
            title: "N.Y. State of Mind",
            release: None,
        },
    ];
    let value = serde_json::to_value(bulk_lookup_payload(&queries))?;

    let recs = &value["recordings"];
    assert_eq!(recs[0]["artist_name"], "50 Cent");
    assert_eq!(recs[0]["recording_name"], "Candy Shop");
    assert_eq!(recs[0]["release_name"], "The Massacre");
    // A missing release is skipped, not serialized as null.
    assert!(recs[1].get("release_name").is_none());
    Ok(())
}

#[test]
fn mbid_match_treats_blank_recording_id_as_no_match() {
    assert!(mbid_match(None, None).is_none());
    assert!(mbid_match(Some("   ".to_owned()), None).is_none());

    let matched = mbid_match(Some("rec-1".to_owned()), Some(String::new()));
    assert_eq!(
        matched.as_ref().map(|m| m.recording_mbid.as_str()),
        Some("rec-1"),
        "a non-empty recording id is a match",
    );
    // A blank release id degrades to None rather than an empty string.
    assert_eq!(matched.and_then(|m| m.release_mbid), None);
}

#[test]
fn align_bulk_results_keys_on_index_not_position() -> TestResult {
    // A response that is reordered and short one entry (the unmatched track is
    // omitted here to prove alignment survives a partial, out-of-order array).
    let results: Vec<BulkLookupResult> = serde_json::from_str(
        r#"[
            {"index": 2, "recording_mbid": "rec-2", "release_mbid": "rel-2"},
            {"index": 0, "recording_mbid": "rec-0"}
        ]"#,
    )?;
    let aligned = align_bulk_results(3, results);

    assert_eq!(aligned.len(), 3);
    assert_eq!(aligned[0].as_ref().map(|m| m.recording_mbid.as_str()), Some("rec-0"));
    assert!(aligned[1].is_none()); // never returned by the server
    assert_eq!(aligned[2].as_ref().map(|m| m.recording_mbid.as_str()), Some("rec-2"));
    assert_eq!(aligned[2].as_ref().and_then(|m| m.release_mbid.as_deref()), Some("rel-2"),);
    Ok(())
}
