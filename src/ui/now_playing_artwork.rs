//! Album-art decode + cache for the full-screen Now Playing view.
//!
//! The view needs the active cover in two forms — a sharp tile and a
//! heavily-blurred backdrop. Both derive from the *same* source image, so
//! this module decodes it **once** per track and produces both buffers from
//! that single `DynamicImage`, held as an [`ArtworkPair`] in one small LRU
//! keyed by artwork path.
//!
//! Deliberately a **separate, small** cache from the row-tier
//! [`crate::media::cover_thumbs::CoverThumbs`]: mixing these much larger
//! buffers into that LRU would evict row thumbnails wholesale. The working
//! set here is the active track plus a handful of neighbours, so
//! [`ARTWORK_CACHE_CAP`] is small.
//!
//! Caches buffers rather than `Image` because `slint::Image` is not
//! `Send`/`Sync` and so can't cross the `spawn_blocking` boundary the decode
//! runs on; the caller builds both `Image`s on the UI thread.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use image::imageops::fast_blur;
use lru::LruCache;
use parking_lot::Mutex;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::util::buffer_from_rgb;

/// Side length (px) the sharp cover tile is downscaled to. Roughly matches
/// the 380 px maximum on-screen tile, so it neither upscales nor pays for a
/// 2× `HiDPI` buffer. Matches the Album Detail header tier so every
/// large-tile surface decodes at one size.
const COVER_SIZE: u32 = 384;

/// Side length the cover is downscaled to before blurring. The backdrop
/// carries no fine detail and is stretched to full-window `ImageFit.cover`,
/// and downscaling first makes the blur cheap — box-pass cost scales with
/// pixel count.
const BLUR_DOWNSCALE: u32 = 192;

/// `fast_blur` sigma. At [`BLUR_DOWNSCALE`] this reads as a soft wash of
/// colour with no recognisable shapes.
const BLUR_SIGMA: f32 = 24.0;

/// Hard cap on accepted source resolution — mirrors `CoverThumbs`'s guard
/// so a forged dimension header in a tag can't trigger an absurd
/// allocation before we get a chance to downscale.
const MAX_SOURCE_DIM: u32 = 8192;

/// LRU capacity. "Up Next" surfaces ~20 tracks and the user skips through
/// neighbours, so too small a cap thrashes on exactly the interaction the
/// feature exists for; this covers the realistic working set while keeping
/// the resident `(cover, blur)` pairs bounded.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// A decoded artwork pair: the sharp foreground cover tile and the blurred
/// backdrop, both derived from one source decode.
#[derive(Clone)]
pub struct ArtworkPair {
    /// Sharp, aspect-preserved cover tile (≤ `COVER_SIZE` on its long edge).
    pub cover: SharedPixelBuffer<Rgb8Pixel>,
    /// Heavily-blurred `BLUR_DOWNSCALE`-square backdrop.
    pub blur: SharedPixelBuffer<Rgb8Pixel>,
    /// Dominant accent extracted via `material_you::extract_source_argb_from_rgb8`
    /// from the blur buffer (192² is plenty of pixels for `QuantizerCelebi`
    /// and re-quantizing the sharp tile would burn ~4× more CPU for no
    /// perceptual gain). Supplies the *hue* for every colour the Now Playing
    /// view solves in [`crate::ui::now_playing::backdrop`].
    pub accent_argb: Option<u32>,
    /// 90th-percentile lightness (L*) of `blur` — how bright the backdrop
    /// actually is, and the input the scrim opacity is solved from. See
    /// [`crate::ui::now_playing::backdrop::luma_p90`] for why it's a
    /// percentile and not a mean. `None` only for an empty buffer.
    pub backdrop_luma: Option<f64>,
}

/// `None` records a previously-attempted decode that failed — cached so a
/// broken cover file isn't re-opened on every track change.
type CachedArtwork = Option<ArtworkPair>;

pub struct NowPlayingArtwork {
    cache: Mutex<LruCache<PathBuf, CachedArtwork>>,
}

impl Default for NowPlayingArtwork {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(ARTWORK_CACHE_CAP)),
        }
    }
}

impl NowPlayingArtwork {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached lookup; decode the source **once** + derive both buffers +
    /// insert on miss. Returns the raw `SharedPixelBuffer` pair (both
    /// `Send`) so this can run inside `tokio::task::spawn_blocking` — the
    /// caller wraps each in a `slint::Image` on the UI thread via
    /// `Image::from_rgb8`.
    ///
    /// `None` means the cover failed to decode; the failure is cached so
    /// the same broken file isn't retried on every track change. Hits
    /// promote the entry to most-recently-used.
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        // Fast path: hit promotes LRU position. `LruCache::get` needs
        // `&mut self`, hence the Mutex.
        if let Some(cached) = self.cache.lock().get(path) {
            return cached.clone();
        }
        // Miss: decode + blur without holding the lock so a concurrent
        // lookup isn't blocked behind the (slow) CPU work.
        let pair = decode_artwork(path);
        let returned = pair.clone();
        self.cache.lock().put(path.to_path_buf(), pair);
        returned
    }

    /// Drop every cached `(cover, blur)` pair. Called when the Now Playing
    /// view closes — the heavy buffers aren't needed while it's hidden, and
    /// the caller pairs this with `heap_trim::trim()` so glibc hands the
    /// freed pages back to the OS. Re-opening re-decodes the active cover.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }
}

/// Decode `path` **once**, then derive both the sharp cover tile and the
/// blurred backdrop from that single `DynamicImage`.
fn decode_artwork(path: &Path) -> CachedArtwork {
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);

    let decoded = reader.decode().ok()?;

    // Sharp cover tile: `thumbnail` (not `thumbnail_exact`) preserves aspect
    // ratio and fits the image inside `COVER_SIZE × COVER_SIZE`. A non-square
    // cover yields e.g. a `640 × 600` buffer; the Slint side's
    // `image-fit: cover` crops it to the square tile.
    let cover = buffer_from_rgb(&decoded.thumbnail(COVER_SIZE, COVER_SIZE).to_rgb8());

    // Blurred backdrop: downscale hard first (cheap blur, tiny buffer), then
    // an approximate Gaussian. `thumbnail_exact` is the integer-only fast
    // downscale — album art is overwhelmingly square and any minor aspect
    // distortion is invisible once blurred and re-cropped by `image-fit:
    // cover`. `fast_blur` is a 3-pass box blur — much cheaper than
    // `imageops::blur`'s true Gaussian and indistinguishable at this scale
    // for a backdrop.
    let small = decoded
        .thumbnail_exact(BLUR_DOWNSCALE, BLUR_DOWNSCALE)
        .to_rgb8();
    let blur = buffer_from_rgb(&fast_blur(&small, BLUR_SIGMA));

    // Both statistics come off the same buffer in one place: the quantize the
    // hue is scored from, and the percentile the scrim is sized from. The
    // percentile pass is linear over ~110 KiB — noise beside the quantize.
    let accent_argb = crate::services::material_you::extract_source_argb_from_rgb8(&blur);
    let backdrop_luma = crate::ui::now_playing::backdrop::luma_p90(&blur);

    Some(ArtworkPair {
        cover,
        blur,
        accent_argb,
        backdrop_luma,
    })
}

#[cfg(test)]
#[path = "tests/now_playing_artwork_tests.rs"]
mod tests;
