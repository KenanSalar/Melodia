//! The primitives every image path shares: one bounded decode, one resize, and the gate that
//! keeps two oversized decodes off the heap at once.
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
//! [`decode_capped_to`] is that same decode told what it is being drawn at, so it can
//! take JPEG's own scale-on-decode where the format allows and fall back here where it
//! doesn't. A variant rather than a third primitive: same bound, same contract, and the
//! resize behind it is unchanged.
//!
//! [`resize_rgb8`] is shared for a different reason than the decode. Correctness is
//! most of it — the store is resized through the same call that reads it back, so it
//! cannot filter differently from its consumers — but the resampler is also the
//! dominant per-cover cost, and one copy is what makes swapping it a single edit.
//! **It takes a target and computes none.** What shape a cover ends up is a question
//! each tier answers differently, so it stays at the call site; [`fit_within`] is the
//! one piece of that arithmetic more than one of them needed.
//!
//! [`large_decode_guard`] is shared for the reason the bound above is: what it holds down is
//! process RSS, so a second copy beside it would cap its own half and leave the sum free.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;

use fast_image_resize::{ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, GrayImage, RgbImage};
use jpeg_decoder::PixelFormat;

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

/// [`decode_capped`], decoding no larger than it has to for a `target`-square draw.
///
/// JPEG carries its own downscale: the IDCT runs at 1/8, 1/4 or 1/2 for a fraction of the full
/// cost, and every tier draws a cover far smaller than the store holds, so most of what a full
/// decode produces is pixels the resize behind it was always going to discard. The result still
/// covers `target` on **both** edges — the short one is what a square tier resizes from, so a
/// bound on the long edge alone would hand it a source to enlarge. Entropy decoding is not
/// skipped, only the transform, so this is a fraction rather than the scale factor.
///
/// Falls back to [`decode_capped`] for every other format, for the two colour spaces it does not
/// convert, and for a source over `max_dim` — the bound stays [`capped_limits`]'s alone rather
/// than being spelled a second time here. **Which arm runs is decided from the name**, so a cover
/// that was never going to take the fast path is opened once rather than twice.
///
/// **Blocking** — same contract as [`decode_capped`].
pub fn decode_capped_to(
    path: &Path,
    max_dim: u32,
    target: u32,
) -> image::ImageResult<DynamicImage> {
    match decode_jpeg_scaled(path, max_dim, target) {
        Some(scaled) => Ok(scaled),
        None => decode_capped(path, max_dim),
    }
}

/// The JPEG half of [`decode_capped_to`]. `None` wherever the caller owes a fallback, which
/// includes a source this refuses on size: erroring here would put the dimension bound in two
/// places, where declining sends the same file through [`capped_limits`] for the same failure.
///
/// **The one thing in this module that reads a name instead of a header**, and it is a question
/// about cost rather than about content: sniffing means opening every cover to read two bytes
/// most of them will decline on, where the extension is already in hand. Nothing rests on the
/// answer — both arms end in a real decode against a guessed format, so a mislabelled file loses
/// the fast path and stays correct. Every caller passes a store path, and `media::artwork` puts
/// the format in the name.
fn decode_jpeg_scaled(path: &Path, max_dim: u32, target: u32) -> Option<DynamicImage> {
    if !is_jpeg_name(path) {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    if u32::from(info.width) > max_dim || u32::from(info.height) > max_dim {
        return None;
    }

    let requested = short_edge_request(info.width, info.height, target);
    let (width, height) = decoder.scale(requested, requested).ok()?;
    let pixels = decoder.decode().ok()?;

    let (width, height) = (u32::from(width), u32::from(height));
    match info.pixel_format {
        PixelFormat::RGB24 => {
            RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8)
        }
        PixelFormat::L8 => GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
        // A 16-bit grey needs a byte-order-aware widen and a CMYK an Adobe-inverted convert.
        // `image` has both, and neither reaches a real cover often enough to copy here.
        PixelFormat::L16 | PixelFormat::CMYK32 => None,
    }
}

/// Both spellings `media::artwork` writes for a JPEG, case-folded — a user's own picked cover
/// keeps whatever case its filesystem gave it.
fn is_jpeg_name(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"))
}

/// The scale request that makes a decode clear `target` on the source's **short** edge.
///
/// `jpeg-decoder` takes the first scale where *either* axis reaches the request, which on a
/// non-square source is always the long one — so asking for `target` outright leaves the short
/// edge under it, and a square tier resizing from that enlarges what it was handed. Asking for
/// the long edge in the source's own ratio moves the bar onto the axis that has to clear it.
///
/// A source already shorter than `target` finds no scale that qualifies and falls through to a
/// full decode, which is the answer: there is nothing to downscale.
fn short_edge_request(width: u16, height: u16, target: u32) -> u16 {
    let short = u32::from(width.min(height)).max(1);
    let long = u32::from(width.max(height));
    u16::try_from(target.max(1).saturating_mul(long).div_ceil(short)).unwrap_or(u16::MAX)
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

    // Equal sizes are not rare — a cover under the tier's tile and one over only the store's
    // *byte* bound both arrive asking for the source's own dimensions — and the resizer answers
    // those with a zeroed destination and a row-by-row copy of what is already in hand.
    if (width, height) == source.dimensions() {
        return Some(source.into_owned());
    }

    let mut dst = RgbImage::new(width, height);
    RESIZER
        .with_borrow_mut(|resizer| {
            let resized = resizer.resize(
                source.as_ref(),
                &mut dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
            );
            if resizer.size_of_internal_buffers() > RESIZER_SCRATCH_CAP {
                resizer.reset_internal_buffers();
            }
            resized
        })
        .ok()?;
    Some(dst)
}

/// Ceiling on the scratch a thread's resampler may keep between calls.
///
/// The resizer holds one intermediate buffer sized by the *source* width against the target
/// height and grows it without ever shrinking, so one outsized source would leave that thread
/// holding it for the process lifetime — once per Rayon worker and once per blocking-pool thread.
/// Clear of *twice* what a stored cover into the largest tile needs, because the buffer grows by
/// `max(capacity * 2, required)`: gated on the request size instead, two ordinary tiers in the
/// wrong order trip the reset that the steady state is the whole point of avoiding.
const RESIZER_SCRATCH_CAP: usize = 2 * 1024 * 1024;

thread_local! {
    /// One resizer per worker rather than one per cover: it owns the scratch buffers the
    /// convolution runs in, capped by [`RESIZER_SCRATCH_CAP`]. Every caller is already on a Rayon
    /// worker or a blocking task, so thread-local is the whole of the sharing needed.
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

/// Source pixel count past which a decode waits its turn on [`LARGE_DECODE_GATE`] — well above any
/// real cover or station logo, for the occasional full-resolution original a user's tags carry or a
/// host serves as a favicon.
const LARGE_SOURCE_PIXELS: u64 = 4_000_000;

/// Serializes oversized decodes against each other, across every pool that runs one.
///
/// A worker pool bounds the transient peak at *pool width* × the largest source, the wrong shape
/// when one image is enormous and the rest are thumbnails: narrowing the pool for the rare huge one
/// would cost every scan. Gating on the header-read size leaves the common path untouched and caps
/// the peak at one full-resolution bitmap.
///
/// **One gate rather than one per caller**, which is the whole reason it sits here: the peak it
/// bounds is process RSS, and two gates would each cap their own half while the sum went unbounded.
/// The two callers are also on different pools — covers on the rayon decode pool, station logos on
/// tokio's blocking pool — so nothing else relates them.
static LARGE_DECODE_GATE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// A guard held for the length of an oversized decode, or `None` where `pixels` is small enough to
/// need none.
///
/// Take it from the header read that precedes the decode — [`source_pixels`] for a path,
/// [`memory_dimensions`] for bytes in hand — and hold it across everything that keeps a
/// full-resolution buffer alive, the flatten and re-encode included, not just the decode call.
#[must_use]
pub fn large_decode_guard(pixels: u64) -> Option<parking_lot::MutexGuard<'static, ()>> {
    (pixels > LARGE_SOURCE_PIXELS).then(|| LARGE_DECODE_GATE.lock())
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
