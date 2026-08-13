//! Decoded cover thumbnails for the Tracks view.
//!
//! `slint::Image::load_from_path` decodes eagerly and never caches by path —
//! every call re-reads the file and produces a fresh full-resolution RGBA
//! bitmap, so a fresh `Image` per row is one full decode per *track* rather
//! than per cover. This module puts a bounded cache in front of that:
//!
//! 1. **Dedup by path** — many tracks share one album cover, so a hit is a
//!    refcount bump on the same `SharedPixelBuffer`.
//! 2. **Thumbnail downscale** — each unique cover is decoded once and
//!    reduced to `thumb_size` square before it reaches Slint.
//! 3. **RGB8, not RGBA8** — album art is overwhelmingly alpha-free, so the
//!    fourth channel is dead weight. `FemtoVG` converts on upload, once per
//!    cover rather than per draw.
//! 4. **LRU eviction** — without a cap, a catalogue with thousands of
//!    unique covers grows the cache forever.
//! 5. **Decoder limits** — `image::Limits` bounds the source resolution, so
//!    a forged dimension header can't allocate gigabytes.
//!
//! [`CoverThumbs::prewarm`] decodes a batch in parallel (Rayon, inside
//! `spawn_blocking`) before the model is built, so per-row lookups are hits.
//!
//! ## Sizing rationale
//!
//! `thumb_size` is **per-instance**, not global — views display artwork at
//! wildly different sizes, and one size either softens the big tiles or
//! wastes memory on the small ones. `FemtoVG` minifies with plain bilinear and
//! no mipmaps, so each tier is sized near its on-screen size:
//!
//!   * **Row tier** ([`CoverThumbs::new`], [`THUMB_SIZE`]) — the 36 px
//!     track-row tile and the `clamp(bar-w * 0.07, 36px, 46px)`
//!     now-playing-bar tile.
//!   * **Album tiers** ([`CoverThumbs::with_config`]) — the Albums grid
//!     cards (flex-filled well past 280 px on wide panels) and the Album
//!     Detail header tile. `src/ui/albums/` owns an instance per tier;
//!     mixing those larger buffers into the row-tier LRU would evict row
//!     thumbnails wholesale. Same separation as
//!     [`crate::ui::now_playing_artwork::NowPlayingArtwork`].
//!
//! Buffers are cached rather than `Image` because `slint::Image` holds a
//! `*mut ()` internally and is deliberately neither `Send` nor `Sync`, so it
//! can neither live in a cross-thread cache nor come out of a Rayon
//! pipeline. `SharedPixelBuffer<Rgb8Pixel>` is both, and refcounted — the UI
//! thread wraps a clone via `Image::from_rgb8` when building rows.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use rayon::prelude::*;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

use super::image_decode::{MAX_SOURCE_DIM, decode_capped, source_pixels};

/// Default ("row tier") square thumbnail size, in pixels, at a 1× display —
/// just over the 46 px now-playing-bar tile, the larger of the two consumers
/// (the track row's is 36 px). Larger tiers ([`CoverThumbs::with_config`])
/// pass their own size; see the module docs' "Sizing rationale".
const ROW_THUMB_SIZE: u32 = 48;

/// The row tier on a `HiDPI` display — 2× the row tile, 1.56× the bar tile.
const ROW_THUMB_SIZE_HIDPI: u32 = 72;

/// Row-tier decode size for a display at `scale`.
///
/// Split rather than fixed at the `HiDPI` value because a 1× display is the
/// common case and was paying 2.25× the pixels for a tile it draws at 46 px.
/// The threshold sits below 1.5 so a fractional-scale desktop rounds up to the
/// sharp tier — softness is the worse failure of the two.
pub fn row_cover_size(scale: f64) -> u32 {
    if scale > 1.25 {
        ROW_THUMB_SIZE_HIDPI
    } else {
        ROW_THUMB_SIZE
    }
}

/// Source pixel count past which a decode waits its turn on
/// [`LARGE_DECODE_GATE`]. Well above any real cover (a 2000² square is 4 Mpx);
/// this is for the occasional full-resolution original a user's tags carry.
const LARGE_SOURCE_PIXELS: u64 = 4_000_000;

/// Serializes oversized decodes against each other.
///
/// [`DECODE_POOL`] bounds the transient peak at *pool width* × the largest
/// source, which is the wrong shape when one cover is enormous and the rest are
/// thumbnails: the pool exists to keep small decodes parallel, so narrowing it
/// for the rare huge one would cost every scan. Gating on the header-read size
/// instead leaves the common path untouched and caps the peak at one
/// full-resolution bitmap. Uncontended in every library that has no such cover.
static LARGE_DECODE_GATE: Mutex<()> = Mutex::new(());

/// Maximum entries kept in the row-tier cache — counts unique *covers*, not
/// tracks, so it scales with library size rather than queue length. Most
/// libraries sit well under it; past the cap, eviction just means a
/// scroll-back re-decodes one thumbnail inline, with no visible flash.
const CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(512) {
    Some(n) => n,
    None => panic!("CACHE_CAP > 0"),
};

/// `None` represents a previously-attempted decode that failed — we cache
/// failures too so refilters don't keep re-hitting the same broken file.
type CachedBuf = Option<SharedPixelBuffer<Rgb8Pixel>>;

/// Dedicated, bounded Rayon pool for [`CoverThumbs::prewarm`]'s parallel
/// decode. Each decode briefly holds a full-resolution `DynamicImage` before
/// downscaling, so fanning across the `num_cpus`-wide global pool would let
/// that many full-res bitmaps coexist at the peak. A small dedicated pool
/// bounds that *and* isolates the burst from the library scanner's use of
/// the global pool. `None` if the pool fails to build — `prewarm` then falls
/// back to the global pool.
static DECODE_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// The bounded decode pool, lazily built. Sized to half the logical cores,
/// clamped to 2–4 — the knob trading prewarm throughput against the
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
    /// Side length every cover in this cache is downscaled to — one instance is
    /// one size tier (see module docs). Atomic rather than plain because the
    /// tiers are held behind `Arc` and [`Self::set_thumb_size`] retunes them
    /// once the display's scale factor is known.
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
    /// Row-tier cache ([`ROW_THUMB_SIZE`], [`CACHE_CAP`]). Shared by the Tracks
    /// / Browse views and the now-playing bar; the queue sheet keeps its own
    /// private instance, released on close.
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache tier with a caller-chosen thumbnail size and LRU capacity, for
    /// views displaying artwork far larger than the row tile (the Albums grid
    /// and detail header). See the module docs' "Sizing rationale" for why
    /// each size gets its own instance. Capacity is retunable via
    /// [`Self::resize`].
    pub fn with_config(thumb_size: u32, cache_cap: NonZeroUsize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(cache_cap)),
            thumb_size: AtomicU32::new(thumb_size),
        }
    }

    /// Drop every cached buffer. Used when a per-view cache (e.g. the
    /// Albums grid's album-tier instance) is released because the view is
    /// no longer on screen — callers pair this with `heap_trim::trim()` so
    /// glibc returns the freed pages to the OS rather than retaining them.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Retune the LRU capacity in place — used once the real display size
    /// is known (see `albums::tune_cache_for_display`). Shrinking evicts
    /// the least-recently-used entries down to the new cap.
    pub fn resize(&self, cache_cap: NonZeroUsize) {
        self.cache.lock().resize(cache_cap);
    }

    /// Retune the decode size in place, alongside [`Self::resize`], once the
    /// display's scale factor is known.
    ///
    /// Every buffer already in the cache was decoded at the old size, so a
    /// genuine change drops them — free at the one call site, which runs before
    /// any view has fetched. Retunable rather than fixed at construction
    /// because the tier is built inside a view's `new`, where the scale factor
    /// isn't in hand.
    pub fn set_thumb_size(&self, thumb_size: u32) {
        if self.thumb_size.swap(thumb_size, Ordering::Relaxed) == thumb_size {
            return;
        }
        self.cache.lock().clear();
    }

    /// Current LRU capacity (max resident thumbnails). A `prewarm` caller
    /// walking a display-ordered path list can `.take(capacity())` while
    /// building it so it never allocates a path Vec longer than the cache
    /// can ever hold — `prewarm` itself caps decode work at this same
    /// number, so anything past it would only evict the earlier (more
    /// visible) covers.
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Cached lookup; decode + insert on miss. Returns the empty default
    /// `Image` for cache misses that fail to decode (and remembers that
    /// failure so we don't retry on every refilter).
    ///
    /// Hits promote the entry to most-recently-used. Inserts may evict
    /// the least-recently-used entry once the cache is full.
    ///
    /// Safe to call from any thread, but the returned `Image` is not
    /// `Send` — use it on the same thread.
    pub fn get_or_load(&self, path: &Path) -> Image {
        // Fast path: hit promotes LRU position. `LruCache::get` requires
        // &mut self, hence the Mutex.
        if let Some(maybe_buf) = self.cache.lock().get(path) {
            return buf_to_image(maybe_buf);
        }
        // Miss: decode without holding the lock so other threads can
        // continue reading the cache during the (slow) decode step.
        let buf = decode_thumb_buffer(path, self.thumb_size.load(Ordering::Relaxed));
        let img = buf_to_image(&buf);
        self.cache.lock().put(path.to_path_buf(), buf);
        img
    }

    /// Convenience wrapper around [`Self::get_or_load`] for the very common
    /// "row holds an `Option<String>` artwork path" shape — returns the
    /// default empty [`Image`] for `None` or `Some("")`. Centralises the
    /// `as_deref().filter(!is_empty).map(load).unwrap_or_default()` chain
    /// that previously appeared in three near-identical row-builders.
    pub fn get_or_load_opt(&self, path: Option<&str>) -> Image {
        match path.filter(|p| !p.is_empty()) {
            Some(p) => self.get_or_load(Path::new(p)),
            None => Image::default(),
        }
    }

    /// Cache-only lookup: returns the already-decoded [`Image`] for `path`,
    /// or [`Image::default()`] on miss — **never** decodes synchronously.
    ///
    /// For a surface that mounts rows *before* its tier is warm, which in
    /// this app is the queue sheet alone: its rows have to land in the model
    /// before `on_open_changed` returns so the slide-up has text on frame
    /// one, and a per-row [`Self::get_or_load_opt`] on that frame would
    /// block the UI thread on a screenful of decodes — freezing the very
    /// animation the synchronous build exists to feed. It serves the
    /// placeholder instead, and the sheet swaps to
    /// [`Self::get_or_load_opt`] once its off-thread [`Self::prewarm`] has
    /// landed. Everywhere else prewarms and awaits before setting rows, so
    /// the decoding lookup is already a cache hit and this isn't needed.
    pub fn get_cached_opt(&self, path: Option<&str>) -> Image {
        let Some(p) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        self.cache.lock().get(Path::new(p)).map_or_else(Image::default, buf_to_image)
    }

    /// Same caching contract as [`Self::get_or_load`] but returns the raw
    /// [`SharedPixelBuffer<Rgb8Pixel>`] backing the cache entry instead of
    /// wrapping it in a [`slint::Image`]. Material You seeds its palette from
    /// the already-decoded thumbnail this way, rather than opening and
    /// decoding the full-resolution artwork a second time.
    ///
    /// Returns `None` if a cached prior decode failed for this path; the
    /// failure is remembered so callers don't retry the same broken file.
    pub fn get_or_load_rgb8(&self, path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        if let Some(maybe_buf) = self.cache.lock().get(path) {
            return maybe_buf.clone();
        }
        let buf = decode_thumb_buffer(path, self.thumb_size.load(Ordering::Relaxed));
        let returned = buf.clone();
        self.cache.lock().put(path.to_path_buf(), buf);
        returned
    }

    /// Decode every uncached path in `paths` in parallel and populate the
    /// cache. Intended to run inside `tokio::task::spawn_blocking` so the
    /// CPU-bound decode work doesn't tie up the async runtime's worker
    /// pool. Already-cached entries are skipped — calling this twice is
    /// safe and roughly free on the second call.
    ///
    /// Uses `LruCache::contains` (non-promoting) for the pre-check so
    /// prewarm doesn't reorder LRU positions of paths that are already
    /// hot from earlier views.
    ///
    /// Input duplicates are deduped here rather than at call sites — a
    /// caller passing one path per *track* (e.g. a queue of many tracks
    /// sharing a few album covers) must not trigger one decode per
    /// duplicate.
    ///
    /// Work is capped at the LRU capacity: decoding more unique paths
    /// than the cache can hold would evict the earliest entries with the
    /// latest — pure wasted decode CPU, and on display-ordered input it
    /// would evict exactly the covers about to paint. Callers should pass
    /// paths in display order so the kept prefix is the visible one.
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
        // Hoist the size out of `self` so the Rayon closure captures a
        // plain `u32` (`Copy`/`Send`) rather than borrowing `self`.
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
        // Run the parallel decode on the bounded `DECODE_POOL` so the burst
        // of full-res `DynamicImage`s can't spike RSS across every core;
        // fall back to the global pool if the dedicated one failed to build.
        let decoded: Vec<(PathBuf, CachedBuf)> = match decode_pool() {
            Some(pool) => pool.install(decode_all),
            None => decode_all(),
        };
        let mut cache = self.cache.lock();
        for (p, buf) in decoded {
            // `put` is insert-or-update; since we filtered against
            // `contains` earlier this is effectively insert-only, but a
            // racing `get_or_load` between the filter and lock reacquire
            // could have inserted the same key — `put` handles that
            // safely (and counts as an LRU touch).
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
    // Held for the decode only — see `LARGE_DECODE_GATE`. A header the probe
    // can't read leaves the decode ungated, which is the same position every
    // decode was in before the gate existed.
    let _oversized = source_pixels(path)
        .is_some_and(|pixels| pixels > LARGE_SOURCE_PIXELS)
        .then(|| LARGE_DECODE_GATE.lock());

    let dyn_img = decode_capped(path, MAX_SOURCE_DIM).ok()?;

    // `thumbnail_exact` uses an integer-only fast algorithm and outputs
    // exactly `thumb_size × thumb_size`. Album art is overwhelmingly
    // square, so the lack of aspect preservation is moot; non-square
    // covers get a tiny aspect distortion which `image-fit: cover` on the
    // Slint side would have re-cropped anyway. Roughly 10× faster than
    // `resize_to_fill` with a high-quality filter, and the difference is
    // imperceptible.
    let thumb = dyn_img.thumbnail_exact(thumb_size, thumb_size).to_rgb8();
    let (w, h) = thumb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(thumb.as_raw());
    Some(buf)
}

#[cfg(test)]
#[path = "tests/cover_thumbs_tests.rs"]
mod tests;
