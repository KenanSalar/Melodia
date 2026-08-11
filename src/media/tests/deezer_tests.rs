use super::{
    DeezerAlbumSearchResponse, DeezerAnswer, DeezerSearchResponse, classify, halts_a_batch,
    quotable,
};

fn parse(json: &str) -> DeezerAlbumSearchResponse {
    serde_json::from_str(json).unwrap_or(DeezerAlbumSearchResponse { data: Vec::new() })
}

/// Verbatim body from a tripped quota — HTTP **200**, `error` where `data` belongs.
const QUOTA_BODY: &str = r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#;

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
        message
            .as_deref()
            .is_some_and(|m| m.contains("Failed to parse Deezer response")),
        "unexpected outcome: {message:?}"
    );
}
