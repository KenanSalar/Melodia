use super::{ItunesSearchResponse, upsize_artwork_url};

fn parse(json: &str) -> ItunesSearchResponse {
    serde_json::from_str(json).unwrap_or(ItunesSearchResponse {
        results: Vec::new(),
    })
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
