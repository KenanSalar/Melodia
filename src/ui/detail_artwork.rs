//! Album-art decode + cache for the Album Detail header.
//!
//! The detail view needs the active cover in two forms — a sharp
//! `tile-size`-rendered header tile and a heavily-blurred, full-bleed hero
//! backdrop. Both derive from the *same* source image, so this module
//! decodes it **once** per album and produces both buffers from that single
//! `DynamicImage`. The result is an [`ArtworkPair`] held in one small LRU
//! keyed by artwork path.
//!
//! Modelled on [`crate::ui::now_playing_artwork`] — same shape, same knobs,
//! same `Send`-able buffer return type so the decode can run inside
//! `tokio::task::spawn_blocking`. The caller wraps each half in a
//! `slint::Image` on the UI thread.
//!
//! This is deliberately a **separate, small** cache from
//! [`crate::media::cover_thumbs::CoverThumbs`] (the row-tier 72 px LRU) and
//! from the Albums grid-tier cover cache: mixing 384 px + 128 px paired
//! buffers into either would pollute it and waste memory. The working set
//! here is the currently-open album plus a handful of recently-opened ones
//! — back-and-forth between cards stays a cache hit.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::fast_blur;
use lru::LruCache;
use parking_lot::Mutex;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::state::AppState;

/// Side length (px) the sharp cover tile is downscaled to. Matches
/// [`crate::ui::now_playing_artwork`]'s `COVER_SIZE` and the on-screen
/// header tile's `clamp(width*0.22, 200px, 380px)` ceiling — `image-fit:
/// cover` + Skia mipmaps keep it crisp without the memory cost of a 2×
/// `HiDPI` buffer. Each cover buffer is at most `384 × 384 × 3 ≈ 432 KiB`.
const COVER_SIZE: u32 = 384;

/// Width / height the cover is downscaled to before blurring. Source
/// album art is always 1:1, but the detail-view hero region paints
/// landscape (full content-panel width × ~250 px tall), so a 3:2
/// landscape source matches the target aspect better than a square
/// when `image-fit: cover`-stretched. Downscaling first makes the blur
/// cheap (box-pass cost scales with pixel count) and bounds the
/// blurred buffer at `192 × 128 × 3 ≈ 72 KiB`. Squashing a square
/// source into the landscape buffer is invisible after the heavy blur.
const BLUR_WIDTH: u32 = 192;
const BLUR_HEIGHT: u32 = 128;

/// `fast_blur` sigma. Approximates a `blur(50px)` CSS filter — at this
/// downscale it reads as a soft wash of colour with no recognisable
/// shapes. Slightly lighter than `now_playing_artwork::BLUR_SIGMA`
/// (24.0 ≈ blur(60px)) — the detail hero's gradient floor + crust
/// scrim sit on top, so a softer blur is enough.
const BLUR_SIGMA: f32 = 20.0;

/// Hard cap on accepted source resolution — mirrors `CoverThumbs`'s guard
/// so a forged dimension header in a tag can't trigger an absurd
/// allocation before we get a chance to downscale.
const MAX_SOURCE_DIM: u32 = 8192;

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
    /// Heavily-blurred `BLUR_WIDTH × BLUR_HEIGHT` landscape backdrop.
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
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);

    let decoded = reader.decode().ok()?;

    let cover = buffer_from_rgb(&decoded.thumbnail(COVER_SIZE, COVER_SIZE).to_rgb8());

    let small = decoded
        .thumbnail_exact(BLUR_WIDTH, BLUR_HEIGHT)
        .to_rgb8();
    let blur = buffer_from_rgb(&fast_blur(&small, BLUR_SIGMA));

    Some(ArtworkPair { cover, blur })
}

fn buffer_from_rgb(img: &image::RgbImage) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = img.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    buf
}
