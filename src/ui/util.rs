//! Small shared helpers and artwork constants for the UI glue layer.
//!
//! Everything here was previously copy-pasted across two or more modules —
//! `albums.rs`/`browse.rs`/`tracks.rs` for the conversions, and the four
//! cover-decode paths for the sizing constants. Hoisted so there's a single
//! source of truth rather than a comment in each copy asserting they match.

use slint::{Rgb8Pixel, SharedPixelBuffer};

/// Side length (px) a sharp cover tile is downscaled to.
///
/// Roughly matches the ~380 px maximum on-screen tile shared by the Now-Playing
/// view and the Album Detail header, so it neither upscales nor pays for a 2×
/// `HiDPI` buffer — and both surfaces decode at one size. The Edit-Tags cover
/// preview rides the same tier rather than sizing to its own 160 px tile: one
/// decode size across the app is worth more than the buffer it saves on a
/// dialog that holds one image.
pub const COVER_SIZE: u32 = 384;

/// Side length a cover is downscaled to before blurring.
///
/// A backdrop carries no fine detail and is stretched to fill under
/// `image-fit: cover`, so downscaling first makes the blur cheap — box-pass cost
/// scales with pixel count — and anything larger buys nothing. The Album Detail
/// hero pairs this width with a shorter height, since its region paints
/// landscape.
pub const BLUR_TARGET: u32 = 192;

/// `fast_blur` sigma at [`BLUR_TARGET`] — a soft wash of colour with no
/// recognisable shapes left in it.
///
/// The Album Detail hero deliberately runs lighter than this: its gradient floor
/// and crust scrim sit on top, so a softer blur is enough.
pub const BLUR_SIGMA: f32 = 24.0;

/// Copy an `image` RGB8 buffer into a Slint `SharedPixelBuffer`. Shared by every
/// cover/blur decode path (`detail_artwork`, `now_playing_artwork`,
/// `mosaic_blur`, the tag-editor preview); the buffer is tightly packed, so the
/// byte lengths match and a single `copy_from_slice` suffices.
pub fn buffer_from_rgb(img: &image::RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = img.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    buf
}

/// Saturating `i64 → i32` conversion. Slint's generated models use `i32`
/// for ids; DB ids are `i64`. Real ids never overflow, but the saturating
/// fallback keeps the conversion total instead of panicking.
pub fn clamp_i64_to_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// Lowercased sort key for an optional string; `None` sorts as the empty
/// string. Used by the in-memory track-list sorts in [`crate::ui::track_sort`].
pub fn opt_lc(s: Option<&str>) -> String {
    s.unwrap_or("").to_lowercase()
}

#[cfg(test)]
#[path = "tests/util_tests.rs"]
mod tests;
