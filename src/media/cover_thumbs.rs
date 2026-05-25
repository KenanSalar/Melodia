//! Decoded cover thumbnails for the Tracks view.
//!
//! `slint::Image::load_from_path` decodes the source eagerly and never
//! caches by path — every call re-reads the file and produces a fresh
//! full-resolution RGBA bitmap. Album art is commonly 1000 × 1000 (~4 MB
//! decoded); naively assigning a fresh `Image` per row pushed RSS past
//! 500 MB on a real library. This module fixes that with four layers:
//!
//! 1. **Dedup by path.** Many tracks share the same album cover, so the
//!    cache returns a cheap clone of the same `SharedPixelBuffer` instead
//!    of re-decoding the file once per track.
//! 2. **Thumbnail downscale.** Each unique cover is decoded once and
//!    immediately reduced to `THUMB_SIZE × THUMB_SIZE`; the downscaled
//!    pixels are what we hand to Slint. For typical 1000×1000 sources this
//!    cuts memory by ~190× per cover (1000×1000×4 → 72×72×3).
//! 3. **RGB8, not RGBA8.** Album art is overwhelmingly JPEG/PNG-without-
//!    alpha. Decoding to RGB8 saves 25% per cover at no quality cost; the
//!    Skia backend converts to RGBA8 once on GPU upload, so the conversion
//!    is paid per-cover (bounded), not per-draw.
//! 4. **LRU eviction.** The cache is capped at `CACHE_CAP` entries; on
//!    overflow the least-recently-used cover is evicted. Without this, a
//!    catalogue with thousands of unique covers grows the cache forever.
//! 5. **Defensive decoder limits.** `image::Limits` caps the maximum
//!    source resolution we'll attempt, so a pathological 32 K × 32 K
//!    file in a tag can't allocate gigabytes.
//!
//! Pair this with [`CoverThumbs::prewarm`] from the call site to decode
//! unique paths in parallel via Rayon (inside a `spawn_blocking` task)
//! before the model is built — tracks then appear quickly because the
//! per-row lookup is a hashmap hit.
//!
//! ## Sizing rationale
//!
//! The thumbnail side length is **per-instance** (`thumb_size`), not a
//! global constant — different views display artwork at wildly different
//! sizes, and a one-size cache either pixelates the big tiles or wastes
//! memory on the small ones. Two tiers exist today:
//!
//!   * **Row tier** ([`CoverThumbs::new`] / [`Default`], [`THUMB_SIZE`] =
//!     72 px) — the 36 px track-row tile and the `clamp(bar-w * 0.07,
//!     36px, 46px)` now-playing-bar tile. 72 px is 2× `HiDPI` of the
//!     largest, which Skia's mipmaps render crisply at any DPR; smaller
//!     would soften the bar tile, larger would waste memory across the
//!     `CACHE_CAP`-entry cache.
//!   * **Album tiers** ([`CoverThumbs::with_config`]) — the Albums grid
//!     cards (448 px, flex-filled well past 280 px on wide panels) and the
//!     Album Detail header tile (384 px). `src/ui/albums.rs` owns a
//!     dedicated instance per tier; mixing those large buffers into the
//!     row-tier LRU would pollute it, so each gets its own (small-capped)
//!     cache — same separation rationale as
//!     [`crate::ui::now_playing_artwork::NowPlayingArtwork`].
//!
//! ## Why we cache buffers, not `Image`
//!
//! `slint::Image` wraps an internal `VRc` that contains `*mut ()`, so the
//! type is intentionally **not** `Send` or `Sync`. We can't put it in a
//! cross-thread cache, and Rayon's parallel pipeline can't produce it.
//! `slint::SharedPixelBuffer<Rgb8Pixel>` *is* `Send + Sync` and is
//! ref-counted, so:
//!   * the cache stores buffers (one decoded RGB payload per unique
//!     cover, shared across all tracks that reference it),
//!   * decoding happens in parallel on background threads,
//!   * `Image::from_rgb8(buf.clone())` runs on the UI thread when
//!     building rows; the clone is a refcount bump, not a copy.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lru::LruCache;
use parking_lot::Mutex;
use rayon::prelude::*;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

/// Default ("row tier") square thumbnail size, in pixels. 72 px = 2× the
/// 36 px row tile and 1.56× the 46 px now-playing bar tile, which Skia's
/// mipmaps render crisply on `HiDPI`. Each cached thumbnail occupies
/// 72 × 72 × 3 ≈ 15.5 KB. Larger tiers ([`CoverThumbs::with_config`]) pass
/// their own size — see the module docs' "Sizing rationale".
const THUMB_SIZE: u32 = 72;

/// Hard cap on accepted source resolution. Real album art is well under
/// this; the limit just prevents a malformed file with a forged dimension
/// header from triggering an absurd allocation.
const MAX_SOURCE_DIM: u32 = 8192;

/// Maximum number of entries kept in the cache — counts unique album
/// *covers* (one entry per artwork file), not tracks, so it tracks
/// library size, not queue / playback. At ~15.5 KB per entry (RGB8 /
/// 72 px), 512 entries ≈ 8 MB peak. Older entries are evicted LRU-style
/// on overflow; a missed cover just re-decodes inline on scroll-back (a
/// few ms for one 72 px thumbnail — no flash, no glitch). Most libraries
/// have a few hundred unique covers, well under the cap; only a very
/// large library (~6 000+ tracks) reaches eviction at all, and it
/// degrades gracefully there. The previous 1 500-entry cap budgeted
/// ~23 MB of headroom realistic libraries never used.
const CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(512) {
    Some(n) => n,
    None => panic!("CACHE_CAP > 0"),
};

/// `None` represents a previously-attempted decode that failed — we cache
/// failures too so refilters don't keep re-hitting the same broken file.
type CachedBuf = Option<SharedPixelBuffer<Rgb8Pixel>>;

/// Dedicated, bounded Rayon pool for [`CoverThumbs::prewarm`]'s parallel
/// decode. Each decode briefly holds a full-resolution `DynamicImage`
/// before downscaling (a 3000×3000 cover ≈ 27 MiB); fanning that across the
/// global pool — `num_cpus` wide — can spike RSS by hundreds of MiB during
/// a prewarm burst. A small dedicated pool caps how many full-res decodes
/// coexist *and* isolates the burst from the library scanner's use of the
/// global pool. `None` if the pool fails to build — `prewarm` then falls
/// back to the global pool.
static DECODE_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// The bounded decode pool, lazily built. Sized to half the logical cores,
/// clamped to 2–4 — the knob trading prewarm throughput against the
/// transient decode peak.
fn decode_pool() -> Option<&'static rayon::ThreadPool> {
    DECODE_POOL
        .get_or_init(|| {
            let threads = std::thread::available_parallelism()
                .map(|p| (p.get() / 2).clamp(2, 4))
                .unwrap_or(2);
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
    /// Side length every cover in this cache is downscaled to. Fixed at
    /// construction — a single instance is one size tier (see module docs).
    thumb_size: u32,
}

impl Default for CoverThumbs {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(CACHE_CAP)),
            thumb_size: THUMB_SIZE,
        }
    }
}

impl CoverThumbs {
    /// Row-tier cache: 72 px thumbnails, `CACHE_CAP` entries. Used by the
    /// Tracks / Browse views and the now-playing bar (shared instance) and
    /// by the queue sheet (its own private instance, released on close).
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache tier with a caller-chosen thumbnail size and LRU capacity.
    /// Used for views that display artwork far larger than the 72 px row
    /// tile (e.g. the Albums grid / detail at 448 / 384 px) — see the module
    /// docs' "Sizing rationale" for why each size gets its own instance.
    /// The capacity can later be retuned with [`Self::resize`].
    pub fn with_config(thumb_size: u32, cache_cap: NonZeroUsize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(cache_cap)),
            thumb_size,
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
        let buf = decode_thumb_buffer(path, self.thumb_size);
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
    /// Used by the queue sheet's first-frame "skeleton" render: rows must
    /// land in the model before the open callback returns so the slide-up
    /// animation has something to display, but blocking the UI thread on
    /// cover decode would freeze the animation itself. After this
    /// synchronous skeleton paint, the caller kicks off
    /// [`Self::prewarm`] off-thread and re-runs the row build with
    /// [`Self::get_or_load_opt`] once the cache is hot — covers fade in
    /// without ever blocking a frame.
    pub fn get_cached_opt(&self, path: Option<&str>) -> Image {
        let Some(p) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        self.cache
            .lock()
            .get(Path::new(p))
            .map_or_else(Image::default, buf_to_image)
    }

    /// Same caching contract as [`Self::get_or_load`] but returns the raw
    /// [`SharedPixelBuffer<Rgb8Pixel>`] backing the cache entry instead of
    /// wrapping it in a [`slint::Image`]. Used by Material You to consume
    /// the already-decoded 72×72 thumbnail buffer when generating a
    /// dynamic palette seed — avoids opening + decoding the artwork file
    /// a second time, which on large embedded covers (1500–3000 px) was
    /// the source of a ~30 MiB residual heap delta on the current track.
    ///
    /// Returns `None` if a cached prior decode failed for this path; the
    /// failure is remembered so callers don't retry the same broken file.
    pub fn get_or_load_rgb8(&self, path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        if let Some(maybe_buf) = self.cache.lock().get(path) {
            return maybe_buf.clone();
        }
        let buf = decode_thumb_buffer(path, self.thumb_size);
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
    pub fn prewarm(&self, paths: &[PathBuf]) {
        let missing: Vec<PathBuf> = {
            let cache = self.cache.lock();
            paths
                .iter()
                .filter(|p| !cache.contains(*p))
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        // Hoist the size out of `self` so the Rayon closure captures a
        // plain `u32` (`Copy`/`Send`) rather than borrowing `self`.
        let thumb_size = self.thumb_size;
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
    buf.as_ref()
        .map(|b| Image::from_rgb8(b.clone()))
        .unwrap_or_default()
}

fn decode_thumb_buffer(path: &Path, thumb_size: u32) -> CachedBuf {
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);

    // `thumbnail_exact` uses an integer-only fast algorithm and outputs
    // exactly `thumb_size × thumb_size`. Album art is overwhelmingly
    // square, so the lack of aspect preservation is moot; non-square
    // covers get a tiny aspect distortion which `image-fit: cover` on the
    // Slint side would have re-cropped anyway. Roughly 10× faster than
    // `resize_to_fill` with a high-quality filter, and the difference is
    // imperceptible.
    let dyn_img = reader.decode().ok()?;
    let thumb = dyn_img.thumbnail_exact(thumb_size, thumb_size).to_rgb8();
    let (w, h) = thumb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(thumb.as_raw());
    Some(buf)
}

#[cfg(test)]
#[path = "tests/cover_thumbs_tests.rs"]
mod tests;
