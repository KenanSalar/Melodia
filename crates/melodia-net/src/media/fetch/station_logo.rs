//! One radio station's logo, off a third-party host, as bytes for the store.
//!
//! A station's `favicon_url` points anywhere the station's owner typed, at whatever a browser tab
//! wanted, so the guards here are about the host being unknown: the scheme, the deadline and the
//! byte cap. Whether the image is big enough to draw, and what tile it needs, is the store's
//! (`media::image::logo_tile::store`).
//!
//! Reached only through `library::radio::fetch_logo`, which is where the switch that turns Radio
//! off refuses. Nothing here logs a URL.

use std::time::Duration;

use melodia_core::error::AppError;

/// Ceiling on one logo download, checked against the header and again against the body.
///
/// Smaller than the artist path's: this is a site icon rather than a press photo, and the cap is
/// what bounds a host that answers a favicon request with a hero image.
const MAX_LOGO_BYTES: u64 = 2 * 1024 * 1024;

/// Deadline on one logo request, start to finish.
///
/// The shared client bounds a *read* and a connect, not a request, so a host that trickles bytes
/// never trips either and holds its slot for minutes. `services::net::radio_browser` wraps its own calls
/// for the same reason. Set well under that: a favicon this slow is a miss, and a page's worth of
/// them queue behind each other.
pub(super) const LOGO_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A logo as the host served it, before the store has had a say.
#[derive(Debug)]
pub struct FetchedLogo {
    pub bytes: Vec<u8>,
    /// What the response's type is filed under; see [`extension_for`].
    pub extension: &'static str,
}

/// Download one station logo.
///
/// `Ok(None)` is a host that answered with something other than an image; `Err` is a failure
/// worth retrying later.
pub async fn fetch(
    client: &reqwest::Client,
    favicon_url: &str,
) -> Result<Option<FetchedLogo>, AppError> {
    let parsed = fetchable_url(favicon_url)?;

    let response = client
        .get(parsed)
        .timeout(LOGO_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network("Station logo download failed", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!("Station logo returned HTTP {status}")));
    }

    let Some(extension) = stored_extension(&response) else {
        return Ok(None);
    };
    // Ahead of the body, so an oversized host costs a header rather than a transfer. Checked
    // again below because the field is optional and a server may simply be wrong about it.
    if let Some(len) = response.content_length()
        && len > MAX_LOGO_BYTES
    {
        return Err(AppError::network_msg(format!(
            "Station logo too large: {len} bytes (max {MAX_LOGO_BYTES})"
        )));
    }

    let bytes =
        crate::services::net::read_capped(response, "Station logo body", MAX_LOGO_BYTES).await?;
    Ok(Some(FetchedLogo { bytes, extension }))
}

/// The URL to fetch, or a refusal.
///
/// HTTP as well as HTTPS, and nothing else: a `file://` or `data:` URL is not a fetch to make on a
/// directory row's say-so. Cleartext is admitted because refusing it cost real logos and bought
/// little: no credential is sent, and what comes back is only ever bytes the store decodes as an
/// image, bounds and re-encodes.
pub(super) fn fetchable_url(favicon_url: &str) -> Result<reqwest::Url, AppError> {
    crate::services::net::http_url(favicon_url).ok_or_else(|| {
        AppError::network_msg("Station logo URL must be an http:// or https:// address")
    })
}

/// What the response says it is, folded through [`extension_for`].
fn stored_extension(response: &reqwest::Response) -> Option<&'static str> {
    let header = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    extension_for(header)
}

/// The extension to file the bytes under, or `None` for a response the store cannot hold.
///
/// **`.ico` is the one worth naming.** Admitting it widened `artwork::STORED_EXTENSIONS`, and the
/// `image` feature list that has to stay a superset of it, for a container only radio ever sees.
/// It earns that because a station's logo field is a favicon field: refusing the format refused
/// stations whose only listed image was one.
///
/// The extension only names the file: every reader in the tree sniffs content, and `store_image`
/// re-encodes anything over its bounds to JPEG regardless. So an unrecognised image type is
/// admitted as JPEG rather than refused, and it is the header parse there that has the last word.
fn extension_for(header: &str) -> Option<&'static str> {
    let content_type = header.split(';').next().unwrap_or(header).trim().to_ascii_lowercase();

    match content_type.as_str() {
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" | "image/x-ms-bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        other if other.starts_with("image/") => Some("jpg"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/station_logo_tests.rs"]
mod tests;
