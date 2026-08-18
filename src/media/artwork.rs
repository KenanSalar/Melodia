use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
fn resize_to_cover(img: &image::DynamicImage, width: u32, height: u32) -> image::RgbImage {
    use image::imageops::FilterType;

    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let (tw, th) = (f64::from(width), f64::from(height));

    // Scale so the image fully covers the target region
    let scale = (tw / iw).max(th / ih);
    let scaled_w = f64_to_pixel(iw * scale);
    let scaled_h = f64_to_pixel(ih * scale);

    let resized = img.resize_exact(scaled_w, scaled_h, FilterType::Lanczos3);

    // Center-crop to target dimensions
    let x = (scaled_w.saturating_sub(width)) / 2;
    let y = (scaled_h.saturating_sub(height)) / 2;
    resized.crop_imm(x, y, width, height).to_rgb8()
}

/// Side of the composed collage. Every consumer draws it far smaller — it is sized
/// so the playlist thumbnail it persists survives a detail hero at full resolution.
pub(crate) const COMPOSITE_SIZE: u32 = 600;

const COMPOSITE_HALF: u32 = COMPOSITE_SIZE / 2;

/// Destination `(x, y, width, height)` per source, indexed by `len - 1`.
///
/// Data rather than a `match` arm each, so the decode loop can walk it one source at
/// a time; a four-cover collage never holds four full-size decodes at once.
const COMPOSITE_LAYOUTS: [&[(u32, u32, u32, u32)]; 4] = [
    // the whole canvas
    &[(0, 0, COMPOSITE_SIZE, COMPOSITE_SIZE)],
    // left | right
    &[
        (0, 0, COMPOSITE_HALF, COMPOSITE_SIZE),
        (COMPOSITE_HALF, 0, COMPOSITE_HALF, COMPOSITE_SIZE),
    ],
    // left | right top over right bottom
    &[
        (0, 0, COMPOSITE_HALF, COMPOSITE_SIZE),
        (COMPOSITE_HALF, 0, COMPOSITE_HALF, COMPOSITE_HALF),
        (COMPOSITE_HALF, COMPOSITE_HALF, COMPOSITE_HALF, COMPOSITE_HALF),
    ],
    // 2x2
    &[
        (0, 0, COMPOSITE_HALF, COMPOSITE_HALF),
        (COMPOSITE_HALF, 0, COMPOSITE_HALF, COMPOSITE_HALF),
        (0, COMPOSITE_HALF, COMPOSITE_HALF, COMPOSITE_HALF),
        (COMPOSITE_HALF, COMPOSITE_HALF, COMPOSITE_HALF, COMPOSITE_HALF),
    ],
];

/// Composes 1-4 source images into a single [`COMPOSITE_SIZE`] square.
///
/// Layouts:
/// - 1 image: fills the entire canvas
/// - 2 images: left/right halves
/// - 3 images: left half = image 1, right top/bottom = images 2, 3
/// - 4 images: 2x2 grid
///
/// **A source that won't decode drops out of the set rather than failing the compose** — the
/// layout is picked from what survives, so three readable covers give a three-up collage and only
/// an entirely unreadable set reads as no artwork. The curated heroes recompose from whatever the
/// database's top four currently are, and a cover deleted under them should cost that slot rather
/// than the whole banner. The retry re-decodes; nothing reaches it while every file is where the
/// database says it is.
///
/// **Blocking** — call from `spawn_blocking` or a Rayon worker, never the UI thread.
pub(crate) fn compose_cover(source_paths: &[PathBuf]) -> Option<image::RgbImage> {
    let all: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();
    // Refused for its size, and dropping a source is not how a set gets a layout — so the
    // readability pass below would decode every path to reach the same answer.
    layout_for(all.len())?;

    if let Some(canvas) = compose_exact(&all) {
        return Some(canvas);
    }

    let readable: Vec<&Path> =
        all.iter().copied().filter(|p| decode_capped(p, MAX_SOURCE_DIM).is_ok()).collect();
    // Everything decoded this time, so whatever failed above lost a race rather than being
    // unreadable — and `compose_exact(&readable)` is then the call that just failed, for a third
    // walk of the same paths.
    if readable.len() == all.len() {
        return None;
    }
    compose_exact(&readable)
}

/// The rect list for a set of `len` sources, or `None` where there is no layout for that many.
fn layout_for(len: usize) -> Option<&'static [(u32, u32, u32, u32)]> {
    COMPOSITE_LAYOUTS.get(len.checked_sub(1)?).copied()
}

/// One all-or-nothing pass, holding a single decode at a time — a four-cover collage never has
/// four full-size sources in memory at once.
fn compose_exact(sources: &[&Path]) -> Option<image::RgbImage> {
    let rects = layout_for(sources.len())?;

    let mut canvas = image::RgbImage::new(COMPOSITE_SIZE, COMPOSITE_SIZE);
    for (path, &(x, y, width, height)) in sources.iter().zip(rects) {
        let source = decode_capped(path, MAX_SOURCE_DIM).ok()?;
        let tile = resize_to_cover(&source, width, height);
        image::imageops::overlay(&mut canvas, &tile, i64::from(x), i64::from(y));
    }
    Some(canvas)
}

/// [`compose_exact`] persisted into `artwork_dir` under its own content hash.
///
/// **Strict where [`compose_cover`] is lenient**: this bakes a file the mosaic picker has already
/// previewed slot for slot, so a source quietly dropping out would persist a collage that isn't
/// the one the user chose.
///
/// Returns the cached path to the composite image, or None on failure.
pub(crate) fn compose_artwork(source_paths: &[PathBuf], artwork_dir: &Path) -> Option<String> {
    let sources: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();
    let canvas = compose_exact(&sources)?;

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
