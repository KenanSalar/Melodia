//! Inputs made on demand, for the tests that need a real file or an out-of-range value.

use std::path::PathBuf;

/// The unbounded float fed to the guards that reject nonsense input. `f64::from` widens it
/// where the guard under test takes an `f64`.
pub const UNBOUNDED: f32 = f32::MAX * 2.0;

/// A solid-colour `side` × `side` PNG in a fresh temp dir. The dir comes back alongside the
/// path so the caller can keep it alive — dropping it deletes the file.
pub fn write_test_png(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(side, side, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

/// [`write_test_png`]'s counterpart, for the decode paths that only fire on JPEG.
///
/// A gradient rather than a flat fill: a solid colour is one DC coefficient per block, so every
/// scale factor reproduces it exactly and a scaled decode that ignored the size it was asked for
/// would pass anyway.
pub fn write_test_jpeg(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    write_test_jpeg_sized(side, side)
}

/// [`write_test_jpeg`] at an arbitrary aspect ratio, for the half of the scaled-decode contract a
/// square source cannot fail: a scale picked off the long edge alone still clears the short one.
pub fn write_test_jpeg_sized(
    width: u32,
    height: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.jpg");
    let channel = |value: u32| u8::try_from(value % 256).unwrap_or(0);
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([channel(x), channel(y), channel(x + y)])
    })
    .save(&path)?;
    Ok((tmp, path))
}
