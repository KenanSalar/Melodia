//! Two halves. The payload builders and the alignment are pure and are driven directly; the four
//! entry points below them go over the wire, against a local server every one of them already
//! takes the base URL for.
//!
//! What the wire half is for is the reading of a response rather than the writing of a request:
//! which statuses are answers, which are errors, and which of the errors the submitter is
//! supposed to come back from. The payloads are pinned above and are not re-asserted through a
//! socket.

use melodia_testkit::http::{TestResponse, TestServer};

use super::{
    BulkLookupResult, ListenBrainzError, LookupQuery, align_bulk_results, bulk_lookup_payload,
    feedback_payload, listens_payload, lookup_recording_mbids_bulk, mbid_match,
    playing_now_payload, submit_playing_now, validate_token,
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

/// The account a token belongs to is what the Settings row shows once a connection succeeds, and
/// the header is the whole of the authentication: there is no app registration to fall back on.
#[tokio::test]
async fn a_valid_token_comes_back_with_the_name_it_belongs_to() -> TestResult {
    let server =
        TestServer::start(|_| TestResponse::ok(r#"{"valid": true, "user_name": "listener"}"#))?;
    let client = reqwest::Client::new();

    let validated = validate_token(&client, &server.base_url(), "tok").await?;

    assert!(validated.valid);
    assert_eq!(validated.user_name.as_deref(), Some("listener"));
    let requests = server.requests();
    let [request] = requests.as_slice() else {
        return Err(format!("expected one request, got {}", requests.len()).into());
    };
    assert_eq!(request.path, "/1/validate-token");
    assert_eq!(request.header("authorization"), Some("Token tok"));
    Ok(())
}

/// A rejected token is a verdict rather than a failure, so the dialog can say the token is wrong
/// instead of that something went wrong.
#[tokio::test]
async fn a_rejected_token_is_an_answer_rather_than_an_error() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(401))?;
    let client = reqwest::Client::new();

    let validated = validate_token(&client, &server.base_url(), "tok").await?;

    assert!(!validated.valid);
    assert_eq!(validated.user_name, None);
    Ok(())
}

/// A server that is down is not a verdict at all. Widening the 401 arm by one status tells a user
/// their token is bad on the day `ListenBrainz` has an outage, and they replace a working one.
#[tokio::test]
async fn a_failing_server_is_not_a_verdict_on_the_token() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(503).body("upstream unavailable"))?;
    let client = reqwest::Client::new();

    match validate_token(&client, &server.base_url(), "tok").await {
        Err(ListenBrainzError::Server { status, message }) => {
            assert_eq!(status, 503);
            assert_eq!(message, "upstream unavailable", "the body is what a bug report carries");
        }
        other => return Err(format!("expected a server error, got {other:?}").into()),
    }
    Ok(())
}

/// Now-playing is ephemeral, never queued and never retried, so nothing downstream compensates for
/// it arriving at the wrong endpoint or unauthenticated.
#[tokio::test]
async fn playing_now_posts_to_the_listens_endpoint_under_the_users_token() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("{}"))?;
    let client = reqwest::Client::new();

    submit_playing_now(&client, &server.base_url(), "tok", &make_track()).await?;

    let requests = server.requests();
    let [request] = requests.as_slice() else {
        return Err(format!("expected one request, got {}", requests.len()).into());
    };
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/1/submit-listens");
    assert_eq!(request.header("authorization"), Some("Token tok"));
    Ok(())
}

/// The backfill chunks its library, so an empty chunk is routine. Asking the server about nothing
/// spends a rate-limit slot the next real chunk needs.
#[tokio::test]
async fn an_empty_lookup_batch_asks_the_server_nothing() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("[]"))?;
    let client = reqwest::Client::new();

    let matched = lookup_recording_mbids_bulk(&client, &server.base_url(), "tok", &[]).await?;

    assert!(matched.is_empty());
    assert!(server.requests().is_empty(), "no queries, no request");
    Ok(())
}

/// The lookup parses its body on success and so cannot share `classify_response`, which is the
/// whole reason `error_for` was split out. Reading a 429 there as a plain server error costs the
/// chunk: the backfill's table retries a throttle and retires a server error, permanently.
#[tokio::test]
async fn a_rate_limited_lookup_is_throttled_rather_than_a_server_error() -> TestResult {
    let server =
        TestServer::start(|_| TestResponse::status(429).header("X-RateLimit-Reset-In", "42"))?;
    let client = reqwest::Client::new();
    let queries = [LookupQuery {
        artist: "Artist",
        title: "Song",
        release: None,
    }];

    match lookup_recording_mbids_bulk(&client, &server.base_url(), "tok", &queries).await {
        Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
            assert_eq!(reset_in_secs, Some(42), "the window the server named, not a local guess");
        }
        other => return Err(format!("expected a rate limit, got {other:?}").into()),
    }
    Ok(())
}
