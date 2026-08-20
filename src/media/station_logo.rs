//! One radio station's logo, from a third-party host into the shared artwork store.
//!
//! [`super::deezer::download_and_cache_artist_image`]'s shape, and deliberately not a
//! parameterisation of it: what the two share is the store handoff, and what they differ on is
//! everything before it. Deezer serves a fixed size from two domains we can name, so its guards
//! are an allowlist and a byte cap. A station's `favicon_url` points anywhere the station's owner
//! typed, at whatever a browser tab wanted, so the guards here are about the host being unknown
//! and the image being too small to draw rather than about it being the wrong host.
//!
//! Reached only through `library::radio::fetch_logo`, which is where the switch that turns Radio
//! off refuses. Nothing here logs a URL.

use std::path::Path;

use crate::error::AppError;
use crate::media::{artwork, image_decode};

/// Ceiling on one logo download, checked against the header and again against the body.
///
/// Smaller than the artist path's: this is a site icon rather than a press photo, and the cap is
/// what bounds a host that answers a favicon request with a hero image.
const MAX_LOGO_BYTES: u64 = 2 * 1024 * 1024;

/// Shortest edge a source may have and still be worth storing.
///
/// Set at favicon size rather than at what the surfaces would like. A grid card and (later) a hero
/// tile are both hundreds of logical pixels wide and `FemtoVG` magnifies bilinear with no mipmaps,
/// so a source down here is visibly soft — but a station's logo field *is* a favicon field, and
/// holding out for something drawable mostly bought the glyph instead. What it still keeps out is
/// the 1×1 pixel that turns up where the field was filled in by a tracker. One gate at the writer
/// rather than one per surface, so nothing this small enters the store for a later tier to reject.
const MIN_LOGO_DIM: u32 = 32;

/// Fetch and store one station logo, returning its path in the artwork store.
///
/// `Ok(None)` is a usable answer with no logo in it (not an image, an unsupported container, or
/// too small to draw); `Err` is a failure worth retrying later. Callers memoize both, so the
/// distinction is what separates a debug line from a warning rather than what separates a retry
/// from a give-up.
pub async fn fetch(
    client: &reqwest::Client,
    favicon_url: &str,
    artwork_dir: &Path,
) -> Result<Option<String>, AppError> {
    let parsed = fetchable_url(favicon_url)?;

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| AppError::network("Station logo download failed", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!("Station logo returned HTTP {status}")));
    }

    let Some(ext) = stored_extension(&response) else {
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

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::network("Station logo body could not be read", e))?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return Err(AppError::network_msg(format!(
            "Station logo too large: {} bytes (max {MAX_LOGO_BYTES})",
            bytes.len()
        )));
    }

    let dir = artwork_dir.to_path_buf();
    tokio::task::spawn_blocking(move || store_if_big_enough(&bytes, ext, &dir))
        .await
        .map_err(AppError::io_source)
}

/// The URL to fetch, or a refusal.
///
/// HTTP as well as HTTPS, and nothing else: a `file://` or `data:` URL is not a fetch to make on a
/// directory row's say-so. Cleartext is admitted because refusing it cost real logos and bought
/// little: no credential is sent, and what comes back is only ever bytes the store decodes as an
/// image, bounds and re-encodes.
fn fetchable_url(favicon_url: &str) -> Result<reqwest::Url, AppError> {
    let parsed = reqwest::Url::parse(favicon_url)
        .map_err(|e| AppError::network("Station logo URL could not be parsed", e))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::network_msg("Station logo URL must be HTTP"));
    }
    Ok(parsed)
}

/// The store's own bounds, plus the floor above, on the decode pool.
///
/// `memory_dimensions` reads the header rather than decoding, so the floor costs nothing on the
/// sources it rejects and nothing on the ones it passes: `store_image` asks the same question
/// again and a header parse is not what either of them spends time on.
fn store_if_big_enough(bytes: &[u8], ext: &'static str, dir: &Path) -> Option<String> {
    let (width, height) = image_decode::memory_dimensions(bytes, image_decode::MAX_SOURCE_DIM)?;
    if width < MIN_LOGO_DIM || height < MIN_LOGO_DIM {
        return None;
    }
    artwork::store_image(bytes, ext, dir)
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
