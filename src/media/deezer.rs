use std::path::Path;

use crate::error::AppError;

#[derive(serde::Deserialize)]
struct DeezerSearchResponse {
    data: Vec<DeezerArtist>,
}

#[derive(serde::Deserialize)]
struct DeezerArtist {
    picture_medium: Option<String>,
}

/// Searches the Deezer API for an artist and returns the medium picture URL if found.
pub async fn search_artist_image_url(
    client: &reqwest::Client,
    artist_name: &str,
) -> Result<Option<String>, AppError> {
    let response: reqwest::Response = client
        .get("https://api.deezer.com/search/artist")
        .query(&[("q", artist_name), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer API request failed", e))?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let body = response
        .json::<DeezerSearchResponse>()
        .await
        .map_err(|e| AppError::network("Failed to parse Deezer response", e))?;

    Ok(body.data.first().and_then(|a| a.picture_medium.clone()))
}

#[derive(serde::Deserialize)]
struct DeezerAlbumSearchResponse {
    data: Vec<DeezerAlbum>,
}

#[derive(serde::Deserialize)]
struct DeezerAlbum {
    cover_big: Option<String>,
}

/// Searches the Deezer API for an album and returns the 500×500 `cover_big` URL
/// if found. Used for Discord Rich Presence, which fetches the URL server-side,
/// so this only passes the string along — no download, no disk cache.
pub async fn search_album_cover(
    client: &reqwest::Client,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    // Deezer advanced-search syntax pins both fields, tighter than title alone.
    let query = format!("artist:\"{artist}\" album:\"{album}\"");
    let response: reqwest::Response = client
        .get("https://api.deezer.com/search/album")
        .query(&[("q", query.as_str()), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer album search failed", e))?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let body = response
        .json::<DeezerAlbumSearchResponse>()
        .await
        .map_err(|e| AppError::network("Failed to parse Deezer album response", e))?;

    Ok(body.data.first().and_then(|a| a.cover_big.clone()))
}

/// Maximum image download size (5 MB).
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Downloads an artist image from a URL and caches it in the artists directory.
/// Returns the absolute path to the cached image, or None if download fails.
///
/// Validates that the URL uses HTTPS and points to a Deezer CDN domain,
/// and enforces a size limit to prevent unbounded memory allocation.
pub async fn download_and_cache_artist_image(
    client: &reqwest::Client,
    image_url: &str,
    artists_dir: &Path,
) -> Result<Option<String>, AppError> {
    // Validate URL scheme and domain
    let parsed = reqwest::Url::parse(image_url)
        .map_err(|e| AppError::network("Invalid image URL", e))?;
    if parsed.scheme() != "https" {
        return Err(AppError::network_msg("Image URL must use HTTPS"));
    }
    let host = parsed.host_str().unwrap_or("");
    if !host.ends_with(".deezer.com") && !host.ends_with(".dzcdn.net") {
        return Err(AppError::network_msg(format!(
            "Image URL has untrusted domain: {host}"
        )));
    }

    let response = client
        .get(image_url)
        .send()
        .await
        .map_err(|e| AppError::network("Failed to download artist image", e))?;

    // Check Content-Length before downloading body
    if let Some(len) = response.content_length()
        && len > MAX_IMAGE_BYTES
    {
        return Err(AppError::network_msg(format!(
            "Image too large: {len} bytes (max {MAX_IMAGE_BYTES})"
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::network("Failed to read image bytes", e))?;

    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(AppError::network_msg(format!(
            "Image too large: {} bytes (max {})",
            bytes.len(),
            MAX_IMAGE_BYTES
        )));
    }

    if bytes.is_empty() {
        return Ok(None);
    }

    // BLAKE3 hash (first 16 hex chars) for dedup filename
    let hash = blake3::hash(&bytes);
    let hash_hex: String = hash.to_hex()[..16].to_string();

    let filename = format!("{hash_hex}.jpg");
    let file_path = artists_dir.join(&filename);

    // Skip write if file already exists (dedup)
    if !file_path.exists() {
        std::fs::write(&file_path, &bytes).map_err(|e| {
            AppError::Io(e)
        })?;
    }

    Ok(Some(file_path.to_string_lossy().into_owned()))
}

#[cfg(test)]
#[path = "tests/deezer_tests.rs"]
mod tests;
