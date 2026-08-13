use std::borrow::Cow;
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
/// Three arms rather than an `Option`, because "nothing matched" and "we never
/// answered" are different facts and a caller that memoizes misses must not
/// memoize the second. Deezer reports a tripped rate limit as **HTTP 200**
/// carrying `{"error": {…}}` where `data` belongs — so decoding straight into the
/// success type reports the API's own refusal as a malformed response, and a
/// caller pacing a batch can't tell "no match" from "stop asking".
pub enum DeezerAnswer<T> {
    /// Deezer answered, and this is what it found — `None` when nothing matched.
    Body(Option<T>),
    /// A non-success HTTP status. Says nothing about whether the thing searched
    /// for exists, so it is not a miss.
    HttpStatus(reqwest::StatusCode),
    /// Deezer answered 200 with its own error object. See [`halts_a_batch`] for
    /// which codes are about the caller's pace rather than about the query.
    ApiError { message: String, code: i64 },
}

impl<T> DeezerAnswer<T> {
    /// Re-key a decoded body onto what the caller actually wanted, leaving the two
    /// non-body arms alone. The generic parameter changes across that step, so
    /// without this every call site re-spells both of them to say nothing.
    fn map_body<U>(self, extract: impl FnOnce(T) -> Option<U>) -> DeezerAnswer<U> {
        match self {
            Self::Body(body) => DeezerAnswer::Body(body.and_then(extract)),
            Self::HttpStatus(status) => DeezerAnswer::HttpStatus(status),
            Self::ApiError { message, code } => DeezerAnswer::ApiError { message, code },
        }
    }
}

/// Whether a Deezer error code is about the *caller's* pace rather than about the
/// query that asked.
///
/// `4` is `Quota limit exceeded` and `700` a busy service: a batch queued behind
/// either would spend itself on refusals, so a caller pacing one should stop.
/// Every other code answers the request it was sent — a malformed advanced-search
/// string (`600`), a bad or missing parameter (`500`/`501`), no data (`800`) — and
/// says nothing about the next search. Stopping on those lets one unlucky name cap
/// how far a pass ever gets, and permanently, since a refusal is never memoized:
/// the next pass re-reaches the same name and stops in the same place.
pub const fn halts_a_batch(code: i64) -> bool {
    matches!(code, 4 | 700)
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

/// Read and classify a search response.
async fn decode_search<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<DeezerAnswer<T>, AppError> {
    let status = response.status();
    if !status.is_success() {
        return Ok(DeezerAnswer::HttpStatus(status));
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

    Ok(decode_search::<DeezerSearchResponse>(response, "Deezer response")
        .await?
        .map_body(|body| body.data.first().and_then(|a| a.picture_medium.clone())))
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
    let query = format!("artist:\"{}\" album:\"{}\"", quotable(artist), quotable(album));
    let response = client
        .get("https://api.deezer.com/search/album")
        .query(&[("q", query.as_str()), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer album search failed", e))?;

    // An `Option` rather than a `DeezerAnswer`: the caller runs one lookup per
    // track change and reads any `Err` as non-definitive, which is what both
    // non-body arms need. A `None` there would be cached as "this album has no
    // cover" for the rest of the session, and neither arm says that.
    match decode_search::<DeezerAlbumSearchResponse>(response, "Deezer album response").await? {
        DeezerAnswer::Body(body) => {
            Ok(body.and_then(|b| b.data.first().and_then(|a| a.cover_big.clone())))
        }
        DeezerAnswer::HttpStatus(status) => {
            Err(AppError::network_msg(format!("Deezer album search returned HTTP {status}")))
        }
        DeezerAnswer::ApiError { message, code } => Err(AppError::network_msg(format!(
            "Deezer refused the album search: {message} (code {code})"
        ))),
    }
}

/// Strip the one character Deezer's advanced-search syntax can't carry inside a
/// field.
///
/// Each field is delimited by double quotes and the syntax documents no escape,
/// so an embedded one closes the field early and the whole query comes back as
/// `InvalidQueryException` (code 600). Dropping it costs a character of precision
/// on the handful of titles that contain one and keeps the query well-formed.
///
/// **The artist search deliberately doesn't route through here**, and reads like
/// an oversight rather than a decision — it sends a plain `q=<name>` with no field
/// syntax at all, so a quote there is ordinary free text, and stripping it would
/// search for a name nobody has.
fn quotable(value: &str) -> Cow<'_, str> {
    if value.contains('"') {
        return Cow::Owned(value.replace('"', ""));
    }
    Cow::Borrowed(value)
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
    let parsed =
        reqwest::Url::parse(image_url).map_err(|e| AppError::network("Invalid image URL", e))?;
    if parsed.scheme() != "https" {
        return Err(AppError::network_msg("Image URL must use HTTPS"));
    }
    let host = parsed.host_str().unwrap_or("");
    if !host.ends_with(".deezer.com") && !host.ends_with(".dzcdn.net") {
        return Err(AppError::network_msg(format!("Image URL has untrusted domain: {host}")));
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

    let bytes =
        response.bytes().await.map_err(|e| AppError::network("Failed to read image bytes", e))?;

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
        std::fs::write(&file_path, &bytes).map_err(AppError::Io)?;
    }

    Ok(Some(file_path.to_string_lossy().into_owned()))
}

#[cfg(test)]
#[path = "tests/deezer_tests.rs"]
mod tests;
