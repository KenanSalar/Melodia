//! Album-art decode + cache for the Album Detail header.
//!
//! The detail view needs the active cover in two forms — a sharp header tile
//! and a heavily-blurred, full-bleed hero backdrop. Both derive from the
//! *same* source image, so this module decodes it **once** per album and
//! produces both buffers from that single `DynamicImage`, held as an
//! [`ArtworkPair`] in one small LRU keyed by artwork path.
//!
//! Modelled on [`crate::ui::now_playing_artwork`] — same shape, same knobs,
//! same `Send`-able buffer return type so the decode can run inside
//! `tokio::task::spawn_blocking`, with the caller wrapping each half in a
//! `slint::Image` on the UI thread. Kept separate from both the row-tier
//! [`crate::media::cover_thumbs::CoverThumbs`] and the Albums grid-tier
//! cache for the same reason: these paired buffers are far larger, and would
//! evict the small tiles wholesale. The working set is the open album plus a
//! few recently-opened ones, so back-and-forth between cards stays a hit.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::fast_blur;
use lru::LruCache;
use parking_lot::Mutex;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};
use crate::state::AppState;
use crate::ui::util::{BLUR_TARGET, COVER_SIZE, buffer_from_rgb};

/// Height the cover is downscaled to before blurring, against the shared
/// [`BLUR_TARGET`] width. Source album art is always 1:1, but the hero region
/// paints landscape (full content-panel width × ~250 px tall), so a 3:2 buffer
/// matches the target aspect better than a square under `image-fit: cover` —
/// and squashing a square source into it is invisible after the blur.
const BLUR_HEIGHT: u32 = 128;

/// `fast_blur` sigma. Deliberately lighter than the shared
/// [`crate::ui::util::BLUR_SIGMA`]: the hero's gradient floor and crust scrim
/// sit on top of this blur, so it needs less of its own.
const BLUR_SIGMA: f32 = 20.0;

/// LRU capacity. The working set for the Album Detail view is the
/// currently-open album plus a handful of recently-opened ones (the
/// back-and-forth pattern). At one `(cover, blur)` pair per entry
/// ≈ `432 KiB + 72 KiB`, 12 entries caps at ≈ 6 MiB — comfortably under
/// the RSS ceiling.
const ARTWORK_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(12) {
    Some(n) => n,
    None => panic!("ARTWORK_CACHE_CAP > 0"),
};

/// A decoded artwork pair: the sharp foreground header tile and the
/// blurred hero backdrop, both derived from one source decode.
#[derive(Clone)]
pub struct ArtworkPair {
    /// Sharp, aspect-preserved cover tile (≤ `COVER_SIZE` on its long edge).
    pub cover: SharedPixelBuffer<Rgb8Pixel>,
    /// Heavily-blurred `BLUR_TARGET × BLUR_HEIGHT` landscape backdrop.
    pub blur: SharedPixelBuffer<Rgb8Pixel>,
}

/// `None` records a previously-attempted decode that failed — cached so a
/// broken cover file isn't re-opened on every album change.
type CachedArtwork = Option<ArtworkPair>;

pub struct DetailArtwork {
    cache: Mutex<LruCache<PathBuf, CachedArtwork>>,
}

impl Default for DetailArtwork {
    fn default() -> Self {
        Self {
            cache: Mutex::new(LruCache::new(ARTWORK_CACHE_CAP)),
        }
    }
}

impl DetailArtwork {
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
    /// the same broken file isn't retried on every album open. Hits
    /// promote the entry to most-recently-used.
    pub fn get_or_decode(&self, path: &Path) -> CachedArtwork {
        if let Some(cached) = self.cache.lock().get(path) {
            return cached.clone();
        }
        let pair = decode_artwork(path);
        let returned = pair.clone();
        self.cache.lock().put(path.to_path_buf(), pair);
        returned
    }

    /// Drop every cached `(cover, blur)` pair. Called when the user leaves
    /// the Albums section — the heavy buffers aren't needed while it's
    /// hidden, and the caller pairs this with `heap_trim::trim()` so glibc
    /// hands the freed pages back to the OS. Re-opening re-decodes.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }
}

/// A decoded `(cover, blur)` pair as raw RGB8 buffers, both `Send` so
/// they cross the `upgrade_in_event_loop` boundary. The detail views
/// carry this from the worker to the UI thread, which wraps each half
/// in a `slint::Image` via `Image::from_rgb8` (`Image` itself is
/// `!Send`, hence the raw-buffer form). `None` on either half means
/// missing / failed-decode artwork.
pub(crate) type DetailPair = (
    Option<SharedPixelBuffer<Rgb8Pixel>>,
    Option<SharedPixelBuffer<Rgb8Pixel>>,
);

/// Decode an entity's artwork into a [`DetailPair`] for a detail-view
/// header — the sharp header tile **and** the heavily-blurred hero
/// backdrop, both derived from one source decode. Off-loaded to the
/// `spawn_blocking` pool (image decode + box blur are CPU-bound).
/// Returns `(None, None)` for a missing / empty / failed-decode path;
/// the caller clears `has-blur` so the gradient floor shows through.
/// Shared by every detail view (Album / Artist / Playlist).
pub(crate) async fn decode_detail_pair(
    state: &AppState,
    artwork: Arc<DetailArtwork>,
    path: Option<String>,
) -> DetailPair {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return (None, None);
    };
    match state
        .runtime
        .spawn_blocking(move || artwork.get_or_decode(Path::new(&path)))
        .await
    {
        Ok(Some(pair)) => (Some(pair.cover), Some(pair.blur)),
        Ok(None) => (None, None),
        Err(e) => {
            log::warn!("detail artwork decode: {e}");
            (None, None)
        }
    }
}

fn decode_artwork(path: &Path) -> CachedArtwork {
    let decoded = decode_capped(path, MAX_SOURCE_DIM).ok()?;

    let cover = buffer_from_rgb(&decoded.thumbnail(COVER_SIZE, COVER_SIZE).to_rgb8());

    let small = decoded.thumbnail_exact(BLUR_TARGET, BLUR_HEIGHT).to_rgb8();
    let blur = buffer_from_rgb(&fast_blur(&small, BLUR_SIGMA));

    Some(ArtworkPair { cover, blur })
}
