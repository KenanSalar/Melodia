//! Normalizing a picked cover into something a tag can carry.
//!
//! What a container then does with the result is `tag_writer_tests.rs`'s: the WebP and M4A cases
//! there are about the save, and these are about the bytes handed to it.

use std::path::PathBuf;

use lofty::picture::{MimeType, PictureType};
use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

fn assets_dir() -> PathBuf {
    PathBuf::from(ASSETS_DIR)
}

#[test]
fn a_jpeg_cover_is_embedded_byte_for_byte() -> Result<(), AppError> {
    let jpg = assets_dir().join("cover.jpg");
    let original = std::fs::read(&jpg)?;

    let picture = cover_picture_from_path(&jpg)?;

    assert_eq!(
        picture.mime_type(),
        Some(&MimeType::Jpeg),
        "JPEG must pass through, not be re-compressed"
    );
    assert_eq!(picture.data(), original.as_slice());
    assert_eq!(picture.pic_type(), PictureType::CoverFront);
    Ok(())
}

#[test]
fn a_corrupt_cover_is_rejected_before_anything_is_written() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let broken = tmp.path().join("truncated.jpg");
    // A valid JPEG magic number and nothing behind it. `Picture::from_reader`
    // sniffs 8 bytes and would happily embed this; only a real decode catches it.
    std::fs::write(&broken, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46])?;

    assert!(
        cover_picture_from_path(&broken).is_err(),
        "the decode is the validation step — a truncated image must not embed"
    );
    Ok(())
}

/// A PNG wider than the cap, in as few bytes as a solid image compresses to. Generated rather than
/// checked in: what the cap answers for is the dimension, and a real cover that size would be
/// megabytes of fixture for one number.
fn oversized_png(tmp: &TempDir) -> Result<PathBuf, AppError> {
    let path = tmp.path().join("poster.png");
    image::RgbImage::from_pixel(EMBED_MAX_DIM * 2, 64, image::Rgb([9, 9, 9]))
        .save(&path)
        .map_err(|e| AppError::metadata(format!("Failed to write {}", path.display()), e))?;
    Ok(path)
}

/// Over the caps a pick is fitted and re-encoded, so one poster does not become the larger half of
/// every file in the batch it is written to. Inside them it is handed through as the user's own
/// bytes, a re-encode there spending quality on nothing.
#[test]
fn an_oversized_cover_is_fitted_and_one_inside_the_caps_is_untouched() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    let fitted = cover_picture_from_path(&oversized_png(&tmp)?)?;
    let fitted_size = image::load_from_memory(fitted.data())
        .map(|decoded| (decoded.width(), decoded.height()))
        .map_err(|e| AppError::metadata("Failed to read the fitted cover back", e))?;
    assert_eq!(fitted_size, (EMBED_MAX_DIM, 32));
    assert_eq!(fitted.mime_type(), Some(&MimeType::Jpeg));

    let inside = assets_dir().join("cover.jpg");
    let untouched = cover_picture_from_path(&inside)?;
    assert_eq!(untouched.data(), std::fs::read(&inside)?);
    Ok(())
}
