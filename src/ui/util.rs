//! Small shared helpers and artwork constants for the UI glue layer.
//!
//! Everything here was previously copy-pasted across two or more modules —
//! `albums.rs`/`browse.rs`/`tracks.rs` for the conversions, and the four
//! cover-decode paths for the sizing constants. Hoisted so there's a single
//! source of truth rather than a comment in each copy asserting they match.

use slint::{Rgb8Pixel, SharedPixelBuffer};

/// Side length (px) a sharp cover tile is downscaled to.
///
/// Roughly matches the ~380 px maximum on-screen tile in the Now-Playing view,
/// the largest cover the app paints, so it neither upscales nor pays for a 2×
/// `HiDPI` buffer there. The two smaller consumers ride the same tier rather
/// than sizing to their own tiles — the Album Detail header's
/// `Theme.hero-artwork` square and the Edit-Tags dialog's preview, both of
/// which this covers at 2× with room over: one decode size across the app is
/// worth more than the buffers it would save on surfaces holding one image.
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
/// recognisable shapes left in it. Taken by the Now Playing tier and the
/// mosaic composition, the two backdrops with nothing painted over them.
///
/// The detail heroes deliberately run lighter, and say so at their own
/// `BlurSpec` rather than here: their gradient floor and solved scrim sit on
/// top, so a softer blur is enough.
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

/// Saturating `usize → i32` for a collection length. Slint counts are `i32`;
/// a library that overflows one has bigger problems than a wrong stats line,
/// so saturate rather than wrap.
pub fn len_as_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

/// Lowercased sort key for an optional string; `None` sorts as the empty
/// string. Used by the in-memory track-list sorts in [`crate::ui::track_sort`].
pub fn opt_lc(s: Option<&str>) -> String {
    s.unwrap_or("").to_lowercase()
}

#[cfg(test)]
#[path = "tests/util_tests.rs"]
mod tests;
