//! Turning a user-picked image file into a cover a tag can carry.
//!
//! Beside [`super::tag_writer`] rather than inside it for the reason [`super::rating_tags`] and
//! [`super::role_tags`] sit there: each owns one thing a container is particular about, and this
//! one is particular about pixels rather than keys. Nothing here names a `Tag`.
//!
//! The bounds are this module's own and deliberately not the artwork store's, which answers a
//! different question: that one fills a cache we can rebuild from these bytes, and these land in
//! a file the user keeps. What the two do share is the normalizer underneath, `image_decode`.

use std::io::Cursor;
use std::path::Path;

use lofty::picture::{Picture, PictureType};

use melodia_artwork::media::image::image_decode;
use melodia_core::error::AppError;

/// Longest edge an embedded cover keeps.
///
/// The largest thumbnail the Cover Art Archive publishes and the largest size Picard offers short
/// of the original, so it is where the tools a library has already been through agree. Far above
/// anything the artwork store draws, and small enough that a picked poster does not become the
/// larger half of every track on the album it is written to.
const EMBED_MAX_DIM: u32 = 1200;

/// Byte ceiling, and a bound of its own rather than a consequence of [`EMBED_MAX_DIM`]: dimensions
/// decide decode cost, bytes decide how much of the user's file is picture, and an image can fail
/// either alone. Ordinary 8-bit encodings cannot reach this inside the dimension cap; 16-bit PNG
/// and uncompressed TIFF can. It also sits well under the point where base64 in a Vorbis comment
/// would push the packet past the 16 MiB Symphonia will assemble, which is a file Melodia would
/// have written and then refused to play.
const EMBED_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Quality for the re-encode. Its own knob rather than the artwork store's, which answers a
/// different question: that one fills a cache we can rebuild from this, and this lands in a file
/// the user keeps.
const EMBED_JPEG_QUALITY: u8 = 90;

/// Resampler for the re-encode. Lanczos3 for the reason the store's persisted images take it: this
/// outlives the session, and nothing downstream can sharpen it back.
const EMBED_FILTER: image_decode::FilterType = image_decode::FilterType::Lanczos3;

/// Decode-validate a user-picked cover and produce an embeddable [`Picture`].
///
/// Two constraints that don't agree: lofty sniffs the mime from 8 bytes and rejects anything
/// outside PNG / JPEG / GIF / BMP / TIFF outright, while MP4's `covr` writer hard-errors on TIFF —
/// which lofty happily sniffs — but only on M4A/ALAC. No single picker filter can express that, so
/// normalize rather than filter: JPEG and PNG embed byte-for-byte, everything else is re-encoded
/// to JPEG, which every container accepts.
///
/// The decode is also the **validation**: `Picture::from_reader` never decodes, so a truncated
/// JPEG would embed into N files and only blow up at thumbnail time. Failing here aborts the batch
/// before any file is touched.
///
/// Call it **once per batch**, before any fan-out — it reads the image into memory, so per-track
/// would re-read the file N times.
pub fn cover_picture_from_path(path: &Path) -> Result<Picture, AppError> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::metadata(format!("Failed to read cover {}", path.display()), e))?;

    let mut reader =
        image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| {
            AppError::metadata(format!("Unrecognized image format: {}", path.display()), e)
        })?;
    // The same bound every other artwork decode runs under. Reading from memory rather than a
    // path, this one can't go through `decode_capped` — but a forged header shouldn't get a
    // bigger allocation for being hand-picked.
    reader.limits(image_decode::capped_limits(image_decode::MAX_SOURCE_DIM));

    let format = reader.format();
    let decoded = reader
        .decode()
        .map_err(|e| AppError::metadata(format!("Failed to decode cover {}", path.display()), e))?;

    // Every container we target embeds JPEG and PNG as-is, so a cover already inside the bounds is
    // handed through untouched — `decoded` was only ever the validator. Past them it stops being
    // the validator and becomes the source the re-encode below works from.
    let embeds_as_is = matches!(format, Some(image::ImageFormat::Jpeg | image::ImageFormat::Png));
    let within_bounds = decoded.width() <= EMBED_MAX_DIM
        && decoded.height() <= EMBED_MAX_DIM
        && bytes.len() <= EMBED_MAX_BYTES;

    let data = if embeds_as_is && within_bounds {
        bytes
    } else {
        let jpeg = embeddable_jpeg(&decoded, path)?;
        // The artwork store's normalizer declines a re-encode that grew and this owes the same
        // rule for the same reason: under a fixed quality a cheaply-encoded source is always at
        // risk of growing, so a cap may cost CPU but must never make the user's own file bigger.
        // Flat art a shade over the dimension cap is what reaches it. Only that cap is waived,
        // never the compatibility one: a source no container would take is re-encoded whatever
        // the sizes say.
        if embeds_as_is && jpeg.len() >= bytes.len() { bytes } else { jpeg }
    };

    let mut picture = Picture::from_reader(&mut Cursor::new(&data)).map_err(|e| {
        AppError::metadata(format!("Not a usable cover image: {}", path.display()), e)
    })?;
    // `from_reader` always yields `PictureType::Other`.
    picture.set_pic_type(PictureType::CoverFront);
    Ok(picture)
}

/// `decoded` fitted inside the embed bounds and re-encoded as a JPEG, which every container we
/// target accepts.
///
/// Flattens an alpha channel, and that is the trade rather than an oversight: the artwork store's
/// own normalizer flattens for the same reason, and every tier draws out of `cover_thumbs`, which
/// is `RgbImage` end to end, so the transparency this would preserve is transparency nothing here
/// can show. Keeping it means re-encoding to PNG and giving back most of the size the cap is for.
fn embeddable_jpeg(decoded: &image::DynamicImage, source: &Path) -> Result<Vec<u8>, AppError> {
    let (width, height) =
        image_decode::fit_within(decoded.width(), decoded.height(), EMBED_MAX_DIM, EMBED_MAX_DIM);
    let resized =
        image_decode::resize_rgb8(decoded, width, height, EMBED_FILTER).ok_or_else(|| {
            AppError::metadata_msg(format!("Cover {} could not be resized", source.display()))
        })?;

    image_decode::encode_jpeg(resized, EMBED_JPEG_QUALITY).map_err(|e| {
        AppError::metadata(format!("Failed to re-encode cover {} to JPEG", source.display()), e)
    })
}

#[cfg(test)]
#[path = "tests/cover_embed_tests.rs"]
mod tests;
