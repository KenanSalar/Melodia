use std::path::Path;

use melodia_testkit::http::{TestResponse, TestServer};

use super::{
    DeezerAlbumSearchResponse, DeezerAnswer, DeezerSearchResponse, classify,
    download_and_cache_artist_image, halts_a_batch, quotable, search_album_cover_at,
    search_artist_image_url_at,
};
use melodia_core::error::AppError;

type TestResult = Result<(), AppError>;

fn parse(json: &str) -> DeezerAlbumSearchResponse {
    serde_json::from_str(json).unwrap_or(DeezerAlbumSearchResponse { data: Vec::new() })
}

async fn artist_image_from(response: TestResponse) -> Result<DeezerAnswer<String>, AppError> {
    let Ok(server) = TestServer::start(move |_| response.clone()) else {
        unreachable!("a loopback listener on port 0")
    };
    search_artist_image_url_at(&reqwest::Client::new(), &server.base_url(), "Daft Punk").await
}

async fn album_cover_from(response: TestResponse) -> Result<Option<String>, AppError> {
    let Ok(server) = TestServer::start(move |_| response.clone()) else {
        unreachable!("a loopback listener on port 0")
    };
    search_album_cover_at(&reqwest::Client::new(), &server.base_url(), "Daft Punk", "Discovery")
        .await
}

/// Verbatim body from a tripped quota — HTTP **200**, `error` where `data` belongs.
const QUOTA_BODY: &str =
    r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#;

#[test]
fn extracts_cover_big_from_first_result() {
    // Trimmed shape of a real /search/album response — extra fields are ignored.
    let body = parse(
        r#"{
            "data": [
                {
                    "id": 302127,
                    "title": "Discovery",
                    "cover_medium": "https://e-cdn.example/medium.jpg",
                    "cover_big": "https://e-cdn.example/big.jpg"
                }
            ],
            "total": 1
        }"#,
    );
    let url = body.data.first().and_then(|a| a.cover_big.clone());
    assert_eq!(url.as_deref(), Some("https://e-cdn.example/big.jpg"));
}

#[test]
fn missing_cover_big_yields_none() {
    let body = parse(r#"{"data":[{"id":1,"title":"X"}],"total":1}"#);
    let url = body.data.first().and_then(|a| a.cover_big.clone());
    assert!(url.is_none());
}

#[test]
fn empty_data_yields_none() {
    let body = parse(r#"{"data":[],"total":0}"#);
    let url = body.data.first().and_then(|a| a.cover_big.clone());
    assert!(url.is_none());
}

#[test]
fn a_quota_answer_is_named_rather_than_reported_as_a_parse_failure() {
    let named = match classify::<DeezerSearchResponse>(QUOTA_BODY.as_bytes(), "Deezer response") {
        Ok(DeezerAnswer::ApiError { message, code }) => Some((message, code)),
        _ => None,
    };
    assert_eq!(named, Some(("Quota limit exceeded".to_owned(), 4)));
}

/// The reason [`classify`] can try the error shape first and lose nothing: the two
/// are disjoint, because neither `data` nor `error` carries a serde default. Adding
/// one to `data` would turn every refusal into an empty result set — memoized as a
/// definitive "no match" for the rest of the session.
#[test]
fn a_quota_body_does_not_decode_as_an_empty_result_set() {
    assert!(serde_json::from_str::<DeezerSearchResponse>(QUOTA_BODY).is_err());
    assert!(serde_json::from_str::<DeezerAlbumSearchResponse>(QUOTA_BODY).is_err());
}

#[test]
fn a_search_body_still_decodes_past_the_error_arm() {
    let body = r#"{"data":[{"id":1,"picture_medium":"https://e-cdn.example/a.jpg"}],"total":1}"#;
    let url = match classify::<DeezerSearchResponse>(body.as_bytes(), "Deezer response") {
        Ok(DeezerAnswer::Body(Some(decoded))) => {
            decoded.data.first().and_then(|a| a.picture_medium.clone())
        }
        _ => None,
    };
    assert_eq!(url.as_deref(), Some("https://e-cdn.example/a.jpg"));
}

/// The artist-image pass abandons every remaining batch on a halting code, and a
/// refusal is never memoized — so a code that halts on the wrong grounds lets one
/// unlucky artist name stop the pass at the same place on every scan, forever.
/// `600` is the live risk: the album search builds an advanced-search string, and
/// Deezer answers a malformed one with `InvalidQueryException`.
#[test]
fn only_the_codes_about_our_own_pace_halt_a_batch() {
    assert!(halts_a_batch(4), "quota — the batch behind it would refuse too");
    assert!(halts_a_batch(700), "service busy — same");

    for answered_the_query in [500, 501, 600, 800] {
        assert!(
            !halts_a_batch(answered_the_query),
            "code {answered_the_query} is about the query that asked, not the next one"
        );
    }
}

/// Deezer's advanced-search fields are quote-delimited with no documented escape,
/// so an embedded quote closes the field early and the whole query comes back as
/// code 600 — which [`halts_a_batch`] deliberately does not stop on, leaving a
/// silent per-album miss instead.
#[test]
fn an_embedded_quote_cannot_close_a_search_field_early() {
    assert_eq!(quotable(r#"Rock 'n' "Roll""#), "Rock 'n' Roll");
    assert!(matches!(quotable("Discovery"), std::borrow::Cow::Borrowed(_)));
}

#[test]
fn a_body_of_neither_shape_is_still_a_parse_failure() {
    let message =
        classify::<DeezerSearchResponse>(b"<html>gateway timeout</html>", "Deezer response")
            .err()
            .map(|e| e.to_string());
    assert!(
        message.as_deref().is_some_and(|m| m.contains("Failed to parse Deezer response")),
        "unexpected outcome: {message:?}"
    );
}

// ---- the wire the classifier is fed from ----

/// `classify` is pinned above over bytes and `map_body` over the arms; what neither says is that
/// the artist search feeds them the response at all. The composition is the claim here.
#[tokio::test]
async fn an_artist_search_answers_with_the_first_result_picture() -> TestResult {
    let answered = artist_image_from(TestResponse::ok(
        r#"{"data":[{"picture_medium":"https://cdn.example/a/medium.jpg"},
                    {"picture_medium":"https://cdn.example/b/medium.jpg"}]}"#,
    ))
    .await?;

    let DeezerAnswer::Body(picture) = answered else {
        return Err(AppError::Validation(
            "a decoded search must answer on the body arm".to_owned(),
        ));
    };
    assert_eq!(picture.as_deref(), Some("https://cdn.example/a/medium.jpg"));
    Ok(())
}

/// A tripped quota arrives as **HTTP 200** with `error` where `data` belongs, so this is the case
/// that says the search reaches the error arm rather than reporting a malformed response — which
/// is what a caller pacing a batch reads [`halts_a_batch`] off.
#[tokio::test]
async fn a_quota_answer_reaches_the_batch_as_the_code_that_halts_it() -> TestResult {
    let answered = artist_image_from(TestResponse::ok(QUOTA_BODY)).await?;

    let DeezerAnswer::ApiError { code, .. } = answered else {
        return Err(AppError::Validation("a 200 carrying `error` is not a body".to_owned()));
    };
    assert!(halts_a_batch(code), "code {code} came back off the wire and must stop the pass");
    Ok(())
}

/// The status arm, which is not a miss: it says nothing about whether the artist exists, so a
/// caller memoizing misses must never fold it in.
#[tokio::test]
async fn a_refused_artist_search_is_a_status_rather_than_a_miss() -> TestResult {
    let answered = artist_image_from(TestResponse::status(503)).await?;

    assert!(matches!(answered, DeezerAnswer::HttpStatus(_)), "a 503 is not an empty result set");
    Ok(())
}

/// The album door folds both non-body arms into an `Err` where the artist door hands them back,
/// and for the reason iTunes' sibling case gives: its caller caches an `Ok(None)` as "this album
/// has no cover" for the rest of the session, and neither arm says that.
#[tokio::test]
async fn a_refused_album_search_is_an_error_rather_than_a_definitive_miss() -> TestResult {
    let refused = album_cover_from(TestResponse::status(503)).await;

    assert!(matches!(refused, Err(AppError::Network { .. })), "{refused:?}");

    let over_quota = album_cover_from(TestResponse::ok(QUOTA_BODY)).await;

    assert!(
        matches!(over_quota, Err(AppError::Network { .. })),
        "a 200 carrying an error object is not a cover-less album: {over_quota:?}"
    );
    Ok(())
}

// ---- the image download, which refuses before it opens a socket ----

/// The allowlist is what stops a URL off a compromised or merely wrong search result being fetched
/// and written into the artist store. Both refusals fire ahead of the request, which is why they
/// need no server — and why a loopback case for the download is not written at all: admitting one
/// means relaxing the guard under test.
///
/// **On the refusal's own wording, not on `Err`.** A host that got past the guard fails the fetch
/// anyway, on a name that does not resolve, and reports the same variant — so a test matching only
/// `AppError::Network` passes with the allowlist deleted. Confirmed by mutation.
#[tokio::test]
async fn an_image_url_off_a_host_deezer_does_not_own_is_refused() -> TestResult {
    let client = reqwest::Client::new();

    for elsewhere in [
        "https://cdn.example/a.jpg",
        // Both spellings of "ends with something that looks like the real host".
        "https://deezer.com.example/a.jpg",
        "https://notdzcdn.net/a.jpg",
    ] {
        let refused = download_and_cache_artist_image(&client, elsewhere, Path::new("/")).await;

        let Err(AppError::Network { msg, .. }) = refused else {
            return Err(AppError::Validation(format!("{elsewhere} was accepted")));
        };
        assert!(msg.contains("untrusted domain"), "{elsewhere} was refused for {msg} instead");
    }
    Ok(())
}

#[tokio::test]
async fn an_image_url_that_is_not_https_is_refused() -> TestResult {
    let refused = download_and_cache_artist_image(
        &reqwest::Client::new(),
        "http://e-cdns-images.dzcdn.net/a.jpg",
        Path::new("/"),
    )
    .await;

    let Err(AppError::Network { msg, .. }) = refused else {
        return Err(AppError::Validation(format!("a plaintext URL was accepted: {refused:?}")));
    };
    assert!(msg.contains("HTTPS"), "refused for {msg} rather than for its scheme");
    Ok(())
}
