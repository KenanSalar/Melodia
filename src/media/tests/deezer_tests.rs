use super::{DeezerAlbumSearchResponse, DeezerAnswer, DeezerSearchResponse, classify};

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
