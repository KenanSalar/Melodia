//! The cover + blur decode cache both artwork tiers are built from.
//!
//! Each source is decoded once into a sharp tile and a blurred backdrop, held as an
//! [`ArtworkPair`] in a small path-keyed LRU. [`crate::ui::now_playing_artwork`] and
//! [`crate::ui::detail_artwork`] differ only in capacity and [`BlurSpec`], hence two newtypes.
//!
//! Separate from the row tier's `CoverThumbs`, whose thumbnails these much larger buffers would
//! evict wholesale. Buffers rather than `Image`, which can't cross the `spawn_blocking` it decodes
//! on.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use image::imageops::fast_blur;
use lru::LruCache;
use parking_lot::Mutex;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::backdrop::BackdropSample;
use crate::ui::util::{BLUR_TARGET, COVER_SIZE, buffer_from_rgb};
use melodia_artwork::media::image::decoded::Decoded;
use melodia_artwork::media::image::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped_to, fit_within, resize_rgb8,
};

/// The blurred half's shape: [`BLUR_TARGET`] wide by `height`, softened at `sigma`. A tier holds it
/// as an `Option`, `None` under the aurora, where building a blur is the cost the setting avoids.
#[derive(Clone, Copy)]
pub struct BlurSpec {
    /// A landscape band wants a landscape buffer; squashing a square source is invisible blurred.
    pub height: u32,
    /// `fast_blur` sigma. A backdrop under a floor and a scrim needs less than one under nothing.
    pub sigma: f32,
}

/// The sharp foreground tile and the blurred backdrop, both from one source decode.
#[derive(Clone)]
pub struct ArtworkPair {
    /// Aspect-preserved, at most [`COVER_SIZE`] on its long edge.
    pub cover: SharedPixelBuffer<Rgb8Pixel>,
    /// `None` when the tier has no spec.
    pub blur: Option<SharedPixelBuffer<Rgb8Pixel>>,
    pub(crate) sample: BackdropSample,
}

/// A lookup's answer, `None` for a cover that won't decode or isn't there.
pub type CachedArtwork = Option<ArtworkPair>;

/// A path-keyed LRU of `(cover, blur)` pairs at one blur shape.
pub struct ArtworkCache {
    cache: Mutex<LruCache<PathBuf, Decoded<ArtworkPair>>>,
    blur: Option<BlurSpec>,
}

impl ArtworkCache {
    pub fn new(capacity: NonZeroUsize, blur: Option<BlurSpec>) -> Self {
        Self { cache: Mutex::new(LruCache::new(capacity)), blur }
    }

    /// Cached lookup, decoding once on a miss. Returns `Send` buffers so it can run inside
    /// `spawn_blocking`; the caller wraps each in an `Image` on the UI thread.
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        if let Some(cached) = self.cache.lock().get(path).filter(|held| held.is_current(path)) {
            return cached.ready().cloned();
        }
        // Outside the lock, so a concurrent lookup isn't parked behind the CPU work.
        let pair =
            decode_artwork(path, self.blur).map_or_else(|| Decoded::failed(path), Decoded::Ready);
        let returned = pair.ready().cloned();
        self.cache.lock().put(path.to_path_buf(), pair);
        returned
    }

    /// Drops every cached pair. The caller pairs this with `allocator::trim()`.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Whether `path` has an entry, a hit or a remembered failure.
    #[cfg(test)]
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.cache.lock().contains(path)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.cache.lock().len()
    }
}

fn decode_artwork(path: &Path, blur_spec: Option<BlurSpec>) -> CachedArtwork {
    // `COVER_SIZE`, the tile being the larger of the two halves this one decode has to cover.
    pair_from_image(&decode_capped_to(path, MAX_SOURCE_DIM, COVER_SIZE).ok()?, blur_spec)
}

/// Both halves plus the measurement from an image in hand, for the curated heroes' composed
/// collage.
///
/// With no spec the seeds still come, the aurora wanting them, but off the cover tile rather than a
/// second downscale that existed only to make `fast_blur` cheap, and with no brightness.
pub(crate) fn pair_from_image(
    decoded: &DynamicImage,
    blur_spec: Option<BlurSpec>,
) -> Option<ArtworkPair> {
    // Aspect-preserving; the Slint side's `image-fit: cover` crops it to the square tile.
    let (cover_w, cover_h) = fit_within(decoded.width(), decoded.height(), COVER_SIZE, COVER_SIZE);
    let cover = buffer_from_rgb(&resize_rgb8(decoded, cover_w, cover_h, FilterType::Box)?);

    let Some(spec) = blur_spec else {
        let sample = BackdropSample::quantize(cover.as_bytes());
        return Some(ArtworkPair { cover, blur: None, sample });
    };

    // Downscaled hard so the blur is cheap, squashing a square cover into the band on purpose: it
    // is invisible once blurred, and the 3-pass box blur indistinguishable from a Gaussian.
    let (band_w, band_h) = fit_within(BLUR_TARGET, spec.height, decoded.width(), decoded.height());
    let small = resize_rgb8(decoded, band_w, band_h, FilterType::Box)?;
    let blur = buffer_from_rgb(&fast_blur(&small, spec.sigma));

    // Here rather than at the publisher, so it is paid once per cover rather than once per open.
    let sample = BackdropSample::measure(small.as_raw(), blur.as_bytes());

    Some(ArtworkPair { cover, blur: Some(blur), sample })
}

#[cfg(test)]
#[path = "tests/artwork_cache_tests.rs"]
mod tests;
