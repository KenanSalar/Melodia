//! The shared path-keyed thumbnail cache every track row draws through.
//!
//! `slint::Image::load_from_path` decodes eagerly and never caches by path, so an `Image` per row
//! is one full-resolution decode per *track* rather than per cover. This puts a bounded cache in
//! front of it: deduped by path, downscaled to `thumb_size` square, kept as **RGB8** (album art is
//! overwhelmingly alpha-free, and `FemtoVG` converts on upload rather than per draw), evicted LRU,
//! and decoded under `image::Limits` so a forged dimension header can't allocate gigabytes.
//!
//! **`thumb_size` is per-instance.** Views draw artwork at wildly different sizes and `FemtoVG`
//! minifies with plain bilinear and no mipmaps, so one size either softens the big tiles or wastes
//! memory on the small ones; mixing grid-sized buffers into the row tier's LRU would also evict
//! row thumbnails wholesale.
//!
//! Buffers are cached rather than `Image` because `slint::Image` is deliberately neither `Send`
//! nor `Sync`, so it can neither live in a cross-thread cache nor come out of a Rayon pipeline.
//! `SharedPixelBuffer<Rgb8Pixel>` is both, and refcounted.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use rayon::prelude::*;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

use super::image_decode::{MAX_SOURCE_DIM, decode_capped, source_pixels};

/// Row-tier thumbnail size at a 1× display — just over the now-playing bar's tile, the larger of
/// the tier's two consumers.
const ROW_THUMB_SIZE: u32 = 48;

/// The row tier on a `HiDPI` display.
const ROW_THUMB_SIZE_HIDPI: u32 = 72;

/// Row-tier decode size for a display at `scale`. Split rather than fixed at the `HiDPI` value
/// because a 1× display is the common case and would otherwise pay for pixels it never draws. The
/// threshold sits below 1.5 so a fractional-scale desktop rounds *up* — softness is the worse of
/// the two failures.
pub fn row_cover_size(scale: f64) -> u32 {
    if scale > 1.25 {
        ROW_THUMB_SIZE_HIDPI
    } else {
        ROW_THUMB_SIZE
    }
}

/// Source pixel count past which a decode waits its turn on [`LARGE_DECODE_GATE`] — well above any
/// real cover, for the occasional full-resolution original a user's tags carry.
const LARGE_SOURCE_PIXELS: u64 = 4_000_000;

/// Serializes oversized decodes against each other.
///
/// [`DECODE_POOL`] bounds the transient peak at *pool width* × the largest source, the wrong shape
/// when one cover is enormous and the rest are thumbnails: narrowing the pool for the rare huge
/// one would cost every scan. Gating on the header-read size leaves the common path untouched and
/// caps the peak at one full-resolution bitmap.
static LARGE_DECODE_GATE: Mutex<()> = Mutex::new(());

/// Row-tier LRU capacity, counting unique *covers* rather than tracks, so it scales with library
/// size. Past the cap, eviction means a scroll-back re-decodes one thumbnail inline.
const CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(512) {
    Some(n) => n,
    None => panic!("CACHE_CAP > 0"),
};

/// `None` is a decode that failed — cached too, so refilters don't keep re-hitting the same broken
/// file.
type CachedBuf = Option<SharedPixelBuffer<Rgb8Pixel>>;

/// Bounded Rayon pool for [`CoverThumbs::prewarm`]. Each decode briefly holds a full-resolution
/// `DynamicImage`, so fanning across the `num_cpus`-wide global pool would let that many coexist
/// at the peak; a small dedicated one bounds that *and* isolates the burst from the library
/// scanner. `None` if it fails to build, in which case `prewarm` falls back to the global pool.
static DECODE_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// Sized to half the logical cores, clamped — the knob trading prewarm throughput against the
/// transient decode peak.
fn decode_pool() -> Option<&'static rayon::ThreadPool> {
    DECODE_POOL
        .get_or_init(|| {
            let threads =
                std::thread::available_parallelism().map_or(2, |p| (p.get() / 2).clamp(2, 4));
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("cover-decode-{i}"))
                .build()
                .ok()
        })
        .as_ref()
}

pub struct CoverThumbs {
    cache: Mutex<LruCache<PathBuf, CachedBuf>>,
    /// Side length every cover in this cache is downscaled to. Atomic because the tiers are held
    /// behind `Arc` and [`Self::set_thumb_size`] retunes them once the scale factor is known.
    thumb_size: AtomicU32,
}

impl Default for CoverThumbs {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(CACHE_CAP)),
            thumb_size: AtomicU32::new(ROW_THUMB_SIZE),
        }
    }
}

impl CoverThumbs {
    /// The shared row tier, behind every track table and the now-playing bar. The queue sheet
    /// keeps its own private instance, released on close.
    pub fn new() -> Self {
        Self::default()
    }

    /// A tier with a caller-chosen thumbnail size and capacity, for views drawing artwork far
    /// larger than a row tile. Capacity is retunable via [`Self::resize`].
    pub fn with_config(thumb_size: u32, cache_cap: NonZeroUsize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(cache_cap)),
            thumb_size: AtomicU32::new(thumb_size),
        }
    }

    /// Drop every cached buffer, for a per-view tier released on section leave. Callers pair this
    /// with `heap_trim::trim()` so glibc hands the freed pages back to the OS.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Retune the LRU capacity in place, once the real display size is known. Shrinking evicts
    /// down to the new cap.
    pub fn resize(&self, cache_cap: NonZeroUsize) {
        self.cache.lock().resize(cache_cap);
    }

    /// Retune the decode size in place, alongside [`Self::resize`]. Everything already cached was
    /// decoded at the old size, so a genuine change drops it — free at the one call site, which
    /// runs before any view has fetched. Retunable rather than fixed at construction because a
    /// tier is built inside a view's `new`, where the scale factor isn't in hand.
    pub fn set_thumb_size(&self, thumb_size: u32) {
        if self.thumb_size.swap(thumb_size, Ordering::Relaxed) == thumb_size {
            return;
        }
        self.cache.lock().clear();
    }

    /// Current LRU capacity. A `prewarm` caller building a display-ordered path list can
    /// `.take(capacity())` so it never allocates a `Vec` longer than the cache can hold —
    /// `prewarm` caps its own decode work at the same number.
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Cached lookup, decoding and inserting on miss. A decode that fails is remembered, so a
    /// refilter doesn't retry it. Safe from any thread, but the returned `Image` is not `Send`.
    pub fn get_or_load(&self, path: &Path) -> Image {
        // `LruCache::get` takes `&mut self` (a hit promotes), hence the mutex.
        if let Some(maybe_buf) = self.cache.lock().get(path) {
            return buf_to_image(maybe_buf);
        }
        // Decode off the lock so other threads can keep reading the cache.
        let buf = decode_thumb_buffer(path, self.thumb_size.load(Ordering::Relaxed));
        let img = buf_to_image(&buf);
        self.cache.lock().put(path.to_path_buf(), buf);
        img
    }

    /// [`Self::get_or_load`] for the common "row holds an `Option<String>` artwork path" shape;
    /// `None` or `Some("")` is the empty [`Image`].
    pub fn get_or_load_opt(&self, path: Option<&str>) -> Image {
        match path.filter(|p| !p.is_empty()) {
            Some(p) => self.get_or_load(Path::new(p)),
            None => Image::default(),
        }
    }

    /// Cache-only lookup — **never** decodes synchronously, serving the placeholder on a miss.
    ///
    /// For a surface that mounts rows *before* its tier is warm, which here is the queue sheet
    /// alone: its rows must land in the model before `on_open_changed` returns so the slide-up has
    /// text on frame one, and a per-row [`Self::get_or_load_opt`] there would block the UI thread
    /// on a screenful of decodes, freezing the very animation the synchronous build exists to
    /// feed. The sheet swaps to the decoding lookup once its off-thread [`Self::prewarm`] lands.
    pub fn get_cached_opt(&self, path: Option<&str>) -> Image {
        let Some(p) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        self.cache.lock().get(Path::new(p)).map_or_else(Image::default, buf_to_image)
    }

    /// [`Self::get_or_load`]'s contract over the raw buffer rather than a [`slint::Image`].
    /// Material You seeds its palette from the already-decoded thumbnail this way, rather than
    /// decoding the full-resolution artwork a second time.
    pub fn get_or_load_rgb8(&self, path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        if let Some(maybe_buf) = self.cache.lock().get(path) {
            return maybe_buf.clone();
        }
        let buf = decode_thumb_buffer(path, self.thumb_size.load(Ordering::Relaxed));
        let returned = buf.clone();
        self.cache.lock().put(path.to_path_buf(), buf);
        returned
    }

    /// Decode every uncached path in `paths` in parallel and populate the cache. Runs inside
    /// `spawn_blocking` so the CPU-bound work stays off the async worker pool.
    ///
    /// The pre-check is the **non-promoting** `contains`, so a prewarm can't reorder paths already
    /// hot from earlier views. Duplicates are deduped here rather than at call sites — a caller
    /// passing one path per *track* must not trigger one decode per duplicate.
    ///
    /// Work is capped at the LRU capacity, since decoding more unique paths than the cache holds
    /// evicts the earliest with the latest. **Pass paths in display order** so the kept prefix is
    /// the visible one.
    pub fn prewarm(&self, paths: &[PathBuf]) {
        let missing: Vec<PathBuf> = {
            let cache = self.cache.lock();
            let cap = cache.cap().get();
            let mut seen = std::collections::HashSet::with_capacity(paths.len().min(cap));
            paths
                .iter()
                .filter(|p| !cache.contains(*p) && seen.insert(*p))
                .take(cap)
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        // Hoisted so the Rayon closure captures a plain `u32` rather than `self`.
        let thumb_size = self.thumb_size.load(Ordering::Relaxed);
        let decode_all = move || {
            missing
                .into_par_iter()
                .map(|p| {
                    let buf = decode_thumb_buffer(&p, thumb_size);
                    (p, buf)
                })
                .collect::<Vec<(PathBuf, CachedBuf)>>()
        };
        let decoded: Vec<(PathBuf, CachedBuf)> = match decode_pool() {
            Some(pool) => pool.install(decode_all),
            None => decode_all(),
        };
        let mut cache = self.cache.lock();
        for (p, buf) in decoded {
            // A `get_or_load` racing between the filter above and this reacquire could have
            // inserted the same key.
            if !cache.contains(&p) {
                cache.put(p, buf);
            }
        }
    }
}

fn buf_to_image(buf: &CachedBuf) -> Image {
    buf.as_ref().map(|b| Image::from_rgb8(b.clone())).unwrap_or_default()
}

fn decode_thumb_buffer(path: &Path, thumb_size: u32) -> CachedBuf {
    // Held for the decode only — see `LARGE_DECODE_GATE`. A header the probe can't read leaves the
    // decode ungated, which is no worse than having no gate.
    let _oversized = source_pixels(path)
        .is_some_and(|pixels| pixels > LARGE_SOURCE_PIXELS)
        .then(|| LARGE_DECODE_GATE.lock());

    let dyn_img = decode_capped(path, MAX_SOURCE_DIM).ok()?;

    // `thumbnail_exact` is integer-only and doesn't preserve aspect, which is moot for
    // overwhelmingly square album art — and `image-fit: cover` on the Slint side would have
    // re-cropped a non-square one anyway. Far cheaper than `resize_to_fill`.
    let thumb = dyn_img.thumbnail_exact(thumb_size, thumb_size).to_rgb8();
    let (w, h) = thumb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(thumb.as_raw());
    Some(buf)
}

#[cfg(test)]
#[path = "tests/cover_thumbs_tests.rs"]
mod tests;
