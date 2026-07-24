use super::DeezerAlbumSearchResponse;

fn parse(json: &str) -> DeezerAlbumSearchResponse {
    serde_json::from_str(json).unwrap_or(DeezerAlbumSearchResponse { data: Vec::new() })
}

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
