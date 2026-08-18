use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::FilterType;
use lofty::picture::MimeType;
use lofty::tag::Tag;
use lru::LruCache;
use parking_lot::Mutex;

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};

/// Pipes every byte to both the wrapped writer and a BLAKE3 hasher. Used by
/// `compose_artwork` to encode a JPEG into a temp file while computing its
/// content hash on the fly — so the dedup filename can be derived without
/// holding the full encoded buffer in RAM.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// LRU cap shared by both external-cover caches. Each entry stores a couple
/// of `PathBuf`s / a short `String`, so 2 000 entries is well under 1 MB but
/// covers a realistic distinct-album directory count for a large library.
/// Older entries are evicted on insert once the cap is reached, so neither
/// cache grows unbounded as a user browses lots of folders.
const COVER_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(2_000) {
    Some(n) => n,
    None => panic!("COVER_CACHE_CAP > 0"),
};

/// Filesystem memoization for external cover resolution, two tiers:
///
/// * `dir_to_cover` — parent directory → which cover filename (if any)
///   lives in it, saving the per-track directory probe.
/// * `cover_to_cached` — cover file path → result of [`cache_image_file`]
///   (the deduplicated artwork-cache path, or `None` for an unreadable /
///   empty file). Every track in an album directory resolves to the same
///   cover file, so without this tier the cover is fully re-read and
///   re-hashed once per *track* instead of once per *cover*.
///
/// Locks are held only around LRU get/put — never across the file
/// read+hash itself — so Rayon scan workers don't serialize behind I/O.
pub struct CoverCaches {
    dir_to_cover: Mutex<LruCache<PathBuf, Option<PathBuf>>>,
    cover_to_cached: Mutex<LruCache<PathBuf, Option<String>>>,
}

pub type CoverCache = Arc<CoverCaches>;

/// Build a fresh cover cache with the standard caps. Use this everywhere a
/// `CoverCache` is constructed so the caps stay consistent.
pub fn new_cover_cache() -> CoverCache {
    Arc::new(CoverCaches {
        dir_to_cover: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
        cover_to_cached: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
    })
}

/// Common cover art filenames to look for in the audio file's directory.
const COVER_FILENAMES: &[&str] = &[
    "cover.jpg",
    "cover.png",
    "cover.jpeg",
    "folder.jpg",
    "folder.png",
    "folder.jpeg",
    "front.jpg",
    "front.png",
    "front.jpeg",
    "album.jpg",
    "album.png",
    "album.jpeg",
    "AlbumArt.jpg",
    "AlbumArt.png",
];

/// Creates the artwork cache directory if it doesn't exist.
pub fn init_artwork_cache(app_data_dir: &Path) -> io::Result<PathBuf> {
    let artwork_dir = app_data_dir.join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;
    Ok(artwork_dir)
}

/// Creates the artists image cache directory if it doesn't exist.
pub fn init_artists_cache(app_data_dir: &Path) -> io::Result<PathBuf> {
    let artists_dir = app_data_dir.join("artists");
    std::fs::create_dir_all(&artists_dir)?;
    Ok(artists_dir)
}

/// Computes a truncated BLAKE3 hash (first 8 bytes = 16 hex chars) of the given data.
pub(crate) fn compute_hash(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hash.to_hex()[..16].to_string()
}

/// Searches the audio file's parent directory for a common cover art file.
/// Uses a per-directory cache to avoid redundant filesystem lookups.
fn find_external_cover(file_path: &Path, cover_cache: &CoverCache) -> Option<PathBuf> {
    let dir = file_path.parent()?;
    let dir_buf = dir.to_path_buf();

    // Check cache first. `LruCache::get` requires `&mut self`, so the lock
    // is held briefly while the entry is promoted to most-recently-used.
    if let Some(cached) = cover_cache.dir_to_cover.lock().get(&dir_buf) {
        return cached.clone();
    }

    // Scan directory for cover art
    let mut result = None;
    for name in COVER_FILENAMES {
        let path = dir.join(name);
        if path.exists() {
            result = Some(path);
            break;
        }
    }

    // Cache the result (even None, to avoid re-scanning directories without
    // covers). Evicts the LRU entry if at capacity.
    cover_cache.dir_to_cover.lock().put(dir_buf, result.clone());

    result
}

/// Copies an external image file into the artwork cache, deduplicating by content hash.
pub(crate) fn cache_image_file(source_path: &Path, artwork_dir: &Path) -> Option<String> {
    let data = std::fs::read(source_path).ok()?;
    if data.is_empty() {
        return None;
    }
    let ext = source_path.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
    let hash_hex = compute_hash(&data);
    let filename = format!("{hash_hex}.{ext}");
    let file_path = artwork_dir.join(&filename);
    if !file_path.exists()
        && let Err(e) = std::fs::write(&file_path, &data)
    {
        log::warn!("Failed to write artwork cache {}: {}", file_path.display(), e);
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}

/// Extracts the best cover picture from a lofty tag, hashes it with BLAKE3,
/// and saves it to the artwork cache directory. Returns the absolute path
/// to the cached image file, or None if no picture is found.
pub fn extract_and_cache_artwork(tag: &Tag, artwork_dir: &Path) -> Option<String> {
    use lofty::picture::PictureType;

    let pictures = tag.pictures();
    if pictures.is_empty() {
        return None;
    }

    // Priority: CoverFront > CoverBack > first available
    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.iter().find(|p| p.pic_type() == PictureType::CoverBack))
        .or(pictures.first())?;

    let data = picture.data();
    if data.is_empty() {
        return None;
    }

    let ext = match picture.mime_type() {
        Some(MimeType::Png) => "png",
        Some(MimeType::Bmp) => "bmp",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Tiff) => "tiff",
        // Unknown / explicit JPEG → fall back to "jpg" extension.
        _ => "jpg",
    };

    let hash_hex = compute_hash(data);
    let filename = format!("{hash_hex}.{ext}");
    let file_path = artwork_dir.join(&filename);

    // Skip write if file already exists (dedup)
    if !file_path.exists()
        && let Err(e) = std::fs::write(&file_path, data)
    {
        log::warn!("Failed to write artwork cache {}: {}", file_path.display(), e);
        return None;
    }

    Some(file_path.to_string_lossy().into_owned())
}

/// Unified artwork lookup: checks external cover files first, then embedded tag artwork.
/// Returns the cached artwork path, or None if no artwork is found from either source.
pub fn find_and_cache_artwork(
    file_path: &Path,
    tag: Option<&Tag>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
) -> Option<String> {
    // 1. External cover art files (cover.jpg, folder.jpg, etc.). The
    //    read+hash+copy result is memoized per cover path so an album's
    //    cover is processed once, not once per track in its directory. A
    //    memoized `None` (unreadable / empty cover) also skips the re-read
    //    and falls through to embedded artwork, matching the uncached
    //    behavior.
    if let Some(cover_path) = find_external_cover(file_path, cover_cache) {
        let memo = cover_cache.cover_to_cached.lock().get(&cover_path).cloned();
        let cached = if let Some(result) = memo {
            result
        } else {
            // Lock dropped during the read+hash — two Rayon workers
            // racing on the same cover may duplicate the work once,
            // but never queue behind each other's I/O.
            let result = cache_image_file(&cover_path, artwork_dir);
            cover_cache.cover_to_cached.lock().put(cover_path, result.clone());
            result
        };
        if let Some(cached) = cached {
            return Some(cached);
        }
    }

    // 2. Embedded tag artwork
    tag.and_then(|t| extract_and_cache_artwork(t, artwork_dir))
}

/// Resizes an image to cover a target region, center-cropping to fit exactly.
///
/// `filter` is the caller's because the two collages want different trades: Lanczos3's support in
/// *source* pixels grows with the downscale ratio, which a file that outlives the session is worth
/// paying and a banner recomposed per top-4 change is not.
fn resize_to_cover(
    img: &image::DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> image::RgbImage {
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let (tw, th) = (f64::from(width), f64::from(height));

    // Scale so the image fully covers the target region
    let scale = (tw / iw).max(th / ih);
    let scaled_w = f64_to_pixel(iw * scale);
    let scaled_h = f64_to_pixel(ih * scale);

    let resized = img.resize_exact(scaled_w, scaled_h, filter);

    // Center-crop to target dimensions
    let x = (scaled_w.saturating_sub(width)) / 2;
    let y = (scaled_h.saturating_sub(height)) / 2;
    resized.crop_imm(x, y, width, height).to_rgb8()
}

/// Side of the collage [`compose_artwork`] persists. Sized so the playlist thumbnail survives a
/// detail hero at full resolution — [`compose_cover`] takes its side from the caller instead, the
/// hero's largest consumer being a tile it would only have to resize a second time.
pub(crate) const COMPOSITE_SIZE: u32 = 600;

/// Resampler for the curated heroes' collage. Triangle rather than the Lanczos3 the persisted one
/// takes: this canvas is downscaled again on the way to a cover tile and drawn at a fraction of
/// that, so the sharper filter's extra source taps land in pixels no surface ever resolves.
const HERO_FILTER: FilterType = FilterType::Triangle;

/// Destination `(x, y, width, height)` per source, in **half-canvas units**, indexed by `len - 1`.
/// Half-units so one table serves both canvas sizes; data rather than a `match` arm each, so the
/// decode loop can walk it one source at a time and never holds four full-size decodes.
const COMPOSITE_LAYOUTS: [&[(u32, u32, u32, u32)]; 4] = [
    // the whole canvas
    &[(0, 0, 2, 2)],
    // left | right
    &[(0, 0, 1, 2), (1, 0, 1, 2)],
    // left | right top over right bottom
    &[(0, 0, 1, 2), (1, 0, 1, 1), (1, 1, 1, 1)],
    // 2x2
    &[(0, 0, 1, 1), (1, 0, 1, 1), (0, 1, 1, 1), (1, 1, 1, 1)],
];

/// Composes 1-4 source images into a single `side`-by-`side` square, laid out by
/// [`COMPOSITE_LAYOUTS`]. `side` wants to be the size the collage is actually *drawn* at: this one
/// goes straight to `pair_from_image`, which reduces it to a cover tile, so a larger canvas is
/// resized twice and drawn once.
///
/// **A source that won't decode drops out of the set rather than failing the compose** — the layout
/// is picked from what survives, so three readable covers give a three-up collage and only an
/// entirely unreadable set reads as no artwork. The curated heroes recompose from whatever the
/// database's top four currently are, and a cover deleted under them should cost that slot rather
/// than the whole banner.
///
/// **Blocking** — call from `spawn_blocking` or a Rayon worker, never the UI thread.
pub(crate) fn compose_cover(source_paths: &[PathBuf], side: u32) -> Option<image::RgbImage> {
    let mut sources: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();

    loop {
        match compose_exact(&sources, side, HERO_FILTER) {
            Ok(canvas) => return Some(canvas),
            Err(ComposeStop::NoLayout) => return None,
            // The pass names what it stopped on, so the retry drops exactly that. A separate
            // readability filter would decode every path again only to learn what this already
            // knows — and could not spot a source the *cap* refuses, whose header reads fine.
            Err(ComposeStop::Unreadable(index)) => {
                sources.remove(index);
            }
        }
    }
}

/// The rect list for a set of `len` sources, or `None` where there is no layout for that many.
fn layout_for(len: usize) -> Option<&'static [(u32, u32, u32, u32)]> {
    COMPOSITE_LAYOUTS.get(len.checked_sub(1)?).copied()
}

/// Why [`compose_exact`] gave up, which the two callers answer differently.
enum ComposeStop {
    /// No layout for this many sources — an empty set, or more than [`COMPOSITE_LAYOUTS`] covers.
    /// Decided before anything is decoded.
    NoLayout,
    /// The source at this index would not decode.
    Unreadable(usize),
}

/// One all-or-nothing pass, holding a single decode at a time. The layout is in half-canvas units,
/// so `side` scales it; both sides are even, so the scale is exact.
fn compose_exact(
    sources: &[&Path],
    side: u32,
    filter: FilterType,
) -> Result<image::RgbImage, ComposeStop> {
    let rects = layout_for(sources.len()).ok_or(ComposeStop::NoLayout)?;
    // An odd side leaves the rects a pixel short of the canvas they were scaled against, so the
    // collage carries an unpainted hairline down two edges — right on both of today's callers and
    // silently wrong the day one of them derives its side from the display.
    debug_assert!(side >= 2 && side.is_multiple_of(2), "the half-unit layout needs an even side");
    let half = side / 2;

    let mut canvas = image::RgbImage::new(side, side);
    for (index, (path, &(x, y, width, height))) in sources.iter().zip(rects).enumerate() {
        let source =
            decode_capped(path, MAX_SOURCE_DIM).map_err(|_| ComposeStop::Unreadable(index))?;
        let tile = resize_to_cover(&source, width * half, height * half, filter);
        image::imageops::overlay(&mut canvas, &tile, i64::from(x * half), i64::from(y * half));
    }
    Ok(canvas)
}

/// [`compose_exact`] persisted into `artwork_dir` under its own content hash, returning the cached
/// path or `None` on failure.
///
/// **Strict where [`compose_cover`] is lenient**: this bakes a file the mosaic picker has already
/// previewed slot for slot, so a source quietly dropping out would persist a collage that isn't the
/// one the user chose.
pub(crate) fn compose_artwork(source_paths: &[PathBuf], artwork_dir: &Path) -> Option<String> {
    let sources: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();
    let canvas = compose_exact(&sources, COMPOSITE_SIZE, FilterType::Lanczos3).ok()?;

    // Stream the JPEG into a temp file in the artwork directory while a tee
    // writer feeds every byte to a BLAKE3 hasher. This avoids holding the
    // entire encoded JPEG in RAM (composite mosaics are typically ~50 KB but
    // the scanner can issue several in parallel during a large library scan).
    let tmp = match tempfile::NamedTempFile::new_in(artwork_dir) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("Failed to create composite tempfile: {e}");
            return None;
        }
    };
    let hash_hex = {
        let mut hashing = HashingWriter::new(BufWriter::new(tmp.as_file()));
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut hashing, 90);
        if let Err(e) = image::DynamicImage::ImageRgb8(canvas).write_with_encoder(encoder) {
            log::warn!("Failed to encode composite JPEG: {e}");
            return None;
        }
        if let Err(e) = hashing.flush() {
            log::warn!("Failed to flush composite JPEG: {e}");
            return None;
        }
        hashing.hasher.finalize().to_hex()[..16].to_string()
    };
    let filename = format!("{hash_hex}.jpg");
    let file_path = artwork_dir.join(&filename);

    if file_path.exists() {
        // Already cached — drop the temp file (NamedTempFile cleans up on drop).
        return Some(file_path.to_string_lossy().into_owned());
    }
    if let Err(e) = tmp.persist(&file_path) {
        log::warn!("Failed to write composite artwork {}: {}", file_path.display(), e);
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}

/// Convert a non-negative f64 pixel coordinate (post `.round()` from image
/// resize math) into a `u32`. Saturates to `u32::MAX` if the value
/// somehow overflows; clamps NaN/negatives to 0.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "image dimensions stay well below u32::MAX in practice; this helper is the saturating boundary"
)]
fn f64_to_pixel(v: f64) -> u32 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        v.round() as u32
    }
}

#[cfg(test)]
#[path = "tests/artwork_tests.rs"]
mod tests;
