//! The parse and the upsize are pure; what is around them is a status the caller cannot
//! re-interrogate.
//!
//! `discord::artwork::run_lookup` caches an `Ok(None)` as a definitive miss for the rest of the
//! session, so every case below turns on the difference between "this album has no cover" and
//! "nobody answered".

use melodia_testkit::http::{TestResponse, TestServer};

use super::{ItunesSearchResponse, search_album_cover_at, upsize_artwork_url};
use melodia_core::error::AppError;

type TestResult = Result<(), AppError>;

fn parse(json: &str) -> ItunesSearchResponse {
    serde_json::from_str(json).unwrap_or(ItunesSearchResponse {
        results: Vec::new(),
    })
}

async fn cover_from(response: TestResponse) -> Result<Option<String>, AppError> {
    let Ok(server) = TestServer::start(move |_| response.clone()) else {
        unreachable!("a loopback listener on port 0")
    };
    search_album_cover_at(&reqwest::Client::new(), &server.base_url(), "Daft Punk", "Discovery")
        .await
}

#[test]
fn extracts_artwork_url_from_first_result() {
    // Trimmed shape of a real /search?entity=album response — extra fields are ignored.
    let body = parse(
        r#"{
            "resultCount": 1,
            "results": [
                {
                    "wrapperType": "collection",
                    "collectionType": "Album",
                    "artistName": "Daft Punk",
                    "collectionName": "Discovery",
                    "artworkUrl100": "https://is1-ssl.mzstatic.example/image/100x100bb.jpg"
                }
            ]
        }"#,
    );
    let url = body.results.first().and_then(|a| a.artwork_url_100.clone());
    assert_eq!(url.as_deref(), Some("https://is1-ssl.mzstatic.example/image/100x100bb.jpg"));
}

#[test]
fn missing_artwork_url_yields_none() {
    let body = parse(r#"{"resultCount":1,"results":[{"collectionName":"X"}]}"#);
    let url = body.results.first().and_then(|a| a.artwork_url_100.clone());
    assert!(url.is_none());
}

#[test]
fn empty_results_yields_none() {
    let body = parse(r#"{"resultCount":0,"results":[]}"#);
    let url = body.results.first().and_then(|a| a.artwork_url_100.clone());
    assert!(url.is_none());
}

#[test]
fn upsize_swaps_size_token() {
    let upsized = upsize_artwork_url("https://is1-ssl.mzstatic.example/image/100x100bb.jpg");
    assert_eq!(upsized, "https://is1-ssl.mzstatic.example/image/512x512bb.jpg");
}

#[test]
fn upsize_leaves_unmatched_url_unchanged() {
    let url = "https://e-cdn.example/big.jpg";
    assert_eq!(upsize_artwork_url(url), url);
}

// ---- the status, which the caller reads as a verdict about the album ----

/// **A refusal is an `Err`, never `Ok(None)`.** iTunes Search rate-limits with a status, and the
/// caller caches a `None` as "this album has no cover" for the rest of the session — so folding
/// one in blanks that album until restart, on a lookup that would have worked a second later.
#[tokio::test]
async fn a_refused_search_is_an_error_rather_than_a_definitive_miss() -> TestResult {
    let refused = cover_from(TestResponse::status(403)).await;

    let Err(AppError::Network { msg, .. }) = refused else {
        return Err(AppError::Validation(format!(
            "a non-success status must not answer as a miss, got {refused:?}"
        )));
    };
    assert!(msg.contains("403"), "the refusal names the status it got: {msg}");
    Ok(())
}

/// The other half of the same distinction: a search the API *answered* with nothing is a real
/// miss, and caching it is the point.
#[tokio::test]
async fn a_search_the_api_answered_with_nothing_is_a_miss() -> TestResult {
    let found = cover_from(TestResponse::ok(r#"{"resultCount":0,"results":[]}"#)).await?;

    assert_eq!(found, None);
    Ok(())
}

/// The upsize is pinned pure above; what this adds is that the door applies it. Without it the
/// caller gets a 100 px thumbnail where the Deezer path beside it returns 500 px.
#[tokio::test]
async fn the_first_result_is_upsized_on_the_way_out() -> TestResult {
    let found = cover_from(TestResponse::ok(
        r#"{"resultCount":1,"results":[{"artworkUrl100":"https://cdn.example/a/100x100bb.jpg"}]}"#,
    ))
    .await?;

    assert_eq!(found.as_deref(), Some("https://cdn.example/a/512x512bb.jpg"));
    Ok(())
}
