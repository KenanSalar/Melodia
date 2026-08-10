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

/// The `error` object Deezer sends **in place of** a body, at HTTP 200.
#[derive(serde::Deserialize)]
struct DeezerErrorBody {
    error: DeezerApiError,
}

#[derive(serde::Deserialize)]
struct DeezerApiError {
    message: String,
    code: i64,
}

/// What one search round trip actually returned.
///
/// The second arm exists because Deezer reports a tripped rate limit as **HTTP
/// 200** carrying `{"error": {…}}` where `data` belongs — so decoding straight
/// into the success type reports the API's own refusal as a malformed response,
/// and a caller pacing a batch can't tell "no match" from "stop asking".
pub enum DeezerAnswer<T> {
    /// What the search found, or `None` when nothing matched. A non-success
    /// status folds in here too — every caller reads the two the same way.
    Body(Option<T>),
    /// Deezer answered with its own error. `code` 4 is `Quota limit exceeded`;
    /// a caller running a batch should stop the pass rather than spend the rest
    /// of it on refusals.
    ApiError { message: String, code: i64 },
}

/// Classify a body Deezer answered 200 with, peeling off the API's error object
/// before attempting the success shape.
///
/// The two shapes are disjoint — neither `data` nor `error` has a serde default —
/// so the order costs nothing and only the error arm needs to come first. `what`
/// names the body in the failure message, so a genuine decode failure still says
/// which endpoint it came from.
fn classify<T: serde::de::DeserializeOwned>(
    body: &[u8],
    what: &str,
) -> Result<DeezerAnswer<T>, AppError> {
    if let Ok(refusal) = serde_json::from_slice::<DeezerErrorBody>(body) {
        return Ok(DeezerAnswer::ApiError {
            message: refusal.error.message,
            code: refusal.error.code,
        });
    }

    serde_json::from_slice(body)
        .map(|decoded| DeezerAnswer::Body(Some(decoded)))
        .map_err(|e| AppError::network(format!("Failed to parse {what}"), e))
}

/// Read and classify a search response. A non-success status is `Body(None)`.
async fn decode_search<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<DeezerAnswer<T>, AppError> {
    if !response.status().is_success() {
        return Ok(DeezerAnswer::Body(None));
    }

    let body = response
        .bytes()
        .await
        .map_err(|e| AppError::network(format!("Failed to read {what}"), e))?;

    classify(&body, what)
}

/// Searches the Deezer API for an artist and returns the medium picture URL if found.
pub async fn search_artist_image_url(
    client: &reqwest::Client,
    artist_name: &str,
) -> Result<DeezerAnswer<String>, AppError> {
    let response = client
        .get("https://api.deezer.com/search/artist")
        .query(&[("q", artist_name), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer API request failed", e))?;

    match decode_search::<DeezerSearchResponse>(response, "Deezer response").await? {
        DeezerAnswer::Body(body) => Ok(DeezerAnswer::Body(
            body.and_then(|b| b.data.first().and_then(|a| a.picture_medium.clone())),
        )),
        DeezerAnswer::ApiError { message, code } => Ok(DeezerAnswer::ApiError { message, code }),
    }
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
    let response = client
        .get("https://api.deezer.com/search/album")
        .query(&[("q", query.as_str()), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer album search failed", e))?;

    // An `Option` rather than a `DeezerAnswer`: the caller runs one lookup per
    // track change and already treats every error as non-definitive, so a
    // refusal is only worth naming in the log line.
    match decode_search::<DeezerAlbumSearchResponse>(response, "Deezer album response").await? {
        DeezerAnswer::Body(body) => {
            Ok(body.and_then(|b| b.data.first().and_then(|a| a.cover_big.clone())))
        }
        DeezerAnswer::ApiError { message, code } => Err(AppError::network_msg(format!(
            "Deezer refused the album search: {message} (code {code})"
        ))),
    }
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
