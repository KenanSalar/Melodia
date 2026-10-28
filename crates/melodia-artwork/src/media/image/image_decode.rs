//! The primitives every image path shares: one bounded decode, one resize, one JPEG encode, and
//! the gate that keeps two oversized decodes off the heap at once.
//!
//! Cover art comes from files the user didn't write, and a forged dimension header can ask for
//! gigabytes before any downscale caps it, so every decode is bounded here. `cover_embed` decodes
//! from memory and needs the guessed format back, so it builds its own reader around
//! [`capped_limits`]. [`resize_rgb8`] is the one resampler, so the store can't filter differently
//! from the tiers that read it back.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;

use fast_image_resize::{ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, GrayImage, RgbImage};
use jpeg_decoder::PixelFormat;

/// Re-exported so a call site picks a filter without naming the resampler crate, keeping
/// [`resize_rgb8`] the one place it is reachable from.
pub use fast_image_resize::FilterType;

/// Real art is far under this; it only stops a forged header asking for an absurd allocation.
pub const MAX_SOURCE_DIM: u32 = 8192;

/// Decodes `path`, refusing anything wider or taller than `max_dim`. **Blocking.**
pub fn decode_capped(path: &Path, max_dim: u32) -> image::ImageResult<DynamicImage> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(capped_limits(max_dim));
    reader.decode()
}

/// [`decode_capped`], using JPEG's 1/8, 1/4 or 1/2 IDCT to decode no larger than a `target`
/// square needs, every tier drawing far smaller than the store holds. The result still covers
/// `target` on **both** edges, a square tier resizing from the short one. **Blocking.**
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

/// `None` wherever the caller owes the fallback, an oversized source included so the bound stays
/// [`capped_limits`]' alone.
///
/// The arm is picked by extension, because sniffing would open every cover to read two bytes most
/// then decline on. Both arms decode against a guessed format, so a mislabelled file only loses the
/// fast path.
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
        // Rare enough on a cover to leave to `image`'s own conversions.
        PixelFormat::L16 | PixelFormat::CMYK32 => None,
    }
}

/// Case-folded, a picked cover keeping whatever case its filesystem gave it.
fn is_jpeg_name(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"))
}

/// The scale request that makes a decode clear `target` on the source's **short** edge.
/// `jpeg-decoder` takes the first scale where *either* axis reaches the request, so asking for
/// `target` outright lets the long edge satisfy it. A source already shorter than `target` matches
/// no scale and decodes in full, which is right.
fn short_edge_request(width: u16, height: u16, target: u32) -> u16 {
    let short = u32::from(width.min(height)).max(1);
    let long = u32::from(width.max(height));
    u16::try_from(target.max(1).saturating_mul(long).div_ceil(short)).unwrap_or(u16::MAX)
}

/// [`decode_capped`] for bytes already in hand, for the store's own normalizer.
pub fn decode_memory_capped(bytes: &[u8], max_dim: u32) -> Option<DynamicImage> {
    memory_reader(bytes, max_dim)?.decode().ok()
}

/// The dimensions from the header alone, the cheap half of the store's bound check. Also its
/// format check: a container `image` can't decode fails here, so nothing undrawable reaches disk.
pub fn memory_dimensions(bytes: &[u8], max_dim: u32) -> Option<(u32, u32)> {
    memory_reader(bytes, max_dim)?.into_dimensions().ok()
}

fn memory_reader(bytes: &[u8], max_dim: u32) -> Option<image::ImageReader<std::io::Cursor<&[u8]>>> {
    let mut reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(capped_limits(max_dim));
    Some(reader)
}

/// Resamples `src` to exactly `width` × `height` as RGB8, stretching to fit. **Takes a target and
/// computes none**: what shape a cover ends up is each caller's question, and so is `filter`.
/// **Blocking.**
pub fn resize_rgb8(
    src: &DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> Option<RgbImage> {
    let source: Cow<'_, RgbImage> = match src.as_rgb8() {
        Some(rgb) => Cow::Borrowed(rgb),
        None => Cow::Owned(src.to_rgb8()),
    };
    resize_rgb8_image(source, width, height, filter)
}

/// [`resize_rgb8`] for a caller already holding RGB8. A `Cow` so an equal-size request can hand an
/// owned source straight back.
pub fn resize_rgb8_image(
    source: Cow<'_, RgbImage>,
    width: u32,
    height: u32,
    filter: FilterType,
) -> Option<RgbImage> {
    // Not rare (a cover under the tier's tile, or over only the store's byte bound), and the
    // resizer would answer with a zeroed buffer and a row-by-row copy.
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

/// `source` as JPEG bytes at `quality`. Shared because both in-memory JPEG writers sit behind a
/// resize, and encoding differently would make the cover the store caches differ from the one
/// written into the user's file.
pub fn encode_jpeg(source: RgbImage, quality: u8) -> image::ImageResult<Vec<u8>> {
    let mut encoded = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality);
    DynamicImage::ImageRgb8(source).write_with_encoder(encoder)?;
    Ok(encoded)
}

/// Ceiling on the scratch a thread's resampler keeps between calls: it never shrinks its buffer, so
/// one outsized source would stay resident on that thread until quit. Twice what a stored cover
/// into the largest tile needs, because growth is `max(capacity * 2, required)`: any tighter and
/// two ordinary tiers in the wrong order trip the reset.
const RESIZER_SCRATCH_CAP: usize = 2 * 1024 * 1024;

thread_local! {
    /// Per thread rather than per cover, owning the convolution's scratch buffers.
    static RESIZER: RefCell<Resizer> = RefCell::new(Resizer::new());
}

/// `width` × `height` shrunk by one ratio until it fits `max_w` × `max_h`, never enlarged. One
/// ratio rather than a clamp per axis, so the aspect survives, which the backdrop band relies on.
#[must_use]
pub fn fit_within(width: u32, height: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    // The axis overflowing further sets the ratio, cross-multiplied so the comparison is exact.
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

/// Source pixel count from the header alone, cheap enough to ask before every decode. A forged
/// header lies here as it does to `decode_capped`, so callers use it to *order* work, never to
/// trust a size.
pub fn source_pixels(path: &Path) -> Option<u64> {
    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let (width, height) = reader.into_dimensions().ok()?;
    Some(u64::from(width) * u64::from(height))
}

/// Past this a decode waits on [`LARGE_DECODE_GATE`]: well above any real cover or station logo,
/// for the odd full-resolution original a tag carries or a host serves as a favicon.
const LARGE_SOURCE_PIXELS: u64 = 4_000_000;

/// Serializes oversized decodes across every pool that runs one. A pool bounds the peak at its
/// width × the largest source, and narrowing it for the rare huge one would slow every scan. One
/// gate rather than one per pool, because what it bounds is process RSS: the cover pool and
/// tokio's blocking pool would each cap only their own half.
static LARGE_DECODE_GATE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// `None` where `pixels` needs no gate. Take it from the header read before the decode
/// ([`source_pixels`], [`memory_dimensions`]) and hold it while any full-resolution buffer lives,
/// the flatten and re-encode included.
#[must_use]
pub fn large_decode_guard(pixels: u64) -> Option<parking_lot::MutexGuard<'static, ()>> {
    (pixels > LARGE_SOURCE_PIXELS).then(|| LARGE_DECODE_GATE.lock())
}

/// Decoder limits refusing anything wider or taller than `max_dim`. The rest stay at `image`'s
/// defaults, which already cap a single allocation.
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
