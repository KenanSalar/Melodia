//! One bounded image decode, for every caller that needs one.
//!
//! Cover art reaches the app from files the user didn't write, so every decode
//! has to be bounded before it runs: a forged dimension header in a tag can ask
//! for gigabytes long before the downscale that follows would have capped it.
//! `image` takes that bound as [`image::Limits`] on the reader, which means the
//! same four-step preamble — open, guess the format, apply the limits, decode —
//! in front of every decode in the tree.
//!
//! It was written out six times. This is the single copy — plus
//! [`capped_limits`] for the one decode that can't use it: `tag_writer` reads a
//! picked cover into memory (it hands the original bytes through untouched when
//! the format is already embeddable), so it builds its own reader and takes only
//! the bound.

use std::path::Path;

use image::DynamicImage;

/// Hard cap on accepted source resolution for artwork.
///
/// Real album art is far under this; the limit only stops a malformed file from
/// triggering an absurd allocation. Callers that downscale hard anyway (the
/// Material You seed) pass their own, smaller cap.
pub const MAX_SOURCE_DIM: u32 = 8192;

/// Decode `path`, refusing anything wider or taller than `max_dim`.
///
/// **Blocking** — call from `spawn_blocking` or a Rayon worker, never the UI
/// thread. The error carries which step failed, so a caller that logs gets
/// something it can act on; a caller that doesn't care writes `.ok()?`.
pub fn decode_capped(path: &Path, max_dim: u32) -> image::ImageResult<DynamicImage> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(capped_limits(max_dim));
    reader.decode()
}

/// Decoder limits refusing anything wider or taller than `max_dim`.
///
/// Everything else is left at `image`'s defaults, which already cap a single
/// allocation — the dimension bound is what stops a forged header claiming a
/// size no real cover has.
#[must_use]
pub fn capped_limits(max_dim: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    limits
}
