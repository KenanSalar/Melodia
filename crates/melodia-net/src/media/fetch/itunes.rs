use melodia_core::error::AppError;

/// Slack against a schema change: the query asks for one result and reads one field.
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;

/// A constant so the fetch can take its URL as an argument and a test can serve one.
const SEARCH_URL: &str = "https://itunes.apple.com/search";

#[derive(serde::Deserialize)]
struct ItunesSearchResponse {
    results: Vec<ItunesAlbum>,
}

#[derive(serde::Deserialize)]
struct ItunesAlbum {
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
}

/// An upsized album cover URL, Discord's fallback when Deezer has no match. Discord fetches it
/// server-side, so nothing is downloaded here.
pub async fn search_album_cover(
    client: &reqwest::Client,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    search_album_cover_at(client, SEARCH_URL, artist, album).await
}

async fn search_album_cover_at(
    client: &reqwest::Client,
    url: &str,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    // No field syntax here; a combined term with `entity=album` is the usual album lookup.
    let term = format!("{artist} {album}");
    let response: reqwest::Response = client
        .get(url)
        .query(&[("term", term.as_str()), ("entity", "album"), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("iTunes album search failed", e))?;

    // An `Err`, never `Ok(None)`: `discord::artwork::run_lookup` caches `Ok(None)` as a definitive
    // miss for the session, and iTunes rate-limits with a status.
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!("iTunes album search returned HTTP {status}")));
    }

    let bytes =
        crate::services::net::read_capped(response, "iTunes album response", MAX_RESPONSE_BYTES)
            .await?;
    let body: ItunesSearchResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::network("Failed to parse iTunes album response", e))?;

    Ok(body.results.first().and_then(|a| a.artwork_url_100.as_deref().map(upsize_artwork_url)))
}

/// The 100 px size token swapped for a cover about Deezer's `cover_big`. Unchanged where the
/// token is missing, a small cover still beating none.
fn upsize_artwork_url(url_100: &str) -> String {
    url_100.replace("100x100bb", "512x512bb")
}

#[cfg(test)]
#[path = "tests/itunes_tests.rs"]
mod tests;
