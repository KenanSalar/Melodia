//! The shared path-keyed thumbnail cache every track row draws through.
//!
//! `slint::Image::load_from_path` decodes eagerly and never caches by path, so an `Image` per row
//! is one full-resolution decode per *track* rather than per cover. This puts a bounded cache in
//! front of it: deduped by path, downscaled to `thumb_size` square, kept as **RGB8** (album art is
//! overwhelmingly alpha-free, and `FemtoVG` converts on upload rather than per draw), evicted LRU,
//! and decoded under `image::Limits` so a forged dimension header can't allocate gigabytes.
//!
//! **Downscaled only.** A source already smaller than the tier keeps its own size: it is drawn at
//! the tier's size either way, so padding the buffer out to it spends memory on pixels carrying no
//! information — and hands the GPU a box-filtered upscale where it would otherwise magnify
//! bilinearly at draw time.
//!
//! **`thumb_size` is per-instance.** Views draw artwork at wildly different sizes and `FemtoVG`
//! minifies with plain bilinear and no mipmaps, so one size either softens the big tiles or wastes
//! memory on the small ones; mixing grid-sized buffers into the row tier's LRU would also evict
//! row thumbnails wholesale.
//!
//! Buffers are cached rather than `Image` because `slint::Image` is deliberately neither `Send`
//! nor `Sync`, so it can neither live in a cross-thread cache nor come out of a Rayon pipeline.
//! `SharedPixelBuffer<Rgb8Pixel>` is both, and refcounted.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use image::RgbImage;
use lru::LruCache;
use parking_lot::Mutex;
use rayon::prelude::*;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

use super::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped_to, large_decode_guard, resize_rgb8,
    resize_rgb8_image, source_pixels,
};

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
    if scale > 1.25 { ROW_THUMB_SIZE_HIDPI } else { ROW_THUMB_SIZE }
}

/// Row-tier LRU capacity, counting unique *covers* rather than tracks, so it scales with library
/// size. Past the cap, eviction means a scroll-back re-decodes one thumbnail inline.
const CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(512) {
    Some(n) => n,
    None => panic!("CACHE_CAP > 0"),
};

/// `None` is a decode that failed — cached too, so refilters don't keep re-hitting the same broken
/// file.
type CachedBuf = Option<SharedPixelBuffer<Rgb8Pixel>>;

/// A cached thumbnail and the tier size it was decoded for.
///
/// The size rides with the value because the key is the path alone: one cover is one entry
/// whatever [`CoverThumbs::thumb_size`] is set to, so without it a retune has no way to ask
/// whether what the tier holds is still the size being drawn, and no option but to drop the lot.
struct Cached {
    buf: CachedBuf,
    /// The size asked for at the decode, not the buffer's own extent. The tier is downscale-only,
    /// so a cover smaller than it keeps its own size and the two differ routinely.
    size: u32,
}

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

/// Misses [`CoverThumbs::get_or_schedule_opt`] has handed to the decode pool.
///
/// `draining` is what coalesces a screenful of them into one batch: the first miss spawns the
/// drain and every miss behind it only joins the queue. It is cleared under the same lock that
/// finds the queue empty, so a miss arriving as a drain finishes cannot be left with nothing
/// scheduled to pick it up.
#[derive(Default)]
struct Pending {
    /// Waiting misses, oldest first, capped at the tier's capacity — queueing more than the
    /// cache can hold means the tail of the batch evicts the head of it. Newest wins, that
    /// being the one a card is currently asking for; `queued` is the membership half.
    queue: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    /// What this burst has already handed to the pool.
    ///
    /// A path coming back is one the tier decoded and then dropped to fit the rest of the
    /// batch, so decoding it again would evict whatever took its place and the notifier would
    /// ask a third time — a grid mounting more cards than its tier holds would never settle.
    /// Cleared by the first miss the burst hasn't seen, which is what distinguishes a moved
    /// visible set from a cache thrashing under a fixed one.
    settled: HashSet<PathBuf>,
    draining: bool,
}

impl Pending {
    /// Forget everything waiting and everything the burst learned, for a tier being released. A
    /// drain already on the pool is left to notice on its own.
    fn reset(&mut self) {
        self.queue.clear();
        self.queued.clear();
        self.settled.clear();
    }
}

pub struct CoverThumbs {
    cache: Mutex<LruCache<PathBuf, Cached>>,
    /// Side length every cover in this cache is downscaled to. Atomic because the tiers are held
    /// behind `Arc` and [`Self::set_thumb_size`] retunes them once the scale factor is known.
    thumb_size: AtomicU32,
    /// Bumped by [`Self::clear`]. A batch reads it before decoding and again before inserting, so
    /// buffers whose tier was released mid-flight are dropped rather than landing in the memory a
    /// section leave has already handed back. A retune is deliberately not one of these: it moves
    /// what a lookup asks for, not whether there is still a tier to ask.
    epoch: AtomicU64,
    pending: Mutex<Pending>,
    /// Fired once per landed batch, for the UI to invalidate the bindings that missed. Set at
    /// wire time and never replaced, a tier having exactly one surface to tell.
    on_decoded: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl Default for CoverThumbs {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(CACHE_CAP)),
            thumb_size: AtomicU32::new(ROW_THUMB_SIZE),
            epoch: AtomicU64::new(0),
            pending: Mutex::new(Pending::default()),
            on_decoded: OnceLock::new(),
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
            ..Self::default()
        }
    }

    /// Drop every cached buffer, every queued miss and whatever is mid-decode, for a per-view tier
    /// released on section leave. Callers pair this with `allocator::trim()` so glibc hands the
    /// freed pages back to the OS.
    ///
    /// **Both locks, cache first**, which is the order every other path here takes them in — and
    /// this is the only one holding both, so there is no second order to invert against. The
    /// epoch bump belongs under the *cache* lock rather than the queue's, that being the lock a
    /// finished batch takes to insert: emptying the cache and invalidating the batch have to be
    /// one step, or a batch reading the epoch between them lands in the tier this just emptied.
    pub fn clear(&self) {
        let mut cache = self.cache.lock();
        let mut pending = self.pending.lock();
        self.epoch.fetch_add(1, Ordering::Relaxed);
        pending.reset();
        cache.clear();
    }

    /// Downscale everything held to a `proxy_dim` square, in place, for a grid whose section is
    /// being left.
    ///
    /// **The alternative to [`Self::clear`], and the difference is what coming back costs.** A
    /// released tier re-reads and re-decodes every visible cover before a card can paint, so the
    /// re-entry spends that whole round trip on the fallback glyph. A proxy is the same picture
    /// for a fraction of the bytes, and the tier already knows what to do with one:
    /// [`Self::get_or_schedule_opt`] serves an entry whose recorded size isn't the one being drawn
    /// *and* re-queues it, so frame one paints the cover and the full-size replacement lands
    /// behind it.
    ///
    /// Nothing is invalidated, so **no epoch bump** — there is no released memory for an in-flight
    /// batch to land in. A batch landing between the snapshot and the write-back below loses its
    /// decode to a proxy of the same cover and nothing else, which is not worth an identity check
    /// to avoid.
    ///
    /// **The wider race is the same trade and has the same answer.** Where one tier serves several
    /// surfaces, the leave this runs on and the *arriving* surface's prewarm reach the pool in no
    /// fixed order, so this can shrink a screenful the destination decoded a moment ago. It costs
    /// that screenful twice and nothing else: the recorded size makes every one of them stale, so
    /// the cards paint the proxy and re-queue. Narrowing it means either a timestamp per entry or
    /// a caller-supplied path set, and the second is the per-view ownership sharing a tier retired.
    ///
    /// An entry already at or under `proxy_dim` is left alone: it costs nothing and it is still
    /// the right answer at the live size. So is a cached failure, which has no buffer and must
    /// stay remembered.
    pub fn shrink_to_proxy(&self, proxy_dim: u32) {
        // Resampled off the lock, as every other decode here is: the section leave this runs on
        // shares the blocking pool with whatever the next section is already fetching.
        let full: Vec<(PathBuf, SharedPixelBuffer<Rgb8Pixel>)> = {
            let cache = self.cache.lock();
            cache
                .iter()
                .filter_map(|(path, held)| {
                    let buf = held.buf.as_ref()?;
                    (buf.width().max(buf.height()) > proxy_dim).then(|| (path.clone(), buf.clone()))
                })
                .collect()
        };

        let shrunk: Vec<(PathBuf, SharedPixelBuffer<Rgb8Pixel>)> = full
            .into_iter()
            .filter_map(|(path, buf)| {
                let source =
                    RgbImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())?;
                let proxy =
                    resize_rgb8_image(Cow::Owned(source), proxy_dim, proxy_dim, FilterType::Box)?;
                Some((path, buffer_from_rgb(&proxy)))
            })
            .collect();

        {
            let mut cache = self.cache.lock();
            for (path, buf) in shrunk {
                // `peek_mut`, never `put`: promoting would reorder the tier into this pass's
                // iteration order and hand the next eviction the wrong answer about what was
                // seen last.
                if let Some(held) = cache.peek_mut(&path) {
                    held.buf = Some(buf);
                    held.size = proxy_dim;
                }
            }
        }
        // What [`Self::set_thumb_size`] clears it for, and for the same reason: `settled` records
        // which paths this burst already handed to the pool, and every one of them is now worth
        // handing over again. Left standing, a card scrolled to past the next prewarm would have
        // its re-decode refused and keep the proxy.
        self.pending.lock().settled.clear();
    }

    /// Retune the LRU capacity in place, once the real display size is known. Shrinking evicts
    /// down to the new cap.
    pub fn resize(&self, cache_cap: NonZeroUsize) {
        self.cache.lock().resize(cache_cap);
    }

    /// Retune the decode size in place, alongside [`Self::resize`]. Retunable rather than fixed at
    /// construction because a tier is built inside a view's `new`, where the scale factor isn't in
    /// hand.
    ///
    /// **Nothing cached is dropped.** `WindowChrome.display-changed` calls this on every winit
    /// `Resized`, long after the views have fetched, and a tier emptied under mounted cards leaves
    /// every one of them on its placeholder until something re-runs its binding — which on a grid
    /// is the next column change, i.e. the user resizing again. A lookup gets the buffer it
    /// already had instead, at the old size, while the replacement decodes behind it.
    ///
    /// What does go is `settled`: it records which paths this burst has already handed to the
    /// pool, and at the new size every one of them is worth handing over again.
    pub fn set_thumb_size(&self, thumb_size: u32) {
        if self.thumb_size.swap(thumb_size, Ordering::Relaxed) == thumb_size {
            return;
        }
        self.pending.lock().settled.clear();
    }

    /// Current LRU capacity. A `prewarm` caller building a display-ordered path list can
    /// `.take(capacity())` so it never allocates a `Vec` longer than the cache can hold —
    /// `prewarm` caps its own decode work at the same number.
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Cached lookup, decoding and inserting on a miss or on an entry a retune has left at the
    /// wrong size. A decode that fails is remembered, so a refilter doesn't retry it. Safe from
    /// any thread, but the returned `Image` is not `Send`.
    pub fn get_or_load(&self, path: &Path) -> Image {
        let thumb_size = self.thumb_size.load(Ordering::Relaxed);
        // `LruCache::get` takes `&mut self` (a hit promotes), hence the mutex.
        if let Some(held) = self.cache.lock().get(path).filter(|held| held.size == thumb_size) {
            return buf_to_image(&held.buf);
        }
        // Decode off the lock so other threads can keep reading the cache.
        let entry = decode_thumb(path, thumb_size);
        let img = buf_to_image(&entry.buf);
        self.cache.lock().put(path.to_path_buf(), entry);
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
    /// The one lookup that takes whatever size the tier holds, having neither of the two ways to
    /// improve on it: it may not decode and it may not schedule.
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
        self.cache
            .lock()
            .get(Path::new(p))
            .map_or_else(Image::default, |held| buf_to_image(&held.buf))
    }

    /// Register what to call when a scheduled batch lands. First caller wins.
    ///
    /// Takes a plain closure rather than anything UI-shaped so this module stays unaware of Slint
    /// globals; the caller is what knows which binding to invalidate.
    pub fn set_decoded_notifier(&self, notify: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_decoded.set(Box::new(notify));
    }

    /// [`Self::get_or_load_opt`] with the miss moved off the calling thread.
    ///
    /// **For the Slint model getters, which run on the UI thread.** `get_or_load_opt` decodes
    /// inline, which is bounded and invisible while `prewarm` has covered the surface — and
    /// stops being either once a library holds more unique covers than the tier's cap, where
    /// scrolling past the prewarmed prefix turns every row into a decode on the event loop.
    /// This serves the placeholder instead, hands the path to the decode pool, and leaves the
    /// notifier to bring the row back.
    ///
    /// An entry a retune left at the wrong size is served *and* re-queued: it is the right picture
    /// at a stale resolution, so painting it beats a placeholder for however long the replacement
    /// takes, and a resize never blanks a card whose cover the tier already holds.
    pub fn get_or_schedule_opt(self: &Arc<Self>, path: Option<&str>) -> Image {
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        let path = Path::new(path);
        let thumb_size = self.thumb_size.load(Ordering::Relaxed);
        // Scoped so the lock is gone before `schedule`, which takes it again through `capacity`.
        let held = {
            let mut cache = self.cache.lock();
            cache.get(path).map(|held| (buf_to_image(&held.buf), held.size))
        };
        match held {
            Some((img, size)) if size == thumb_size => img,
            Some((img, _)) => {
                self.schedule(path.to_path_buf());
                img
            }
            None => {
                self.schedule(path.to_path_buf());
                Image::default()
            }
        }
    }

    /// Queue one miss, spawning the drain if nothing is already draining.
    fn schedule(self: &Arc<Self>, path: PathBuf) {
        // Read before the queue lock — `capacity` takes the cache's, and nothing else here
        // holds both.
        let capacity = self.capacity();
        {
            let mut pending = self.pending.lock();
            if pending.queued.contains(&path) || pending.settled.contains(&path) {
                return;
            }
            // A miss this burst hasn't seen means the visible set moved, so what it learned
            // about the old one no longer stands.
            pending.settled.clear();
            pending.queued.insert(path.clone());
            pending.queue.push_back(path);
            while pending.queue.len() > capacity {
                if let Some(dropped) = pending.queue.pop_front() {
                    pending.queued.remove(&dropped);
                }
            }
            if pending.draining {
                return;
            }
            pending.draining = true;
        }
        let this = Arc::clone(self);
        let drain = move || this.drain_pending();
        match decode_pool() {
            Some(pool) => pool.spawn(drain),
            None => rayon::spawn(drain),
        }
    }

    /// Decode everything queued, then everything queued while that ran, until the queue is empty.
    ///
    /// Looping rather than one batch per spawn because the misses arrive as fast as rows mount:
    /// a scroll would otherwise spawn a drain per frame, each contending for the same pool.
    fn drain_pending(&self) {
        loop {
            let (epoch, batch) = {
                let mut pending = self.pending.lock();
                if pending.queue.is_empty() {
                    pending.draining = false;
                    return;
                }
                pending.queued.clear();
                let batch: Vec<PathBuf> = pending.queue.drain(..).collect();
                // Doubles as the in-flight guard `queued` was: a binding re-running mid-decode
                // must not queue what this batch is already carrying.
                pending.settled.extend(batch.iter().cloned());
                (self.epoch.load(Ordering::Relaxed), batch)
            };

            let thumb_size = self.thumb_size.load(Ordering::Relaxed);
            // Non-promoting, so this can't reorder a prefix `prewarm` warmed. A tab pick mounts
            // rows *before* its prewarm runs, so the two routinely ask for the same covers and
            // without this every one of them is decoded twice.
            let batch: Vec<PathBuf> = {
                let cache = self.cache.lock();
                batch
                    .into_iter()
                    .filter(|path| !holds(&cache, path.as_path(), thumb_size))
                    .collect()
            };

            let decoded: Vec<(PathBuf, Cached)> = batch
                .into_par_iter()
                .map(|path| {
                    let entry = decode_thumb(&path, thumb_size);
                    (path, entry)
                })
                .collect();

            {
                let mut cache = self.cache.lock();
                // Read under the cache lock, which [`Self::clear`] takes to bump it: a release is
                // either complete and visible here, or waiting behind this insert and about to
                // discard it. Outside the lock the two interleave and the batch lands in the
                // memory a section leave has already handed back.
                if self.epoch.load(Ordering::Relaxed) == epoch {
                    // A `prewarm` or a `get_or_load` could have inserted the same key while this
                    // batch was decoding. Measured against the tier's *live* size rather than the
                    // one this batch read: a retune since then makes what we carry the staler of
                    // the two, and an entry already at the size being drawn is not ours to replace.
                    let current = self.thumb_size.load(Ordering::Relaxed);
                    for (path, entry) in decoded {
                        if !holds(&cache, path.as_path(), current) {
                            cache.put(path, entry);
                        }
                    }
                }
            }

            // Every iteration owes this, not only the ones that landed. A batch someone else had
            // already cached and a batch a release superseded each leave bindings sitting on the
            // placeholder they took when they queued, and this is the only thing that re-runs
            // them; a miss the queue dropped to stay inside the tier comes back the same way.
            if let Some(notify) = self.on_decoded.get() {
                notify();
            }
        }
    }

    /// [`Self::get_or_load`]'s contract over the raw buffer rather than a [`slint::Image`].
    /// Material You seeds its palette from the already-decoded thumbnail this way, rather than
    /// decoding the full-resolution artwork a second time.
    pub fn get_or_load_rgb8(&self, path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        let thumb_size = self.thumb_size.load(Ordering::Relaxed);
        if let Some(held) = self.cache.lock().get(path).filter(|held| held.size == thumb_size) {
            return held.buf.clone();
        }
        let entry = decode_thumb(path, thumb_size);
        let returned = entry.buf.clone();
        self.cache.lock().put(path.to_path_buf(), entry);
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
        // Hoisted so the Rayon closure captures a plain `u32` rather than `self`.
        let thumb_size = self.thumb_size.load(Ordering::Relaxed);
        let missing: Vec<PathBuf> = {
            let cache = self.cache.lock();
            let cap = cache.cap().get();
            let mut seen = HashSet::with_capacity(paths.len().min(cap));
            paths
                .iter()
                .filter(|p| !holds(&cache, p.as_path(), thumb_size) && seen.insert(*p))
                .take(cap)
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let decode_all = move || {
            missing
                .into_par_iter()
                .map(|p| {
                    let entry = decode_thumb(&p, thumb_size);
                    (p, entry)
                })
                .collect::<Vec<(PathBuf, Cached)>>()
        };
        let decoded: Vec<(PathBuf, Cached)> = match decode_pool() {
            Some(pool) => pool.install(decode_all),
            None => decode_all(),
        };
        let mut cache = self.cache.lock();
        // Against the tier's live size for the reason the drain's insert gives: a retune landing
        // mid-decode makes this pass the staler of the two writers.
        let current = self.thumb_size.load(Ordering::Relaxed);
        for (p, entry) in decoded {
            // A `get_or_load` racing between the filter above and this reacquire could have
            // inserted the same key.
            if !holds(&cache, p.as_path(), current) {
                cache.put(p, entry);
            }
        }
    }
}

/// Whether the tier already holds `path` decoded for `thumb_size`.
///
/// **Non-promoting**, so asking cannot reorder the prefix a `prewarm` warmed in display order.
fn holds(cache: &LruCache<PathBuf, Cached>, path: &Path, thumb_size: u32) -> bool {
    cache.peek(path).is_some_and(|held| held.size == thumb_size)
}

fn buf_to_image(buf: &CachedBuf) -> Image {
    buf.as_ref().map(|b| Image::from_rgb8(b.clone())).unwrap_or_default()
}

fn decode_thumb(path: &Path, thumb_size: u32) -> Cached {
    Cached { buf: decode_thumb_buffer(path, thumb_size), size: thumb_size }
}

fn decode_thumb_buffer(path: &Path, thumb_size: u32) -> CachedBuf {
    // Held for the decode only. A header the probe can't read leaves the decode ungated, which is
    // no worse than having no gate.
    let _oversized = source_pixels(path).and_then(large_decode_guard);

    let dyn_img = decode_capped_to(path, MAX_SOURCE_DIM, thumb_size).ok()?;

    // Square and aspect-blind, which is moot for overwhelmingly square album art — and
    // `image-fit: cover` on the Slint side would have re-cropped a non-square one anyway.
    // Bounded by the source's own long edge: a cover smaller than the tier is drawn at the
    // tier's size either way, and enlarging it here only buys a bigger buffer.
    let side = thumb_size.min(dyn_img.width().max(dyn_img.height()));
    let thumb = resize_rgb8(&dyn_img, side, side, FilterType::Box)?;
    Some(buffer_from_rgb(&thumb))
}

/// An RGB8 image as the refcounted buffer the cache and `FemtoVG` both take.
fn buffer_from_rgb(img: &RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = img.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    buf
}

#[cfg(test)]
#[path = "tests/cover_thumbs_tests.rs"]
mod tests;
