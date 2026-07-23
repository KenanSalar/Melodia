//! Small shared helpers for the UI glue layer.
//!
//! These were previously copy-pasted across `albums.rs`, `browse.rs`, and
//! `tracks.rs` — identical definitions in three places. Hoisted here so
//! there's a single source of truth.

use slint::{Rgb8Pixel, SharedPixelBuffer};

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
