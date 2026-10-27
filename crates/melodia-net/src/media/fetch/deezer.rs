use std::borrow::Cow;

use melodia_core::error::AppError;

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

/// What one search round trip returned. Three arms because "nothing matched" and "never
/// answered" differ to a caller that memoizes misses, and Deezer reports a tripped rate limit as
/// HTTP 200 with `{"error": …}` where `data` belongs.
pub enum DeezerAnswer<T> {
    /// `None` when nothing matched.
    Body(Option<T>),
    /// Says nothing about whether the thing searched for exists, so it is not a miss.
    HttpStatus(reqwest::StatusCode),
    /// See [`halts_a_batch`] for which codes are about the caller's pace.
    ApiError { message: String, code: i64 },
}

impl<T> DeezerAnswer<T> {
    /// Re-keys a decoded body, leaving the two non-body arms alone.
    fn map_body<U>(self, extract: impl FnOnce(T) -> Option<U>) -> DeezerAnswer<U> {
        match self {
            Self::Body(body) => DeezerAnswer::Body(body.and_then(extract)),
            Self::HttpStatus(status) => DeezerAnswer::HttpStatus(status),
            Self::ApiError { message, code } => DeezerAnswer::ApiError { message, code },
        }
    }
}

/// Whether a Deezer error code is about the *caller's* pace: `4` is the quota and `700` a busy
/// service. Every other code answers its own request (`600` a malformed query, `500`/`501` a bad
/// parameter, `800` no data), and stopping on those would let one unlucky name cap every pass,
/// a refusal never being memoized.
pub const fn halts_a_batch(code: i64) -> bool {
    matches!(code, 4 | 700)
}

/// Peels off the API's error object before trying the success shape; the two are disjoint, so
/// only the order matters. `what` names the endpoint in a decode failure.
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

/// Slack rather than a budget: a page of results is a few kilobytes and only the first is read.
const MAX_SEARCH_BYTES: u64 = 512 * 1024;

/// A constant so each search can take its URL as an argument and a test can serve one. The image
/// download validates the URL the API handed back instead.
const API_BASE: &str = "https://api.deezer.com";

async fn decode_search<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<DeezerAnswer<T>, AppError> {
    let status = response.status();
    if !status.is_success() {
        return Ok(DeezerAnswer::HttpStatus(status));
    }

    let body = crate::services::net::read_capped(response, what, MAX_SEARCH_BYTES).await?;

    classify(&body, what)
}

/// The medium picture URL for an artist.
pub async fn search_artist_image_url(
    client: &reqwest::Client,
    artist_name: &str,
) -> Result<DeezerAnswer<String>, AppError> {
    search_artist_image_url_at(client, &format!("{API_BASE}/search/artist"), artist_name).await
}

async fn search_artist_image_url_at(
    client: &reqwest::Client,
    url: &str,
    artist_name: &str,
) -> Result<DeezerAnswer<String>, AppError> {
    let response = client
        .get(url)
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

/// The 500×500 `cover_big` URL for an album, for Discord Rich Presence, which fetches it
/// server-side, so nothing is downloaded here.
pub async fn search_album_cover(
    client: &reqwest::Client,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    search_album_cover_at(client, &format!("{API_BASE}/search/album"), artist, album).await
}

async fn search_album_cover_at(
    client: &reqwest::Client,
    url: &str,
    artist: &str,
    album: &str,
) -> Result<Option<String>, AppError> {
    // Deezer advanced-search syntax pins both fields, tighter than title alone.
    let query = format!("artist:\"{}\" album:\"{}\"", quotable(artist), quotable(album));
    let response = client
        .get(url)
        .query(&[("q", query.as_str()), ("limit", "1")])
        .send()
        .await
        .map_err(|e| AppError::network("Deezer album search failed", e))?;

    // `Err` for both non-body arms: the caller caches a `None` as "no cover" for the session,
    // which neither of them means.
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

/// Drops the double quote, which closes an advanced-search field early (code 600) and has no
/// escape. The artist search deliberately doesn't route through here: it sends a plain
/// `q=<name>`, where a quote is ordinary text.
fn quotable(value: &str) -> Cow<'_, str> {
    if value.contains('"') {
        return Cow::Owned(value.replace('"', ""));
    }
    Cow::Borrowed(value)
}

const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Downloads an artist image the search handed back, HTTPS on a Deezer CDN host only.
pub async fn download_artist_image(
    client: &reqwest::Client,
    image_url: &str,
) -> Result<Vec<u8>, AppError> {
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

    if let Some(len) = response.content_length()
        && len > MAX_IMAGE_BYTES
    {
        return Err(AppError::network_msg(format!(
            "Image too large: {len} bytes (max {MAX_IMAGE_BYTES})"
        )));
    }

    // Still streamed under the cap: the header is only as good as the host.
    crate::services::net::read_capped(response, "Artist image", MAX_IMAGE_BYTES).await
}

#[cfg(test)]
#[path = "tests/deezer_tests.rs"]
mod tests;
