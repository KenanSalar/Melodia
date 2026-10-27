//! The shared path-keyed thumbnail cache every track row draws through.
//!
//! `slint::Image::load_from_path` decodes eagerly and never caches, so an `Image` per row is a
//! full-resolution decode per *track*. This puts a bounded LRU in front: deduped by path, held as
//! RGB8 buffers (album art is almost always opaque, and `slint::Image` is neither `Send` nor
//! `Sync`), decoded under `image::Limits` and downscaled, never up, to a `thumb_size` square.
//! That size is per instance because `FemtoVG` minifies bilinear without mipmaps: one size would
//! either soften the big tiles or spend memory on the small ones.

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

use super::decoded::Decoded;
use super::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped_to, large_decode_guard, resize_rgb8,
    resize_rgb8_image, source_pixels,
};

/// The row tier at 1×: just over the now-playing bar's tile, the larger of its two consumers.
const ROW_THUMB_SIZE: u32 = 48;

const ROW_THUMB_SIZE_HIDPI: u32 = 72;

/// Split rather than fixed at the `HiDPI` size so the common 1× display doesn't pay for pixels it
/// never draws. Below 1.5 so a fractional scale rounds up, softness being the worse failure.
pub fn row_cover_size(scale: f64) -> u32 {
    if scale > 1.25 { ROW_THUMB_SIZE_HIDPI } else { ROW_THUMB_SIZE }
}

const CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(512) {
    Some(n) => n,
    None => panic!("CACHE_CAP > 0"),
};

type CachedBuf = Decoded<SharedPixelBuffer<Rgb8Pixel>>;

/// A cached thumbnail and the size it was decoded for. The key is the path alone, so without the
/// size a retune couldn't tell a stale entry from a current one.
struct Cached {
    buf: CachedBuf,
    /// The size asked for, not the buffer's extent: a cover smaller than the tier keeps its own.
    size: u32,
}

impl Cached {
    fn serves(&self, path: &Path, thumb_size: u32) -> bool {
        self.size == thumb_size && self.buf.is_current(path)
    }
}

/// Its own pool, keeping a prewarm burst off the global one the UI thread's inline decodes wait on,
/// and its width bounds how many full-resolution decodes coexist. `None` falls back to the global.
static DECODE_POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// Half the logical cores, clamped: prewarm throughput against the transient decode peak.
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

/// Misses handed to the decode pool. `draining` coalesces a screenful into one batch, and is
/// cleared under the lock that finds the queue empty, so a miss arriving as a drain finishes is
/// never left unscheduled.
#[derive(Default)]
struct Pending {
    /// Oldest first, capped at the tier's capacity with the newest winning: queueing more than the
    /// cache holds would evict the batch's head with its tail. `queued` is the membership half.
    queue: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    /// What this burst already handed to the pool. A path coming back was decoded and then evicted
    /// by the rest of its batch, so re-decoding it would keep a grid mounting more cards than the
    /// tier holds from ever settling. Cleared by the first unseen miss: the visible set moved.
    settled: HashSet<PathBuf>,
    draining: bool,
}

impl Pending {
    /// For a tier being released. A drain already on the pool notices through the epoch.
    fn reset(&mut self) {
        self.queue.clear();
        self.queued.clear();
        self.settled.clear();
    }
}

enum TierSize {
    /// Atomic so [`CoverThumbs::set_thumb_size`] can retune it behind the `Arc`.
    Own(AtomicU32),
    /// The backing's, read live so its retunes carry over. A miss peeks the backing without
    /// promoting, so no borrower decides what the backing's surfaces keep.
    Backed(Arc<CoverThumbs>),
}

pub struct CoverThumbs {
    cache: Mutex<LruCache<PathBuf, Cached>>,
    size: TierSize,
    /// Bumped by [`Self::clear`]. A batch or prewarm reads it before decoding and again before
    /// inserting, so what it decoded for a tier released mid-flight is dropped rather than
    /// refilling it. A retune isn't a release and doesn't bump it.
    epoch: AtomicU64,
    pending: Mutex<Pending>,
    /// Fired once per drained batch, for the UI to re-run the bindings that missed.
    on_decoded: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

impl Default for CoverThumbs {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(CACHE_CAP)),
            size: TierSize::Own(AtomicU32::new(ROW_THUMB_SIZE)),
            epoch: AtomicU64::new(0),
            pending: Mutex::new(Pending::default()),
            on_decoded: OnceLock::new(),
        }
    }
}

impl CoverThumbs {
    /// The shared row tier, behind every track table and the now-playing bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// A tier for views drawing artwork far larger than a row tile.
    pub fn with_config(thumb_size: u32, cache_cap: NonZeroUsize) -> Self {
        Self {
            cache: Mutex::new(LruCache::new(cache_cap)),
            size: TierSize::Own(AtomicU32::new(thumb_size)),
            ..Self::default()
        }
    }

    /// A tier answering a miss with `backing`'s buffer, by reference, before decoding one: for a
    /// surface that hands its covers back on close while mostly drawing what a shared tier holds.
    /// It follows the backing's size, so [`Self::set_thumb_size`] on it does nothing.
    pub fn backed_by(backing: Arc<CoverThumbs>) -> Self {
        Self { size: TierSize::Backed(backing), ..Self::default() }
    }

    /// Drops every buffer, queued miss and in-flight decode, for a tier released on section leave.
    /// Callers pair it with `allocator::trim()`.
    ///
    /// Both locks, cache first, the order every other path takes them in. The epoch is bumped under
    /// the *cache* lock, the one a finished batch inserts under: emptying and invalidating have to
    /// be one step, or a batch reading the epoch between them lands in the emptied tier.
    pub fn clear(&self) {
        let mut cache = self.cache.lock();
        let mut pending = self.pending.lock();
        self.epoch.fetch_add(1, Ordering::Relaxed);
        pending.reset();
        cache.clear();
    }

    /// Downscales everything held to a `proxy_dim` square in place, for a grid whose section is
    /// left. After a [`Self::clear`] a re-entry re-decodes before any card paints; a proxy is the
    /// same picture in a fraction of the bytes, and its stale recorded size makes
    /// [`Self::get_or_schedule_opt`] paint it and re-queue the full size.
    ///
    /// No epoch bump, nothing being released. A batch or another surface's prewarm landing just
    /// before the write-back can lose a screenful to proxies, which only re-queues it.
    pub fn shrink_to_proxy(&self, proxy_dim: u32) {
        // On the decode pool: the leave's blocking thread is shared with the next section's fetch.
        let full: Vec<(PathBuf, SharedPixelBuffer<Rgb8Pixel>)> = {
            let cache = self.cache.lock();
            cache
                .iter()
                .filter_map(|(path, held)| {
                    let buf = held.buf.ready()?;
                    (buf.width().max(buf.height()) > proxy_dim).then(|| (path.clone(), buf.clone()))
                })
                .collect()
        };

        let resample_all = move || {
            full.into_par_iter()
                .filter_map(|(path, buf)| {
                    let source =
                        RgbImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())?;
                    let proxy = resize_rgb8_image(
                        Cow::Owned(source),
                        proxy_dim,
                        proxy_dim,
                        FilterType::Box,
                    )?;
                    Some((path, buffer_from_rgb(&proxy)))
                })
                .collect::<Vec<(PathBuf, SharedPixelBuffer<Rgb8Pixel>)>>()
        };
        let shrunk: Vec<(PathBuf, SharedPixelBuffer<Rgb8Pixel>)> = match decode_pool() {
            Some(pool) => pool.install(resample_all),
            None => resample_all(),
        };

        {
            let mut cache = self.cache.lock();
            for (path, buf) in shrunk {
                // `peek_mut`, never `put`: promoting would reorder the LRU into this pass's order.
                if let Some(held) = cache.peek_mut(&path) {
                    held.buf = Decoded::Ready(buf);
                    held.size = proxy_dim;
                }
            }
        }
        // Every settled path is worth handing over again, or a card scrolled to past the next
        // prewarm keeps its proxy.
        self.pending.lock().settled.clear();
    }

    /// Retunes the capacity once the display is known. Clears `settled`, an evicted entry being a
    /// miss `schedule` would refuse as already handed over, leaving its card on the glyph. An
    /// unchanged cap is a no-op, `settled` included: this runs every frame of a panel ease.
    pub fn resize(&self, cache_cap: NonZeroUsize) {
        {
            let mut cache = self.cache.lock();
            if cache.cap() == cache_cap {
                return;
            }
            cache.resize(cache_cap);
        }
        self.pending.lock().settled.clear();
    }

    /// Retunes the decode size in place, a tier being built before the scale factor is known.
    ///
    /// **Nothing cached is dropped.** This runs on every winit `Resized`, and a tier emptied under
    /// mounted cards strands them on the placeholder until their binding re-runs, on a grid the
    /// next column change. Only `settled` goes, its paths all worth handing over at the new size.
    pub fn set_thumb_size(&self, thumb_size: u32) {
        let TierSize::Own(size) = &self.size else { return };
        if size.swap(thumb_size, Ordering::Relaxed) == thumb_size {
            return;
        }
        self.pending.lock().settled.clear();
    }

    pub fn thumb_size(&self) -> u32 {
        match &self.size {
            TierSize::Own(size) => size.load(Ordering::Relaxed),
            TierSize::Backed(backing) => backing.thumb_size(),
        }
    }

    fn load(&self, path: &Path, thumb_size: u32) -> Cached {
        self.borrow_from_backing(path, thumb_size).unwrap_or_else(|| decode_thumb(path, thumb_size))
    }

    /// Only at the size being drawn: a stale entry is the backing's to replace, not ours to copy.
    fn borrow_from_backing(&self, path: &Path, thumb_size: u32) -> Option<Cached> {
        let TierSize::Backed(backing) = &self.size else { return None };
        let cache = backing.cache.lock();
        let held = cache.peek(path).filter(|held| held.serves(path, thumb_size))?;
        Some(Cached { buf: held.buf.clone(), size: thumb_size })
    }

    /// For a caller publishing its decode size beside the tier: a retune only agrees with what is
    /// held when nothing is.
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    /// A caller building a prewarm list can `.take()` this many, `prewarm` capping at the same.
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Cached lookup, decoding inline on a miss or on an entry a retune left at the wrong size.
    /// Failures are cached too, so a refilter doesn't retry them.
    pub fn get_or_load(&self, path: &Path) -> Image {
        let thumb_size = self.thumb_size();
        if let Some(held) = self.cache.lock().get(path).filter(|held| held.serves(path, thumb_size))
        {
            return buf_to_image(&held.buf);
        }
        let entry = self.load(path, thumb_size);
        let img = buf_to_image(&entry.buf);
        self.cache.lock().put(path.to_path_buf(), entry);
        img
    }

    pub fn get_or_load_opt(&self, path: Option<&str>) -> Image {
        match path.filter(|p| !p.is_empty()) {
            Some(p) => self.get_or_load(Path::new(p)),
            None => Image::default(),
        }
    }

    /// Cache-only, at whatever size is held. For callers with no generation to come back on: the
    /// now-playing bridge, which a warm-up fills a moment later, and a released Radio logo tier,
    /// which paints monograms until its prewarm announces.
    pub fn get_cached_opt(&self, path: Option<&str>) -> Image {
        let Some(p) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        if let Some(held) = self.cache.lock().get(Path::new(p)) {
            return buf_to_image(&held.buf);
        }
        Image::default()
    }

    /// Registers what to call when a scheduled batch lands; first caller wins. A plain closure, so
    /// the caller decides which binding to invalidate.
    pub fn set_decoded_notifier(&self, notify: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_decoded.set(Box::new(notify));
    }

    /// [`Self::get_or_load_opt`] for the Slint model getters, with the miss off the UI thread: it
    /// serves the placeholder, schedules the decode and leaves the notifier to bring the row back.
    /// An entry at a stale size is served *and* re-queued, so a resize never blanks a card, and a
    /// backed tier borrows before scheduling, a refcount rather than a decode. A cached failure at
    /// the drawn size is a hit, so a broken cover can't spin against the notifier.
    pub fn get_or_schedule_opt(self: &Arc<Self>, path: Option<&str>) -> Image {
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return Image::default();
        };
        let path = Path::new(path);
        let thumb_size = self.thumb_size();
        // Scoped so the lock is gone before `schedule`, which takes it again through `capacity`.
        let held = {
            let mut cache = self.cache.lock();
            cache
                .get(path)
                .map(|held| (buf_to_image(&held.buf), held.size, held.buf.is_current(path)))
        };
        match held {
            Some((img, size, true)) if size == thumb_size => img,
            Some((img, _, current)) => {
                // A missing cover that is back may be among the paths this burst settled.
                if !current {
                    self.pending.lock().settled.remove(path);
                }
                self.schedule(path.to_path_buf());
                img
            }
            None => {
                if let Some(borrowed) = self.borrow_from_backing(path, thumb_size) {
                    let img = buf_to_image(&borrowed.buf);
                    self.cache.lock().put(path.to_path_buf(), borrowed);
                    return img;
                }
                self.schedule(path.to_path_buf());
                Image::default()
            }
        }
    }

    fn schedule(self: &Arc<Self>, path: PathBuf) {
        // Before the queue lock: `capacity` takes the cache's, and `clear` holds cache then queue.
        let capacity = self.capacity();
        {
            let mut pending = self.pending.lock();
            if pending.queued.contains(&path) || pending.settled.contains(&path) {
                return;
            }
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

    /// Decodes everything queued, then whatever queued meanwhile, until the queue is empty. A loop
    /// rather than a spawn per batch, which a scroll would make one per frame.
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
                // Also the in-flight guard against a binding re-running mid-decode.
                pending.settled.extend(batch.iter().cloned());
                (self.epoch.load(Ordering::Relaxed), batch)
            };

            let thumb_size = self.thumb_size();
            // A tab pick mounts rows before its prewarm runs, so the two routinely ask for the
            // same covers.
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
                    let entry = self.load(&path, thumb_size);
                    (path, entry)
                })
                .collect();

            {
                let mut cache = self.cache.lock();
                // Under the cache lock, the one `clear` bumps the epoch under.
                if self.epoch.load(Ordering::Relaxed) == epoch {
                    // Against the *live* size: after a retune this batch is the staler writer, and
                    // an entry another writer landed at the drawn size isn't ours to replace.
                    let current = self.thumb_size();
                    for (path, entry) in decoded {
                        if !holds(&cache, path.as_path(), current) {
                            cache.put(path, entry);
                        }
                    }
                }
            }

            // Every pass owes this, landed or not: the bindings that queued sit on the placeholder
            // with nothing else to re-run them, a miss the queue dropped included.
            if let Some(notify) = self.on_decoded.get() {
                notify();
            }
        }
    }

    /// [`Self::get_or_load`] over the raw buffer, so Material You seeds from the thumbnail rather
    /// than decoding the artwork a second time.
    pub fn get_or_load_rgb8(&self, path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        let thumb_size = self.thumb_size();
        if let Some(held) = self.cache.lock().get(path).filter(|held| held.serves(path, thumb_size))
        {
            return held.buf.ready().cloned();
        }
        let entry = self.load(path, thumb_size);
        let returned = entry.buf.ready().cloned();
        self.cache.lock().put(path.to_path_buf(), entry);
        returned
    }

    /// Decodes every uncached path in parallel and fills the cache. **Blocking.** Dedupes here, so
    /// a caller passing a path per *track* doesn't decode per duplicate, and caps at the capacity,
    /// so **pass paths in display order** to keep the visible prefix.
    pub fn prewarm(&self, paths: &[PathBuf]) {
        // Read once, so the filter and every decode agree on the size.
        let thumb_size = self.thumb_size();
        let (epoch, missing) = {
            let cache = self.cache.lock();
            let cap = cache.cap().get();
            let mut seen = HashSet::with_capacity(paths.len().min(cap));
            let missing: Vec<PathBuf> = paths
                .iter()
                .filter(|p| !holds(&cache, p.as_path(), thumb_size) && seen.insert(*p))
                .take(cap)
                .cloned()
                .collect();
            (self.epoch.load(Ordering::Relaxed), missing)
        };
        if missing.is_empty() {
            return;
        }
        let decode_all = move || {
            missing
                .into_par_iter()
                .map(|p| {
                    let entry = self.load(&p, thumb_size);
                    (p, entry)
                })
                .collect::<Vec<(PathBuf, Cached)>>()
        };
        let decoded: Vec<(PathBuf, Cached)> = match decode_pool() {
            Some(pool) => pool.install(decode_all),
            None => decode_all(),
        };
        let mut cache = self.cache.lock();
        // A release since the filter took this memory back.
        if self.epoch.load(Ordering::Relaxed) != epoch {
            return;
        }
        // The live size, as in the drain, and re-checked per key since a `get_or_load` may have
        // landed one meanwhile.
        let current = self.thumb_size();
        for (p, entry) in decoded {
            if !holds(&cache, p.as_path(), current) {
                cache.put(p, entry);
            }
        }
    }
}

/// Non-promoting, so asking can't reorder the prefix a `prewarm` warmed in display order.
fn holds(cache: &LruCache<PathBuf, Cached>, path: &Path, thumb_size: u32) -> bool {
    cache.peek(path).is_some_and(|held| held.serves(path, thumb_size))
}

fn buf_to_image(buf: &CachedBuf) -> Image {
    buf.ready().map(|b| Image::from_rgb8(b.clone())).unwrap_or_default()
}

fn decode_thumb(path: &Path, thumb_size: u32) -> Cached {
    let buf =
        decode_thumb_buffer(path, thumb_size).map_or_else(|| Decoded::failed(path), Decoded::Ready);
    Cached { buf, size: thumb_size }
}

fn decode_thumb_buffer(path: &Path, thumb_size: u32) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    // An unreadable header leaves the decode ungated, no worse than having no gate.
    let _oversized = source_pixels(path).and_then(large_decode_guard);

    let dyn_img = decode_capped_to(path, MAX_SOURCE_DIM, thumb_size).ok()?;

    // Square and aspect-blind: album art is square, and `image-fit: cover` would re-crop anyway.
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
