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

/// A gradient rather than a flat fill: flat art re-encodes *larger* than its own PNG, which is the
/// never-grow guard's case below rather than this one.
fn gradient_png(tmp: &TempDir, name: &str, side: u32) -> Result<PathBuf, AppError> {
    let path = tmp.path().join(name);
    let channel = |value: u32| u8::try_from(value % 256).unwrap_or(0);
    image::RgbImage::from_fn(side, side, |x, y| {
        image::Rgb([channel(x), channel(y), channel(x + y)])
    })
    .save(&path)
    .map_err(|e| AppError::metadata(format!("Failed to write {}", path.display()), e))?;
    Ok(path)
}

/// The widest edge a cover keeps, from either side of it. A bound written `<` where it means `<=`
/// re-encodes every cover already sized for the job, spending quality on nothing.
#[test]
fn a_cover_on_the_dimension_cap_is_untouched_and_one_pixel_past_it_is_fitted()
-> Result<(), AppError> {
    let tmp = TempDir::new()?;

    let on_the_cap = gradient_png(&tmp, "on-the-cap.png", EMBED_MAX_DIM)?;
    let kept = cover_picture_from_path(&on_the_cap)?;
    assert_eq!(kept.data(), std::fs::read(&on_the_cap)?, "a cover already inside the cap");

    let past_it = gradient_png(&tmp, "past-it.png", EMBED_MAX_DIM + 1)?;
    let fitted = cover_picture_from_path(&past_it)?;
    assert_eq!(fitted.mime_type(), Some(&MimeType::Jpeg), "one pixel past it");
    Ok(())
}

/// Under a fixed quality a cheaply-encoded source is always at risk of growing, so a cap may cost
/// CPU but must never make the user's own file bigger. Flat art a shade over the dimension cap is
/// what reaches this: the PNG holds it in a few bytes where the JPEG spends thousands.
#[test]
fn a_re_encode_that_would_grow_the_file_hands_the_original_back() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("flat.png");
    image::RgbImage::from_pixel(EMBED_MAX_DIM + 1, EMBED_MAX_DIM + 1, image::Rgb([9, 9, 9]))
        .save(&path)
        .map_err(|e| AppError::metadata(format!("Failed to write {}", path.display()), e))?;
    let original = std::fs::read(&path)?;

    let embedded = cover_picture_from_path(&path)?;

    // Format first: a regression here hands back a JPEG, and comparing a megabyte of bytes to say
    // so prints a megabyte.
    assert_eq!(
        embedded.mime_type(),
        Some(&MimeType::Png),
        "the re-encode grew the user's own file"
    );
    assert_eq!(embedded.data().len(), original.len());
    Ok(())
}

/// The other bound, and the one place the never-grow guard is deliberately waived: past the byte
/// cap the tag itself is what grows, so the re-encode goes in whatever it costs. Dimensions decide
/// decode cost where bytes decide how much of the user's file is picture, and 16-bit PNG is what
/// reaches one cap without the other.
#[test]
fn a_cover_past_the_byte_cap_is_re_encoded_even_where_it_would_embed_as_is() -> Result<(), AppError>
{
    /// Incompressible noise, so the PNG lands near its raw size. Fixed seed: a fixture that
    /// sometimes compresses under the cap is a test that sometimes checks nothing.
    const SEED: u32 = 0x2545_F491;
    let tmp = TempDir::new()?;
    let path = tmp.path().join("deep.png");

    let mut state = SEED;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        u16::try_from(state >> 16).unwrap_or(0)
    };
    let noise = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(1000, 1000, |_, _| {
        image::Rgb([next(), next(), next()])
    });
    noise
        .save(&path)
        .map_err(|e| AppError::metadata(format!("Failed to write {}", path.display()), e))?;
    let original = std::fs::read(&path)?;
    assert!(original.len() > EMBED_MAX_BYTES, "the fixture never reached the byte cap");

    let embedded = cover_picture_from_path(&path)?;

    assert_eq!(embedded.mime_type(), Some(&MimeType::Jpeg));
    assert!(embedded.data().len() < original.len());
    Ok(())
}

/// Lofty sniffs WebP happily and MP4's `covr` writer refuses it, so no picker filter can express
/// what every container takes. Normalizing rather than filtering is what covers that, and it has
/// nothing to do with either cap: this source is well inside both.
#[test]
fn a_source_no_container_takes_is_re_encoded_however_small_it_is() -> Result<(), AppError> {
    let webp = assets_dir().join("cover.webp");

    let embedded = cover_picture_from_path(&webp)?;

    assert_eq!(embedded.mime_type(), Some(&MimeType::Jpeg));
    assert_ne!(embedded.data(), std::fs::read(&webp)?.as_slice());
    Ok(())
}

/// `Picture::from_reader` always yields `PictureType::Other`, so every path out of here owes the
/// same correction — including the two that hand back a re-encode rather than the file's own bytes.
#[test]
fn a_re_encoded_cover_is_still_the_front_cover() -> Result<(), AppError> {
    let tmp = TempDir::new()?;

    for path in
        [gradient_png(&tmp, "fitted.png", EMBED_MAX_DIM + 1)?, assets_dir().join("cover.webp")]
    {
        let embedded = cover_picture_from_path(&path)?;
        assert_eq!(embedded.pic_type(), PictureType::CoverFront, "{}", path.display());
    }
    Ok(())
}
