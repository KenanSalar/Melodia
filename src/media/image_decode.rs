//! The two primitives every image path shares: one bounded decode, and one resize.
//!
//! Cover art reaches the app from files the user didn't write, so every decode has
//! to be bounded before it runs: a forged dimension header in a tag can ask for
//! gigabytes long before the downscale that follows would have capped it. `image`
//! takes that bound as [`image::Limits`] on the reader, so the same four-step
//! preamble — open, guess the format, apply the limits, decode — belongs in front of
//! every decode in the tree, and this is the single copy of it.
//!
//! [`capped_limits`] is for the one decode that can't use it: `tag_writer` reads a
//! picked cover from memory, so it builds its own reader and takes only the bound.
//! It needs the guessed [`image::ImageFormat`] back and reports which step failed,
//! neither of which survives the `Option` [`decode_memory_capped`] returns.
//!
//! [`resize_rgb8`] is shared for a different reason than the decode. Correctness is
//! most of it — the store is resized through the same call that reads it back, so it
//! cannot filter differently from its consumers — but the resampler is also the
//! dominant per-cover cost, and one copy is what makes swapping it a single edit.
//! **It takes a target and computes none.** What shape a cover ends up is a question
//! each tier answers differently, so it stays at the call site; [`fit_within`] is the
//! one piece of that arithmetic more than one of them needed.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;

use fast_image_resize::{ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, RgbImage};

/// Re-exported so a call site picks a filter without naming the resampler crate, which is what
/// keeps [`resize_rgb8`] the one place it is reachable from.
pub use fast_image_resize::FilterType;

/// Hard cap on accepted source resolution for artwork.
///
/// Real album art is far under this; the limit only stops a malformed file
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

/// [`decode_capped`] for bytes already in hand, for the store's own normalizer.
///
/// `tag_writer` still builds its own reader rather than calling this: it needs the guessed
/// [`image::ImageFormat`] back and reports which step failed, neither of which survives an
/// `Option`.
pub fn decode_memory_capped(bytes: &[u8], max_dim: u32) -> Option<DynamicImage> {
    memory_reader(bytes, max_dim)?.decode().ok()
}

/// The source's dimensions from its header alone — [`source_pixels`] for bytes in hand, and the
/// cheap half of the store's bound check.
///
/// Doubles as the format validation: a container `image` was not built to decode fails here, so a
/// cover nothing in the app could ever draw is never written to disk.
pub fn memory_dimensions(bytes: &[u8], max_dim: u32) -> Option<(u32, u32)> {
    memory_reader(bytes, max_dim)?.into_dimensions().ok()
}

fn memory_reader(bytes: &[u8], max_dim: u32) -> Option<image::ImageReader<std::io::Cursor<&[u8]>>> {
    let mut reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(capped_limits(max_dim));
    Some(reader)
}

/// Resamples `src` to exactly `width` × `height` as RGB8, stretching to fit.
///
/// **Takes a target and computes none.** What shape a cover should end up — square, aspect-fitted,
/// or the backdrop band's deliberate squash — is the caller's question and stays at the call site;
/// only the pixels move here. `filter` is the caller's for the same reason: a file that outlives
/// the session and a banner recomposed per top-4 change buy different things with it.
///
/// **Blocking** — same contract as [`decode_capped`]. `None` if the target is empty or the
/// resampler refuses the buffer.
pub fn resize_rgb8(
    src: &DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> Option<RgbImage> {
    // Borrowed for the JPEG case, which is nearly all album art; anything else pays one
    // conversion rather than being resampled in a layout the resizer would reject.
    let source: Cow<'_, RgbImage> = match src.as_rgb8() {
        Some(rgb) => Cow::Borrowed(rgb),
        None => Cow::Owned(src.to_rgb8()),
    };

    // A convolution at 1:1 walks every pixel to reproduce it, and the shortcut is not rare: a
    // cover under the tier's tile and one over only the store's *byte* bound both arrive here
    // asking for the source's own dimensions.
    if (width, height) == source.dimensions() {
        return Some(source.into_owned());
    }

    let mut dst = RgbImage::new(width, height);
    RESIZER
        .with_borrow_mut(|resizer| {
            resizer.resize(
                source.as_ref(),
                &mut dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
            )
        })
        .ok()?;
    Some(dst)
}

thread_local! {
    /// One resizer per worker rather than one per cover: it owns the scratch buffers the
    /// convolution runs in. Every caller is already on a Rayon worker or a blocking task, so
    /// thread-local is the whole of the sharing needed.
    static RESIZER: RefCell<Resizer> = RefCell::new(Resizer::new());
}

/// `width` × `height` shrunk by a single ratio until it fits inside `max_w` × `max_h`, or
/// returned unchanged where it already does.
///
/// Never enlarges — which is the whole point at every tier, a cover drawn at the tier's size
/// whether or not the buffer was padded out to it. **One ratio, not a clamp per axis**, so the
/// rectangle's own aspect survives; the backdrop band relies on that, its squash being a property
/// of the band rather than of whatever cover it was handed.
///
/// The two operands swap by call site: a tier fits the *source* into its tile, the backdrop fits
/// its *tile* into the source.
#[must_use]
pub fn fit_within(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    // Whichever axis overflows further decides the ratio both are scaled by. Cross-multiplied
    // rather than divided so the comparison stays exact.
    let (numerator, denominator) =
        if u64::from(max_w) * u64::from(height) <= u64::from(max_h) * u64::from(width) {
            (max_w, width)
        } else {
            (max_h, height)
        };
    let shrink = |value: u32| {
        let scaled = u64::from(value) * u64::from(numerator) / u64::from(denominator);
        u32::try_from(scaled).unwrap_or(value).max(1)
    };
    (shrink(width), shrink(height))
}

/// Source pixel count from the header alone, without decoding a pixel.
///
/// `into_dimensions` stops once the decoder has parsed enough to answer, so this is
/// cheap enough to ask before every decode and the only way to know what one is
/// about to cost. A forged header lies here exactly as it lies to `decode_capped`,
/// which is why callers use it to *order* work rather than to trust a size.
pub fn source_pixels(path: &Path) -> Option<u64> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let (width, height) = reader.into_dimensions().ok()?;
    Some(u64::from(width) * u64::from(height))
}

/// Decoder limits refusing anything wider or taller than `max_dim`.
///
/// Everything else stays at `image`'s defaults, which already cap a single
/// allocation — the dimension bound is what stops a forged header claiming a size
/// no real cover has.
#[must_use]
pub fn capped_limits(max_dim: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    limits
}

#[cfg(test)]
#[path = "tests/image_decode_tests.rs"]
mod tests;
