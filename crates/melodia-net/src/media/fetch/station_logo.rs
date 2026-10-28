//! One station's logo, off a third-party host, as bytes for the store.
//!
//! The guards are about the host being unknown: the scheme, the deadline and the byte cap.
//! Whether the image is drawable is `media::image::logo_tile`'s. Reached only through
//! `library::radio::fetch_logo`, behind the switch that turns Radio off. Nothing here logs a URL.

use std::time::Duration;

use melodia_core::error::AppError;

/// Below the artist path's: a site icon rather than a press photo, bounding a host that answers a
/// favicon request with a hero image.
const MAX_LOGO_BYTES: u64 = 2 * 1024 * 1024;

/// Start to finish, the shared client bounding only a read and a connect. Short, because a
/// favicon this slow is a miss and a page's worth of them queue behind each other.
pub(super) const LOGO_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A logo as the host served it, before the store has had a say.
#[derive(Debug)]
pub struct FetchedLogo {
    pub bytes: Vec<u8>,
    /// What the response's type is filed under; see [`extension_for`].
    pub extension: &'static str,
}

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
    // Ahead of the body, so an oversized host costs a header; the body is capped too, the field
    // being optional.
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

/// HTTP or HTTPS and nothing else. Cleartext is admitted because refusing it cost real logos and
/// bought little: nothing is sent, and what comes back is only decoded, bounded and re-encoded.
pub(super) fn fetchable_url(favicon_url: &str) -> Result<reqwest::Url, AppError> {
    crate::services::net::http_url(favicon_url).ok_or_else(|| {
        AppError::network_msg("Station logo URL must be an http:// or https:// address")
    })
}

fn stored_extension(response: &reqwest::Response) -> Option<&'static str> {
    let header = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    extension_for(header)
}

/// The extension to file the bytes under, or `None` for a response the store can't hold.
///
/// `.ico` widened `artwork::STORED_EXTENSIONS` and the `image` features behind it, earned because
/// a station's logo field is a favicon field. Any other image type files as JPEG: every reader
/// sniffs content, and the store's header parse has the last word.
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
