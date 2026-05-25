use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::picture::MimeType;
use lofty::tag::Tag;
use lru::LruCache;
use parking_lot::Mutex;

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
        Self { inner, hasher: blake3::Hasher::new() }
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

/// LRU cap for the parent-directory → external-cover-path resolution cache.
/// Each entry stores at most two `PathBuf`s (key + the resolved cover path),
/// so 2 000 entries is well under 1 MB but covers a realistic distinct-album
/// directory count for a large library. Older entries are evicted on insert
/// once the cap is reached, so the cache no longer grows unbounded as a user
/// browses lots of folders.
const COVER_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(2_000) {
    Some(n) => n,
    None => panic!("COVER_CACHE_CAP > 0"),
};

pub type CoverCache = Arc<Mutex<LruCache<PathBuf, Option<PathBuf>>>>;

/// Build a fresh cover cache with the standard cap. Use this everywhere a
/// `CoverCache` is constructed so the cap stays consistent.
pub fn new_cover_cache() -> CoverCache {
    Arc::new(Mutex::new(LruCache::new(COVER_CACHE_CAP)))
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
    if let Some(cached) = cover_cache.lock().get(&dir_buf) {
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
    cover_cache.lock().put(dir_buf, result.clone());

    result
}

/// Copies an external image file into the artwork cache, deduplicating by content hash.
pub(crate) fn cache_image_file(source_path: &Path, artwork_dir: &Path) -> Option<String> {
    let data = std::fs::read(source_path).ok()?;
    if data.is_empty() {
        return None;
    }
    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");
    let hash_hex = compute_hash(&data);
    let filename = format!("{hash_hex}.{ext}");
    let file_path = artwork_dir.join(&filename);
    if !file_path.exists()
        && let Err(e) = std::fs::write(&file_path, &data)
    {
        log::warn!(
            "Failed to write artwork cache {}: {}",
            file_path.display(),
            e
        );
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
        .or_else(|| {
            pictures
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverBack)
        })
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
        log::warn!(
            "Failed to write artwork cache {}: {}",
            file_path.display(),
            e
        );
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
    // 1. External cover art files (cover.jpg, folder.jpg, etc.)
    if let Some(cover_path) = find_external_cover(file_path, cover_cache)
        && let Some(cached) = cache_image_file(&cover_path, artwork_dir)
    {
        return Some(cached);
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

/// Composes 1-4 source images into a single 600x600 composite image.
///
/// Layouts:
/// - 1 image: fills the entire canvas
/// - 2 images: left/right halves
/// - 3 images: left half = image 1, right top/bottom = images 2, 3
/// - 4 images: 2x2 grid
///
/// Returns the cached path to the composite image, or None on failure.
pub(crate) fn compose_artwork(source_paths: &[PathBuf], artwork_dir: &Path) -> Option<String> {
    use image::{DynamicImage, RgbImage};

    const SIZE: u32 = 600;

    if source_paths.is_empty() || source_paths.len() > 4 {
        return None;
    }

    // Load all source images
    let images: Vec<DynamicImage> = source_paths
        .iter()
        .filter_map(|p| image::open(p).ok())
        .collect();

    if images.len() != source_paths.len() {
        return None;
    }

    let half = SIZE / 2;
    let mut canvas = RgbImage::new(SIZE, SIZE);

    match images.len() {
        1 => {
            let cropped = resize_to_cover(&images[0], SIZE, SIZE);
            image::imageops::overlay(&mut canvas, &cropped, 0, 0);
        }
        2 => {
            let left = resize_to_cover(&images[0], half, SIZE);
            let right = resize_to_cover(&images[1], half, SIZE);
            image::imageops::overlay(&mut canvas, &left, 0, 0);
            image::imageops::overlay(&mut canvas, &right, i64::from(half), 0);
        }
        3 => {
            let left = resize_to_cover(&images[0], half, SIZE);
            let rt = resize_to_cover(&images[1], half, half);
            let rb = resize_to_cover(&images[2], half, half);
            image::imageops::overlay(&mut canvas, &left, 0, 0);
            image::imageops::overlay(&mut canvas, &rt, i64::from(half), 0);
            image::imageops::overlay(&mut canvas, &rb, i64::from(half), i64::from(half));
        }
        4 => {
            let tl = resize_to_cover(&images[0], half, half);
            let tr = resize_to_cover(&images[1], half, half);
            let bl = resize_to_cover(&images[2], half, half);
            let br = resize_to_cover(&images[3], half, half);
            image::imageops::overlay(&mut canvas, &tl, 0, 0);
            image::imageops::overlay(&mut canvas, &tr, i64::from(half), 0);
            image::imageops::overlay(&mut canvas, &bl, 0, i64::from(half));
            image::imageops::overlay(&mut canvas, &br, i64::from(half), i64::from(half));
        }
        _ => return None,
    }

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
        if let Err(e) = DynamicImage::ImageRgb8(canvas).write_with_encoder(encoder) {
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
