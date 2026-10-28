//! The artwork store: a content-addressed cache of every cover the library draws.
//!
//! **Content-addressed** by a truncated BLAKE3 of the *stored* bytes, so one cover is one file
//! however many tracks carry it; hashing the source would make every writer's `exists()` guard
//! answer about bytes nobody wrote. **Atomically written**, so a name existing means a complete
//! file: a truncated one would never be rewritten, and `CoverThumbs` caches its failed decode.
//! **Bounded on ingest** at the largest tier's decode size, every tier decoding the stored file
//! whole, though [`normalized_bytes`] never inflates a source. **Swept, not refcounted**
//! ([`sweep`]), and only over names [`is_stored_name`] recognises, the directory holding whatever
//! else a user put there.

use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::picture::MimeType;
use lofty::tag::Tag;
use lru::LruCache;
use melodia_core::error::describe;
use parking_lot::Mutex;

use crate::media::image::image_decode::{self, FilterType, MAX_SOURCE_DIM, resize_rgb8};

pub mod composite;
pub mod sweep;

pub use composite::{compose_artwork, compose_cover};

/// Tees every byte into a BLAKE3 hasher, so `compose_artwork` can name its file by content without
/// holding the encoded JPEG in memory.
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

/// Shared by the three ingest memos. An entry is a couple of paths, and this many covers a large
/// library's distinct album directories.
const COVER_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(2_000) {
    Some(n) => n,
    None => panic!("COVER_CACHE_CAP > 0"),
};

/// Memos for artwork ingest, read through [`memoized_path`], so an album's cover is probed, read
/// and hashed once rather than once per track. An embedded picture is keyed by its hash, having no
/// file path to key on.
pub struct CoverCaches {
    dir_to_cover: Mutex<LruCache<PathBuf, Option<PathBuf>>>,
    cover_to_cached: Mutex<LruCache<PathBuf, Option<String>>>,
    source_to_stored: Mutex<LruCache<String, Option<String>>>,
}

pub type CoverCache = Arc<CoverCaches>;

pub fn new_cover_cache() -> CoverCache {
    Arc::new(CoverCaches {
        dir_to_cover: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
        cover_to_cached: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
        source_to_stored: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
    })
}

/// One memo's get-or-compute, unlocked around `compute`: two workers racing on a key duplicate the
/// work once rather than queueing behind each other's I/O.
///
/// **A hit naming a file that is gone is a miss**, the sweep having retired it since; one `stat`
/// is cheaper than a row naming a missing file until the next scan's repair. A memoized `None`
/// stays a hit, being a settled answer.
fn memoized_path<K, P>(
    tier: &Mutex<LruCache<K, Option<P>>>,
    key: K,
    compute: impl FnOnce(&K) -> Option<P>,
) -> Option<P>
where
    K: std::hash::Hash + Eq,
    P: AsRef<Path> + Clone,
{
    let hit = tier.lock().get(&key).cloned();
    if let Some(stored) = hit
        && stored.as_ref().is_none_or(|path| path.as_ref().exists())
    {
        return stored;
    }

    let computed = compute(&key);
    tier.lock().put(key, computed.clone());
    computed
}

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

fn persist_unless_exists(tmp: tempfile::NamedTempFile, file_path: &Path) -> io::Result<()> {
    if file_path.exists() {
        return Ok(());
    }
    tmp.persist(file_path).map(drop).map_err(|e| e.error)
}

/// Recreates `dir` if it has gone. Boot creates every store directory, so this only catches one
/// deleted mid-session, which would otherwise fail every write until restart.
fn stage_in(dir: &Path) -> io::Result<tempfile::NamedTempFile> {
    match tempfile::NamedTempFile::new_in(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dir)?;
            tempfile::NamedTempFile::new_in(dir)
        }
        staged => staged,
    }
}

fn write_atomic(dir: &Path, file_path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = stage_in(dir)?;
    tmp.write_all(bytes)?;
    persist_unless_exists(tmp, file_path)
}

/// Eight bytes of BLAKE3: a birthday bound far past any cover count, and short enough for a log.
const HASH_HEX_LEN: usize = 16;

/// Extensions a stored file may carry, closed so [`is_stored_name`] can be the writers' exact
/// inverse: what the cover pickers accept, lofty's picture MIME types, the JPEG composites and
/// artist images, and the favicon container `station_logo` admits. **The `image` features must
/// cover every one**, held by a test since the build says nothing.
const STORED_EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff", "ico"];

/// `raw` folded onto [`STORED_EXTENSIONS`], else JPEG. Lowercased so a `COVER.JPG` and a
/// `cover.jpg` holding the same bytes are one file; the fallback is free, every reader sniffing.
fn stored_extension(raw: &str) -> &'static str {
    let lowered = raw.to_ascii_lowercase();
    STORED_EXTENSIONS.iter().copied().find(|ext| *ext == lowered).unwrap_or("jpg")
}

/// The one definition of the store's naming scheme; [`is_stored_name`] is its inverse.
fn stored_name(hash_hex: &str, ext: &str) -> String {
    format!("{hash_hex}.{ext}")
}

/// Whether `name` is one the writers here could have produced, and so the sweep's to retire. The
/// extension matches case-insensitively because shipped stores hold `.JPG` names from before
/// [`stored_extension`] folded case.
pub(super) fn is_stored_name(name: &str) -> bool {
    let Some((hash_hex, ext)) = name.rsplit_once('.') else {
        return false;
    };
    hash_hex.len() == HASH_HEX_LEN
        && hash_hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && STORED_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext))
}

fn compute_hash(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hash.to_hex()[..HASH_HEX_LEN].to_string()
}

/// The first [`COVER_FILENAMES`] entry in the audio file's own directory, memoized per directory.
fn find_external_cover(file_path: &Path, cover_cache: &CoverCache) -> Option<PathBuf> {
    let dir = file_path.parent()?.to_path_buf();
    memoized_path(&cover_cache.dir_to_cover, dir, |dir| {
        COVER_FILENAMES.iter().map(|name| dir.join(name)).find(|path| path.exists())
    })
}

/// Longest edge the normalizer brings a stored image down to, and so the ceiling on every tier's
/// decode size. A full-resolution view would read the user's own file rather than raise this: the
/// store serves many small tiles, and at 4K it would run to gigabytes over a few thousand covers.
pub const STORE_MAX_DIM: u32 = 512;

/// A bound of its own, dimensions driving decode cost and bytes driving disk. Ordinary encodings
/// can't reach it inside the dimension cap; 16-bit PNG and uncompressed TIFF can.
const STORE_MAX_BYTES: usize = 1024 * 1024;

/// One quality for the normalizer and the composite alike, a composite being written straight into
/// the store, where the two disagreeing would re-encode it.
const STORE_JPEG_QUALITY: u8 = 90;

/// Lanczos3, as for the persisted composite: the file outlives the session and every tier reads it.
const STORE_FILTER: FilterType = FilterType::Lanczos3;

/// Writes `bytes` into `dir` under their content hash, bounded, deduplicated and atomically: the
/// one funnel for every byte-holding writer, so the store has one size policy. A source inside
/// both bounds is stored byte-identical, only its header read.
pub fn store_image(bytes: &[u8], ext: &str, dir: &Path) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let (width, height) = image_decode::memory_dimensions(bytes, MAX_SOURCE_DIM)?;

    let over_bounds =
        width > STORE_MAX_DIM || height > STORE_MAX_DIM || bytes.len() > STORE_MAX_BYTES;
    let normalized = over_bounds.then(|| normalized_bytes(bytes, width, height)).flatten();

    let (stored, ext) = match normalized.as_deref() {
        Some(jpeg) => (jpeg, "jpg"),
        None => (bytes, ext),
    };
    let file_path = dir.join(stored_name(&compute_hash(stored), ext));
    if !file_path.exists()
        && let Err(e) = write_atomic(dir, &file_path, stored)
    {
        log::warn!("Failed to write artwork cache {}: {}", file_path.display(), describe(&e));
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}

/// `bytes` re-encoded inside the store's bounds, or `None` where that wouldn't pay.
///
/// **Never larger than the source**, so a cap set a little low can waste CPU but never grow the
/// store. A source over only the byte bound keeps its dimensions and is re-encoded in place.
fn normalized_bytes(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let decoded = image_decode::decode_memory_capped(bytes, MAX_SOURCE_DIM)?;
    let (target_w, target_h) =
        image_decode::fit_within(width, height, STORE_MAX_DIM, STORE_MAX_DIM);
    let resized = resize_rgb8(&decoded, target_w, target_h, STORE_FILTER)?;
    let encoded = image_decode::encode_jpeg(resized, STORE_JPEG_QUALITY).ok()?;

    (encoded.len() < bytes.len()).then_some(encoded)
}

pub fn cache_image_file(source_path: &Path, artwork_dir: &Path) -> Option<String> {
    let data = std::fs::read(source_path).ok()?;
    let ext = source_path.extension().and_then(|e| e.to_str()).map_or("jpg", stored_extension);
    store_image(&data, ext, artwork_dir)
}

/// The tag's best cover picture, stored. Memoized on the *source* bytes: an over-cap cover is
/// re-encoded before [`store_image`]'s `exists()` guard is reachable, and every track on an album
/// carries the same picture.
pub fn extract_and_cache_artwork(
    tag: &Tag,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
) -> Option<String> {
    use lofty::picture::PictureType;

    let pictures = tag.pictures();
    if pictures.is_empty() {
        return None;
    }

    let picture = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.iter().find(|p| p.pic_type() == PictureType::CoverBack))
        .or(pictures.first())?;

    let ext = match picture.mime_type() {
        Some(MimeType::Png) => "png",
        Some(MimeType::Bmp) => "bmp",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Tiff) => "tiff",
        _ => "jpg",
    };

    memoized_path(&cover_cache.source_to_stored, compute_hash(picture.data()), |_| {
        store_image(picture.data(), ext, artwork_dir)
    })
}

/// A cover file beside the track first, then the tag's embedded art.
pub fn find_and_cache_artwork(
    file_path: &Path,
    tag: Option<&Tag>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
) -> Option<String> {
    if let Some(cover_path) = find_external_cover(file_path, cover_cache) {
        let cached = memoized_path(&cover_cache.cover_to_cached, cover_path, |path| {
            cache_image_file(path, artwork_dir)
        });
        if cached.is_some() {
            return cached;
        }
    }

    tag.and_then(|t| extract_and_cache_artwork(t, artwork_dir, cover_cache))
}

/// Resizes to cover `width` × `height`, centre-cropping the overflow. `filter` is the caller's:
/// Lanczos3's support grows with the downscale ratio, worth paying for a file that outlives the
/// session and not for a banner recomposed on every top-four change.
fn resize_to_cover(
    img: &image::DynamicImage,
    width: u32,
    height: u32,
    filter: FilterType,
) -> Option<image::RgbImage> {
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let (tw, th) = (f64::from(width), f64::from(height));

    let scale = (tw / iw).max(th / ih);
    let scaled_w = f64_to_pixel(iw * scale);
    let scaled_h = f64_to_pixel(ih * scale);

    let resized = resize_rgb8(img, scaled_w, scaled_h, filter)?;

    let x = (scaled_w.saturating_sub(width)) / 2;
    let y = (scaled_h.saturating_sub(height)) / 2;
    Some(image::imageops::crop_imm(&resized, x, y, width, height).to_image())
}

/// Saturating `f64` → pixel count, for [`resize_to_cover`]'s two scaled sides.
#[expect(
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
