//! The cover + blur decode cache both artwork tiers are built from.
//!
//! Two surfaces want the active cover in two forms at once — a sharp tile and
//! a heavily-blurred backdrop — and both derive from the *same* source image,
//! so each decodes it **once** and produces both buffers from that single
//! `DynamicImage`, held as an [`ArtworkPair`] in a small LRU keyed by artwork
//! path. [`crate::ui::now_playing_artwork`] is the full-screen Now Playing
//! tier and [`crate::ui::detail_artwork`] the detail-header one; they differ
//! only in capacity and in the [`BlurSpec`] below, which is why the machinery
//! lives here and each of them is a newtype over it.
//!
//! Deliberately **separate, small** caches rather than a share of the row-tier
//! [`crate::media::cover_thumbs::CoverThumbs`]: mixing these much larger
//! buffers into that LRU would evict row thumbnails wholesale.
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
use crate::ui::util::{BLUR_TARGET, COVER_SIZE, buffer_from_rgb};

/// The blurred half's shape: [`BLUR_TARGET`] wide by `height`, softened at
/// `sigma`. The width is shared because a backdrop carries no fine detail
/// either way; the height and the sigma are what a surface tunes.
pub struct BlurSpec {
    /// Height the cover is downscaled to before blurring, against the shared
    /// [`BLUR_TARGET`] width. Square matches a square backdrop; a landscape
    /// band wants a landscape buffer, and squashing a square source into one
    /// is invisible after the blur.
    pub height: u32,
    /// `fast_blur` sigma. A backdrop with nothing painted over it wants the
    /// full wash; one sitting under a gradient floor and a solved scrim needs
    /// less of its own.
    pub sigma: f32,
}

/// A decoded artwork pair: the sharp foreground tile and the blurred
/// backdrop, both derived from one source decode.
#[derive(Clone)]
pub struct ArtworkPair {
    /// Sharp, aspect-preserved cover tile (≤ [`COVER_SIZE`] on its long edge).
    pub cover: SharedPixelBuffer<Rgb8Pixel>,
    /// Heavily-blurred backdrop, sized by the tier's [`BlurSpec`].
    pub blur: SharedPixelBuffer<Rgb8Pixel>,
    /// The hue and brightness of `blur` — everything
    /// [`crate::ui::backdrop::solve`] needs to colour the surface.
    pub(crate) sample: BackdropSample,
}

/// `None` records a previously-attempted decode that failed — cached so a
/// broken cover file isn't re-opened on every change.
pub type CachedArtwork = Option<ArtworkPair>;

/// A path-keyed LRU of `(cover, blur)` pairs at one blur shape.
pub struct ArtworkCache {
    cache: Mutex<LruCache<PathBuf, CachedArtwork>>,
    blur: BlurSpec,
}

impl ArtworkCache {
    pub fn new(capacity: NonZeroUsize, blur: BlurSpec) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(capacity)),
            blur,
        }
    }

    /// Cached lookup; decode the source **once** + derive both buffers +
    /// insert on miss. Returns the raw `SharedPixelBuffer` pair (both
    /// `Send`) so this can run inside `tokio::task::spawn_blocking` — the
    /// caller wraps each in a `slint::Image` on the UI thread via
    /// `Image::from_rgb8`.
    ///
    /// `None` means the cover failed to decode; the failure is cached so the
    /// same broken file isn't retried on every change. Hits promote the entry
    /// to most-recently-used.
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        // Fast path: hit promotes LRU position. `LruCache::get` needs
        // `&mut self`, hence the Mutex.
        if let Some(cached) = self.cache.lock().get(path) {
            return cached.clone();
        }
        // Miss: decode + blur without holding the lock so a concurrent
        // lookup isn't blocked behind the (slow) CPU work.
        let pair = decode_artwork(path, &self.blur);
        let returned = pair.clone();
        self.cache.lock().put(path.to_path_buf(), pair);
        returned
    }

    /// Drop every cached `(cover, blur)` pair. Called when the surface goes
    /// away — the heavy buffers aren't needed while it's hidden, and the
    /// caller pairs this with `heap_trim::trim()` so glibc hands the freed
    /// pages back to the OS. Re-opening re-decodes.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Whether `path` has an entry — a hit or a remembered failure.
    #[cfg(test)]
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.cache.lock().contains(path)
    }

    /// How many entries are resident.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.cache.lock().len()
    }
}

/// Decode `path` **once**, then derive both the sharp cover tile and the
/// blurred backdrop from that single `DynamicImage`.
fn decode_artwork(path: &Path, blur_spec: &BlurSpec) -> CachedArtwork {
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
    let small = decoded.thumbnail_exact(BLUR_TARGET, blur_spec.height).to_rgb8();
    let blur = buffer_from_rgb(&fast_blur(&small, blur_spec.sigma));

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
#[path = "tests/artwork_cache_tests.rs"]
mod tests;
