//! The cover + blur decode cache both artwork tiers are built from.
//!
//! Two surfaces want the active cover in two forms at once — a sharp tile and a
//! heavily-blurred backdrop — and both derive from the *same* source image, so each
//! decodes it **once** and produces both buffers from that single `DynamicImage`, held as
//! an [`ArtworkPair`] in a small path-keyed LRU. [`crate::ui::now_playing_artwork`] and
//! [`crate::ui::detail_artwork`] differ only in capacity and [`BlurSpec`], which is why
//! each is a newtype over the machinery here.
//!
//! Deliberately **separate, small** caches rather than a share of the row-tier
//! [`crate::media::cover_thumbs::CoverThumbs`]: mixing these much larger buffers into that
//! LRU would evict row thumbnails wholesale. Buffers rather than `Image`, `slint::Image`
//! being neither `Send` nor `Sync` and so unable to cross the `spawn_blocking` boundary
//! the decode runs on.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use image::imageops::fast_blur;
use lru::LruCache;
use parking_lot::Mutex;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};
use crate::ui::backdrop::BackdropSample;
use crate::ui::util::{BLUR_TARGET, COVER_SIZE, buffer_from_rgb};

/// The blurred half's shape: [`BLUR_TARGET`] wide by `height`, softened at `sigma`. The
/// width is shared, a backdrop carrying no fine detail either way.
///
/// A tier holds it as an `Option` — `None` under the aurora setting, where nothing paints a
/// blur and building one would be the whole cost the setting exists to avoid.
#[derive(Clone, Copy)]
pub struct BlurSpec {
    /// Height the cover is downscaled to before blurring. A landscape band wants a
    /// landscape buffer, and squashing a square source into one is invisible once blurred.
    pub height: u32,
    /// `fast_blur` sigma. A backdrop with nothing painted over it wants the full wash;
    /// one under a gradient floor and a solved scrim needs less of its own.
    pub sigma: f32,
}

/// The sharp foreground tile and the blurred backdrop, both from one source decode.
#[derive(Clone)]
pub struct ArtworkPair {
    /// Sharp, aspect-preserved cover tile (≤ [`COVER_SIZE`] on its long edge).
    pub cover: SharedPixelBuffer<Rgb8Pixel>,
    /// Heavily-blurred backdrop, sized by the tier's [`BlurSpec`]. `None` when the tier has
    /// no spec, the aurora being mounted instead.
    pub blur: Option<SharedPixelBuffer<Rgb8Pixel>>,
    /// What the cover quantized to, and — on the blur arm alone — how bright it is. Everything
    /// the mounted backdrop needs to colour its surface.
    pub(crate) sample: BackdropSample,
}

/// `None` records a previously-attempted decode that failed — cached so a
/// broken cover file isn't re-opened on every change.
pub type CachedArtwork = Option<ArtworkPair>;

/// A path-keyed LRU of `(cover, blur)` pairs at one blur shape.
pub struct ArtworkCache {
    cache: Mutex<LruCache<PathBuf, CachedArtwork>>,
    blur: Option<BlurSpec>,
}

impl ArtworkCache {
    pub fn new(capacity: NonZeroUsize, blur: Option<BlurSpec>) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(capacity)),
            blur,
        }
    }

    /// Cached lookup, decoding the source **once** and deriving both buffers on a miss.
    /// Hands back the raw `SharedPixelBuffer` pair, both `Send`, so this can run inside
    /// `spawn_blocking`; the caller wraps each in a `slint::Image` on the UI thread.
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        // `LruCache::get` needs `&mut self`, hence the Mutex.
        if let Some(cached) = self.cache.lock().get(path) {
            return cached.clone();
        }
        // Decode and blur without holding the lock, so a concurrent lookup isn't parked
        // behind the CPU work.
        let pair = decode_artwork(path, self.blur);
        let returned = pair.clone();
        self.cache.lock().put(path.to_path_buf(), pair);
        returned
    }

    /// Drop every cached pair when the surface goes away. The caller pairs this with
    /// `heap_trim::trim()` so glibc hands the freed pages back; re-opening re-decodes.
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

/// Decode `path` **once**, then derive both halves from that single `DynamicImage`.
fn decode_artwork(path: &Path, blur_spec: Option<BlurSpec>) -> CachedArtwork {
    Some(pair_from_image(&decode_capped(path, MAX_SOURCE_DIM).ok()?, blur_spec))
}

/// Both halves plus the measurement, from an image already in hand.
///
/// Split out for the curated heroes, whose source is a composed collage rather than a
/// file — nothing about the tiers they publish into differs, so nothing about how the
/// pair is derived should either.
///
/// The **seeds survive a `None` spec** — the aurora is what wants them — but nothing else of the
/// blur's preamble does. That second downscale exists to make `fast_blur` cheap, so with no blur
/// to make cheap the quantizer reads the cover tile already in hand and takes no brightness, the
/// aurora solving against none. The tile is the better sample besides, being sharp and
/// aspect-preserved where the band's buffer is squashed to a landscape one.
pub(crate) fn pair_from_image(decoded: &DynamicImage, blur_spec: Option<BlurSpec>) -> ArtworkPair {
    // `thumbnail`, not `thumbnail_exact`: aspect-preserving, so a non-square cover keeps
    // its ratio and the Slint side's `image-fit: cover` crops it to the square tile.
    let cover = buffer_from_rgb(&decoded.thumbnail(COVER_SIZE, COVER_SIZE).to_rgb8());

    let Some(spec) = blur_spec else {
        let sample = BackdropSample::quantize(&cover);
        return ArtworkPair {
            cover,
            blur: None,
            sample,
        };
    };

    // Downscale hard first, so the blur is cheap. `thumbnail_exact` is the integer-only
    // fast path and its aspect distortion is invisible once blurred and re-cropped;
    // `fast_blur`'s 3-pass box blur is indistinguishable from a true Gaussian here.
    let small = decoded.thumbnail_exact(BLUR_TARGET, spec.height).to_rgb8();

    // Off the *sharp* downscale, never the blur — see [`BackdropSample::quantize`]. Here rather
    // than at the publisher: this runs on the blocking pool and is cached, so the quantize is
    // paid once per cover rather than once per open.
    let sample = BackdropSample::measure(&buffer_from_rgb(&small));

    ArtworkPair {
        cover,
        blur: Some(buffer_from_rgb(&fast_blur(&small, spec.sigma))),
        sample,
    }
}

#[cfg(test)]
#[path = "tests/artwork_cache_tests.rs"]
mod tests;
