//! The primitives every outbound fetch shares, and the directory clients built on them.
//!
//! Two rules, both violable from any file and neither visible when broken, so
//! `crates/melodia/tests/net_primitives.rs` walks the corpus for them: a URL from outside the app
//! is **parsed** rather than prefix-tested ([`http_url`]), and a body is **streamed under a cap**
//! rather than collected and measured afterwards ([`read_capped`]).

pub mod autoeq;
pub mod lyrics_directory;
pub mod pacer;
pub mod radio_blocklist;
pub mod radio_browser;

use std::time::Duration;

use melodia_core::error::{AppError, describe};
use reqwest::StatusCode;
use reqwest::header::{ETAG, HeaderValue, IF_NONE_MATCH};

/// The process-wide shared client, built on the first real request so rustls and the pool load
/// only then. The deadline is **per read**, not whole-body: a slow download may take minutes, but
/// no single read should sit silent that long. The build is documented infallible for these
/// options, hence a logged fallback rather than an error.
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_mins(1))
        .pool_max_idle_per_host(4)
        .user_agent(concat!("Melodia/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|e| {
            log::warn!(
                "reqwest::Client::builder().build() failed unexpectedly ({}); falling back to \
                 default client without timeouts — downloads may hang on a wedged socket",
                describe(&e)
            );
            reqwest::Client::new()
        })
}

/// `candidate` as an absolute `http`/`https` URL naming a host, or `None`. **The parse is the
/// check**: a prefix test admits a bare `http://`, which names no host, and has to remember case.
/// Everything taking a URL from outside the app goes through here.
pub fn http_url(candidate: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(candidate.trim()).ok()?;
    is_http(&parsed).then_some(parsed)
}

pub fn is_http_url(candidate: &str) -> bool {
    http_url(candidate).is_some()
}

/// The rule for a URL already parsed: `Url::join` hands an absolute `file:` or `data:` URL back
/// unchanged, so a joined playlist line has to be checked after the parse too.
pub fn is_http(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.has_host()
}

/// One kind of body a capped GET reads.
pub struct CappedGet {
    /// A capitalized noun phrase naming the body in every error, so a refusal points at the right
    /// half of a two-request fetch.
    pub what: &'static str,
    /// The whole request's, body included, where the client's own deadline is per read.
    pub timeout: Duration,
    pub max_bytes: u64,
}

/// GET `url` and read at most `spec.max_bytes` of it, a non-success status refused before the body.
pub async fn get_capped(
    client: &reqwest::Client,
    url: &reqwest::Url,
    spec: &CappedGet,
) -> Result<Vec<u8>, AppError> {
    let response = send(client.get(url.clone()), spec).await?;
    read_success(response, spec).await
}

/// [`get_capped`] for text, lossy so one Latin-1 byte in a title costs a replacement character, not
/// the station.
pub async fn get_capped_text(
    client: &reqwest::Client,
    url: &reqwest::Url,
    spec: &CappedGet,
) -> Result<String, AppError> {
    Ok(String::from_utf8_lossy_owned(get_capped(client, url, spec).await?))
}

/// What [`get_capped_if_changed`] found.
#[derive(Debug)]
pub enum IfChanged {
    NotModified,
    Fresh { body: Vec<u8>, etag: Option<String> },
}

/// [`get_capped`], sending `If-None-Match` when `cached_etag` holds a tag, so an unchanged body
/// costs an empty `304`.
pub async fn get_capped_if_changed(
    client: &reqwest::Client,
    url: &reqwest::Url,
    cached_etag: Option<&str>,
    spec: &CappedGet,
) -> Result<IfChanged, AppError> {
    let mut request = client.get(url.clone());
    if let Some(tag) = cached_etag
        && !tag.is_empty()
        && let Ok(value) = HeaderValue::from_str(tag)
    {
        request = request.header(IF_NONE_MATCH, value);
    }
    let response = send(request, spec).await?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(IfChanged::NotModified);
    }
    let etag = response.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body = read_success(response, spec).await?;
    Ok(IfChanged::Fresh { body, etag })
}

async fn send(
    request: reqwest::RequestBuilder,
    spec: &CappedGet,
) -> Result<reqwest::Response, AppError> {
    request
        .timeout(spec.timeout)
        .send()
        .await
        .map_err(|e| AppError::network(format!("{} could not be fetched", spec.what), e))
}

/// Refuses a non-success status or an oversized `Content-Length` before a byte is read. The header
/// is a courtesy a host can omit or lie about, so [`read_capped`] still bounds the stream.
async fn read_success(response: reqwest::Response, spec: &CappedGet) -> Result<Vec<u8>, AppError> {
    let what = spec.what;
    if !response.status().is_success() {
        return Err(AppError::network_msg(format!(
            "{what} request returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let max_bytes = spec.max_bytes;
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(AppError::network_msg(format!("{what} is larger than {max_bytes} bytes")));
    }
    read_capped(response, what, max_bytes).await
}

/// Ceiling on the capacity a `Content-Length` may reserve up front: enough to skip the small
/// reallocations, low enough that a host overstating its length can't claim the whole cap.
const READ_HINT_MAX_BYTES: u64 = 64 * 1024;

/// Reads at most `max_bytes` of `response`, refusing as soon as the body crosses the cap.
///
/// **Every response body in the tree is read here**, the updater's streamed-to-disk download
/// aside. A cap checked after `bytes()` has already allocated whatever was sent, and a
/// `.json::<T>()` is no exemption: it allocates the whole body before serde sees a byte.
pub async fn read_capped(
    response: reqwest::Response,
    what: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    use futures_util::StreamExt;

    // The header the cap distrusts is still a fine allocation hint, clamped because it's a claim.
    let hint = response.content_length().unwrap_or(0).min(max_bytes).min(READ_HINT_MAX_BYTES);
    let mut body = Vec::with_capacity(usize::try_from(hint).unwrap_or(0));
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| AppError::network(format!("{what} could not be read"), e))?;
        if body.len().saturating_add(chunk.len()) as u64 > max_bytes {
            return Err(AppError::network_msg(format!("{what} is larger than {max_bytes} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
