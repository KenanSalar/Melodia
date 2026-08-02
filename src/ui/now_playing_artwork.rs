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

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};
use crate::ui::backdrop::BackdropSample;
use crate::ui::util::{BLUR_SIGMA, BLUR_TARGET, COVER_SIZE, buffer_from_rgb};

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
    /// Heavily-blurred `BLUR_TARGET`-square backdrop.
    pub blur: SharedPixelBuffer<Rgb8Pixel>,
    /// The hue and brightness of `blur` — everything
    /// [`crate::ui::backdrop::solve`] needs to colour the view.
    pub(crate) sample: BackdropSample,
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
    let decoded = decode_capped(path, MAX_SOURCE_DIM).ok()?;

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
    let small = decoded.thumbnail_exact(BLUR_TARGET, BLUR_TARGET).to_rgb8();
    let blur = buffer_from_rgb(&fast_blur(&small, BLUR_SIGMA));

    // Measured here rather than at the publisher: this runs on the blocking
    // pool and the result is cached, so the quantize is paid once per cover
    // instead of once per open.
    let sample = BackdropSample::measure(&blur);

    Some(ArtworkPair {
        cover,
        blur,
        sample,
    })
}

#[cfg(test)]
#[path = "tests/now_playing_artwork_tests.rs"]
mod tests;
