//! Small shared helpers and artwork constants for the UI glue layer: one source of truth
//! for the row conversions and the cover-decode sizes, rather than a comment in each copy
//! asserting they match.

use slint::{Rgb8Pixel, SharedPixelBuffer};

/// Side length (px) a sharp cover tile is downscaled to. Matches the 384 px maximum on-screen
/// tile in Now Playing, the largest cover the app paints, so it neither upscales nor pays for a
/// 2× `HiDPI` buffer — one decode size across the app being worth more than the buffers it would
/// save on surfaces holding a single image. The two are equal only at 1×, one being a Slint
/// logical length and the other a decode size in physical pixels; neither derives from the other.
pub const COVER_SIZE: u32 = 384;

/// Side length a cover is downscaled to before blurring. A backdrop carries no fine detail and is
/// stretched under `image-fit: cover`, so downscaling first makes the blur cheap and anything
/// larger buys nothing — but **the floor binds harder than the ceiling**: the box average that
/// gets a cover here dilutes thin bright detail into its ground *before* the blur can bloom it, so
/// a sleeve of fine highlights on a dark field flattens well before the wash itself looks any
/// different.
pub const BLUR_TARGET: u32 = 128;

/// `fast_blur` sigma at [`BLUR_TARGET`] — a soft wash of colour with no recognisable
/// shapes left in it. Proportional to that side, so the two move together or the wash changes
/// strength. The Now Playing tier's, that being the one backdrop with nothing painted over it;
/// every band runs lighter and says so at its own `BlurSpec`.
pub const BLUR_SIGMA: f32 = 16.0;

/// Copy an `image` RGB8 buffer into a Slint `SharedPixelBuffer`. Both are tightly packed,
/// so the byte lengths match and one `copy_from_slice` suffices.
pub fn buffer_from_rgb(img: &image::RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = img.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    buf
}

/// Saturating `i64 → i32`: Slint's generated models use `i32` ids, the DB `i64`.
/// Real ids never overflow; saturating keeps the conversion total.
pub fn clamp_i64_to_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// Saturating `usize → i32` for a collection length. A library that overflows an
/// `i32` has bigger problems than a wrong stats line, so saturate rather than wrap.
pub fn len_as_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// Saturating `u32 → i32` for a tally on its way into a gettext plural. Same
/// argument as [`len_as_i32`]; the input type is what differs, the counts that
/// reach a toast being accumulated as `u32`.
pub fn count_as_i32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

#[cfg(test)]
#[path = "tests/util_tests.rs"]
mod tests;
