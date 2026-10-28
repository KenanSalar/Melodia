//! Turning a user-picked image file into a cover a tag can carry: [`super::tag_writer`]'s sibling
//! about pixels rather than keys. Nothing here names a `Tag`.
//!
//! The bounds are its own rather than the artwork store's, which fills a cache rebuildable from
//! these bytes, where these land in a file the user keeps. The normalizer underneath is shared.

use std::io::Cursor;
use std::path::Path;

use lofty::picture::{Picture, PictureType};

use melodia_artwork::media::image::image_decode;
use melodia_core::error::AppError;

/// Longest edge an embedded cover keeps: the largest thumbnail the Cover Art Archive publishes and
/// Picard offers short of the original. Far above anything the store draws, and small enough that
/// a picked poster isn't the larger half of every track on the album.
const EMBED_MAX_DIM: u32 = 1200;

/// Byte ceiling, a bound of its own: dimensions decide decode cost and bytes how much of the
/// user's file is picture, and 16-bit PNG or uncompressed TIFF can pass the first and fail the
/// second. It also keeps base64 in a Vorbis comment under the 16 MiB packet Symphonia assembles.
const EMBED_MAX_BYTES: usize = 4 * 1024 * 1024;

const EMBED_JPEG_QUALITY: u8 = 90;

/// Resampler for the re-encode. Lanczos3 for the reason the store's persisted images take it: this
/// outlives the session, and nothing downstream can sharpen it back.
const EMBED_FILTER: image_decode::FilterType = image_decode::FilterType::Lanczos3;

/// Decode-validates a user-picked cover and builds an embeddable [`Picture`], once per batch.
///
/// lofty rejects anything outside PNG, JPEG, GIF, BMP and TIFF, and MP4's `covr` writer refuses the
/// TIFF lofty accepts, so this normalizes rather than filters: JPEG and PNG embed byte for byte,
/// everything else is re-encoded to JPEG. The decode is the **validation**, `from_reader` never
/// decoding, so a corrupt pick fails the batch before any file is touched.
pub fn cover_picture_from_path(path: &Path) -> Result<Picture, AppError> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::metadata(format!("Failed to read cover {}", path.display()), e))?;

    let mut reader =
        image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| {
            AppError::metadata(format!("Unrecognized image format: {}", path.display()), e)
        })?;
    // `decode_capped`'s bound, which a decode from memory can't go through.
    reader.limits(image_decode::capped_limits(image_decode::MAX_SOURCE_DIM));

    let format = reader.format();
    let decoded = reader
        .decode()
        .map_err(|e| AppError::metadata(format!("Failed to decode cover {}", path.display()), e))?;

    let embeds_as_is = matches!(format, Some(image::ImageFormat::Jpeg | image::ImageFormat::Png));
    let within_dims = decoded.width() <= EMBED_MAX_DIM && decoded.height() <= EMBED_MAX_DIM;
    let within_bytes = bytes.len() <= EMBED_MAX_BYTES;

    let data = if embeds_as_is && within_dims && within_bytes {
        bytes
    } else {
        let jpeg = embeddable_jpeg(&decoded, path)?;
        // A re-encode that grew is declined, as the store's normalizer declines one: a cap may
        // cost CPU but never makes the user's file bigger. Not past the byte cap, where the tag
        // itself grows, nor for a source no container would take.
        if embeds_as_is && within_bytes && jpeg.len() >= bytes.len() { bytes } else { jpeg }
    };

    let mut picture = Picture::from_reader(&mut Cursor::new(&data)).map_err(|e| {
        AppError::metadata(format!("Not a usable cover image: {}", path.display()), e)
    })?;
    // `from_reader` always yields `PictureType::Other`.
    picture.set_pic_type(PictureType::CoverFront);
    Ok(picture)
}

/// `decoded` fitted inside the embed bounds and re-encoded as a JPEG.
///
/// Flattens any alpha, as the store's normalizer does: every tier draws from `cover_thumbs`' RGB,
/// so nothing here can show transparency, and keeping it would mean a PNG the cap exists to avoid.
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
