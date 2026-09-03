//! One radio station's logo, from a third-party host into the radio-logo store.
//!
//! [`super::deezer::download_and_cache_artist_image`]'s shape, and deliberately not a
//! parameterisation of it: what the two share is the store handoff, and what they differ on is
//! everything before it. Deezer serves a fixed size from two domains we can name, so its guards
//! are an allowlist and a byte cap. A station's `favicon_url` points anywhere the station's owner
//! typed, at whatever a browser tab wanted, so the guards here are about the host being unknown
//! and the image being too small to draw rather than about it being the wrong host. It is also
//! why the bytes are run through [`logo_tile::compose`] on the way in and Deezer's are not: a
//! press photo is already the shape its tier draws, a favicon is whatever the site had.
//!
//! Reached only through `library::radio::fetch_logo`, which is where the switch that turns Radio
//! off refuses. Nothing here logs a URL.

use std::path::Path;
use std::time::Duration;

use crate::entities::radio::StoredLogo;
use crate::error::AppError;
use crate::media::logo_tile::Tile;
use crate::media::{artwork, image_decode, logo_tile};

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

/// Deadline on one logo request, start to finish.
///
/// The shared client bounds a *read* and a connect, not a request, so a host that trickles bytes
/// never trips either and holds its slot for minutes. `services::radio_browser` wraps its own calls
/// for the same reason. Set well under that: a favicon this slow is a miss, and a page's worth of
/// them queue behind each other.
pub(super) const LOGO_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetch and store one station logo, returning where it landed.
///
/// `Ok(None)` is a usable answer with no logo in it (not an image, an unsupported container, or
/// too small to draw); `Err` is a failure worth retrying later. Callers memoize both, so the
/// distinction is what separates a debug line from a warning rather than what separates a retry
/// from a give-up.
pub async fn fetch(
    client: &reqwest::Client,
    favicon_url: &str,
    artwork_dir: &Path,
) -> Result<Option<StoredLogo>, AppError> {
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

    let bytes =
        crate::services::net::read_capped(response, "Station logo body", MAX_LOGO_BYTES).await?;

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
pub(super) fn fetchable_url(favicon_url: &str) -> Result<reqwest::Url, AppError> {
    crate::services::net::http_url(favicon_url).ok_or_else(|| {
        AppError::network_msg("Station logo URL must be an http:// or https:// address")
    })
}

/// The store's own bounds, plus the floor above and [`logo_tile::compose`], on the decode pool.
///
/// The floor is asked first and off the header alone, so a source too small to draw costs no
/// decode at all.
fn store_if_big_enough(bytes: &[u8], ext: &'static str, dir: &Path) -> Option<StoredLogo> {
    let (width, height) = image_decode::memory_dimensions(bytes, image_decode::MAX_SOURCE_DIM)?;
    if width < MIN_LOGO_DIM || height < MIN_LOGO_DIM {
        return None;
    }
    // Held to the end rather than around the decode: `logo_tile::compose` flattens at full source
    // resolution before it downscales, and the store decodes again to bound an oversized source.
    // `MAX_LOGO_BYTES` is no substitute — it bounds the *compressed* size, which a flat-colour icon
    // stays under at any dimension, and the fetch window runs several at once.
    let _oversized = image_decode::large_decode_guard(u64::from(width) * u64::from(height));
    // `None` is "store the source's own bytes": it is already a square opaque icon, or the tile
    // it wanted would not encode and the source is the better of the two.
    let tile = match tile_for(bytes) {
        Tile::Composed(tile) => encoded_png(tile),
        Tile::SourceIsFine => None,
        // Refused rather than stored untreated. The caller reads `None` as "nothing usable came
        // back" and the card falls back to its monogram, where the source paints an empty square.
        Tile::Undrawable => return None,
    };

    let path = match tile.as_deref() {
        Some(tile) => artwork::store_image(tile, "png", dir),
        None => artwork::store_image(bytes, ext, dir),
    }?;
    // A file the store just wrote or already had; a stat that fails says nothing about whether it
    // is drawable, so the answer stands and the size falls back to what arrived.
    let stored = std::fs::metadata(&path).map_or(bytes.len() as u64, |meta| meta.len());
    Some(StoredLogo {
        path,
        bytes: stored,
    })
}

/// What [`logo_tile::compose`] makes of the source, past the decode it needs.
///
/// A source the decoder refuses is [`Tile::SourceIsFine`] rather than [`Tile::Undrawable`]: the
/// header read in [`store_if_big_enough`] already says the store can hold it, and with no decode
/// there is no evidence it wanted a tile at all.
fn tile_for(bytes: &[u8]) -> Tile {
    match image_decode::decode_memory_capped(bytes, image_decode::MAX_SOURCE_DIM) {
        Some(decoded) => logo_tile::compose(decoded),
        None => Tile::SourceIsFine,
    }
}

/// A composed tile as PNG rather than the store's JPEG.
///
/// A tile is flat ground behind a hard-edged mark, which is what JPEG rings around and what PNG
/// holds in a few kilobytes; `store_image` re-encodes anyway for the rare composite that lands
/// over its byte bound.
fn encoded_png(tile: image::RgbImage) -> Option<Vec<u8>> {
    let mut encoded = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut encoded);
    image::DynamicImage::ImageRgb8(tile).write_with_encoder(encoder).ok()?;
    Some(encoded)
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
