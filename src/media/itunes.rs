use crate::error::AppError;

#[derive(serde::Deserialize)]
struct ItunesSearchResponse {
    results: Vec<ItunesAlbum>,
}

#[derive(serde::Deserialize)]
struct ItunesAlbum {
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
}

/// Searches the iTunes Search API for an album and returns an upsized cover URL if
/// found. The Discord Rich Presence fallback for when Deezer has no match; like the
/// Deezer path, Discord fetches the URL server-side, so this only passes the string
/// along — no download, no disk cache. Keyless, single GET.
pub async fn search_album_cover(
    client: &reqwest::Client,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    // iTunes has no field-pinning search syntax; a combined term with entity=album is
    // the conventional album lookup.
    let term = format!("{artist} {album}");
    let response: reqwest::Response = client
        .get("https://itunes.apple.com/search")
        .query(&[("term", term.as_str()), ("entity", "album"), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("iTunes album search failed", e))?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let body = response
        .json::<ItunesSearchResponse>()
        .await
        .map_err(|e| AppError::network("Failed to parse iTunes album response", e))?;

    Ok(body
        .results
        .first()
        .and_then(|a| a.artwork_url_100.as_deref().map(upsize_artwork_url)))
}

/// iTunes returns a 100 px thumbnail (`.../100x100bb.jpg`); swap the size token for a
/// ~500 px cover to match Deezer's `cover_big`. Returns the input unchanged when the
/// token isn't present — a small cover still beats none.
fn upsize_artwork_url(url_100: &str) -> String {
    url_100.replace("100x100bb", "512x512bb")
}

#[cfg(test)]
#[path = "tests/itunes_tests.rs"]
mod tests;
