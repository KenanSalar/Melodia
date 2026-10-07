//! Two halves. The payload builders are pure and are driven directly; the entry points below them
//! go over the wire, against a local server every one of them already takes the base URL for.
//!
//! What the wire half is for is the reading of a response rather than the writing of a request:
//! which statuses are answers, which are errors, and which of the errors the submitter is
//! supposed to come back from. The payloads are pinned above and are not re-asserted through a
//! socket.

use melodia_testkit::http::{TestResponse, TestServer};

use super::{
    ListenBrainzError, feedback_payload, listens_payload, playing_now_payload, submit_playing_now,
    validate_token,
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
