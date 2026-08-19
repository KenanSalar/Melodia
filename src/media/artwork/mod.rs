//! The artwork store: a content-addressed cache of every cover the library draws.
//!
//! Four properties, and each exists because its absence was a bug rather than a preference.
//!
//! **Content-addressed.** The filename is a truncated BLAKE3 of the *stored* bytes, so identical
//! artwork is one file however many tracks carry it — which is also what makes one path-keyed
//! entry downstream serve them all. Hashing the source instead would make the `exists()` guard
//! every writer leans on answer about bytes nobody wrote.
//!
//! **Atomically written.** Staged beside the destination and renamed, so the final name existing
//! means the file is complete. Without that a crash mid-write leaves a truncated file under a name
//! no writer will ever revisit, and the cover is gone for good — `CoverThumbs` caches the failed
//! decode and stops even retrying.
//!
//! **Bounded on ingest.** Every tier decodes the stored file *whole* before resizing it, so this
//! cap governs every transient decode buffer in the app, not just disk. It sits at the largest
//! tier's decode size; a source inside it is stored byte-identical and never decoded at all. The
//! one thing it yields to is [`normalized_bytes`]'s never-inflate rule, which keeps a source the
//! re-encode would have grown — so the cap is where the store *aims*, not a hard ceiling. A
//! full-resolution artwork view would read the user's own file rather than raise it.
//!
//! **Swept, not refcounted.** Artwork is shared, so a per-track delete must not unlink the file;
//! [`sweep`] collects against the whole reference set instead. It is gated on
//! [`is_stored_name`] — the inverse of the naming scheme here, which is why the two live in one
//! file — and the store directory holds whatever else a user has put there.
//!
//! Every byte is rederivable from the user's own files, which is what makes deleting aggressively
//! safe. Nothing here ever writes back to those files: `tag_writer` embeds a picked cover's
//! original bytes, so a capped store never caps what lands in a tag.

use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::picture::MimeType;
use lofty::tag::Tag;
use lru::LruCache;
use parking_lot::Mutex;

use crate::media::image_decode::{self, FilterType, MAX_SOURCE_DIM, decode_capped, resize_rgb8};

pub mod sweep;

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

/// LRU cap shared by all three cover caches. Each entry stores a couple
/// of `PathBuf`s / a short `String`, so each tier at its cap is well under
/// 1 MB, and 2 000 entries covers a realistic distinct-album directory count
/// for a large library. Older entries are evicted on insert once the cap is
/// reached, so none of them grows unbounded as a user browses lots of folders.
const COVER_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(2_000) {
    Some(n) => n,
    None => panic!("COVER_CACHE_CAP > 0"),
};

/// Memoization for artwork ingest, three tiers:
///
/// * `dir_to_cover` — parent directory → which cover filename (if any)
///   lives in it, saving the per-track directory probe.
/// * `cover_to_cached` — cover file path → result of [`cache_image_file`]
///   (the deduplicated artwork-cache path, or `None` for an unreadable /
///   empty file). Every track in an album directory resolves to the same
///   cover file, so without this tier the cover is fully re-read and
///   re-hashed once per *track* instead of once per *cover*.
/// * `source_to_stored` — hash of an embedded picture's bytes → the same
///   result, for the tracks that reach [`extract_and_cache_artwork`]. The
///   tier above has a path to key on and this one doesn't, which is the only
///   reason the two are separate.
///
/// All three are read through [`memoized_path`], which owns the locking and the staleness rule.
pub struct CoverCaches {
    dir_to_cover: Mutex<LruCache<PathBuf, Option<PathBuf>>>,
    cover_to_cached: Mutex<LruCache<PathBuf, Option<String>>>,
    source_to_stored: Mutex<LruCache<String, Option<String>>>,
}

pub type CoverCache = Arc<CoverCaches>;

/// Build a fresh cover cache with the standard caps. Use this everywhere a
/// `CoverCache` is constructed so the caps stay consistent.
pub fn new_cover_cache() -> CoverCache {
    Arc::new(CoverCaches {
        dir_to_cover: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
        cover_to_cached: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
        source_to_stored: Mutex::new(LruCache::new(COVER_CACHE_CAP)),
    })
}

/// One tier's get-or-compute, with the lock dropped around `compute` — two Rayon workers racing on
/// the same key duplicate the work once rather than queueing behind each other's I/O.
///
/// **A hit naming a file that is gone counts as a miss.** The sweep can retire a stored cover
/// between the memo landing and a later track reaching it, and a row written from a stale hit does
/// not heal: `track_is_current` reads the track as current, so nothing re-extracts. One `stat`
/// against a decode plus a re-encode. A memoized `None` stays a hit — a directory with no cover and
/// a source that would not store are both settled answers, not worth re-asking per track.
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

/// Publishes a staged file under its final name, dropping it if that name is already taken.
///
/// The rename is what lets every writer trust its own `exists()` guard: the name is content-
/// addressed, so a file left half-written *under* it is never rewritten and that cover is gone
/// until someone deletes it by hand. `database/backup.rs` states the same rule for its staging.
fn persist_unless_exists(tmp: tempfile::NamedTempFile, file_path: &Path) -> io::Result<()> {
    if file_path.exists() {
        return Ok(());
    }
    tmp.persist(file_path).map(drop).map_err(|e| e.error)
}

/// Stages `bytes` beside their destination and renames into place.
fn write_atomic(dir: &Path, file_path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    persist_unless_exists(tmp, file_path)
}

/// Hex characters of BLAKE3 kept in a stored filename. Eight bytes, which puts the birthday bound
/// far under any realistic cover count and leaves the name short enough to read in a log line.
const HASH_HEX_LEN: usize = 16;

/// Extensions a stored file may carry.
///
/// Closed on purpose: [`is_stored_name`] can only be the exact inverse of the writers if the set
/// is. It is the union of what the cover pickers accept, what a lofty picture's MIME maps to, and
/// the JPEG the composites and artist images are encoded as.
///
/// **The `image` dependency's feature list has to cover it**, a format named here but absent there
/// being one the store accepts and no tier can draw. Held by a test rather than by review, the
/// build reporting nothing either way.
const STORED_EXTENSIONS: [&str; 7] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"];

/// `raw` folded onto [`STORED_EXTENSIONS`], falling back to JPEG for anything unrecognised.
///
/// Lowercased rather than taken verbatim, because a `COVER.JPG` and a `cover.jpg` holding the same
/// bytes hash alike and would otherwise land as two files. The fallback costs nothing — every
/// reader sniffs the content rather than the name.
fn stored_extension(raw: &str) -> &'static str {
    let lowered = raw.to_ascii_lowercase();
    STORED_EXTENSIONS.iter().copied().find(|ext| *ext == lowered).unwrap_or("jpg")
}

/// The one definition of the store's naming scheme; [`is_stored_name`] is its inverse.
fn stored_name(hash_hex: &str, ext: &str) -> String {
    format!("{hash_hex}.{ext}")
}

/// Whether `name` is one the writers here could have produced, and so the sweep's to retire.
///
/// Gated on this rather than on "everything in the directory", because the store is a folder in
/// the user's data directory and a name this module cannot have written is not ours to delete.
/// `crash_report::timestamp_of` and `database/backup.rs`'s `version_of` hold the same line.
///
/// The extension is matched case-insensitively where [`stored_extension`] folds it: builds before
/// that took the picked file's own, so `.JPG` names sit in stores already shipped and are exactly
/// as much ours to retire. It costs nothing, the lowercase-hex stem being the whole discriminator.
pub(super) fn is_stored_name(name: &str) -> bool {
    let Some((hash_hex, ext)) = name.rsplit_once('.') else {
        return false;
    };
    hash_hex.len() == HASH_HEX_LEN
        && hash_hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && STORED_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext))
}

/// Computes a truncated BLAKE3 hash ([`HASH_HEX_LEN`] hex chars) of the given data.
fn compute_hash(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hash.to_hex()[..HASH_HEX_LEN].to_string()
}

/// The first [`COVER_FILENAMES`] entry present in the audio file's own directory, memoized per
/// directory so an album's is probed once rather than once per track in it.
fn find_external_cover(file_path: &Path, cover_cache: &CoverCache) -> Option<PathBuf> {
    let dir = file_path.parent()?.to_path_buf();
    memoized_path(&cover_cache.dir_to_cover, dir, |dir| {
        COVER_FILENAMES.iter().map(|name| dir.join(name)).find(|path| path.exists())
    })
}

/// Longest edge a stored image may have.
///
/// The store must hold at least what the largest tier decodes — `ui::grid_prewarm`'s
/// `GRID_COVER_SIZE_HIDPI` at 448 — so that is a floor and this is the smallest round value
/// clearing it, leaving room to retune a tier without rebuilding the store. It doubles as the
/// ceiling on every transient decode buffer in the app, each tier decoding the stored file whole
/// before it resizes. A full-resolution artwork view would decode from the user's own file rather
/// than raise this: the store exists to serve many small frequently-drawn tiles, and at 4K it
/// would run to gigabytes across a few thousand covers.
pub(crate) const STORE_MAX_DIM: u32 = 512;

/// Byte ceiling, and a bound of its own rather than a consequence of [`STORE_MAX_DIM`] —
/// dimensions drive decode cost, bytes drive disk, and a file can fail either alone. Ordinary
/// encodings cannot reach this inside the dimension cap; 16-bit PNG and uncompressed TIFF can.
const STORE_MAX_BYTES: usize = 1024 * 1024;

/// Quality for everything this module encodes — the normalizer's re-encode and the composite
/// alike. One knob, because a composite is written straight into the store and would otherwise be
/// re-encoded the moment the two disagreed.
const STORE_JPEG_QUALITY: u8 = 90;

/// Resampler for the normalizer. Lanczos3 as the persisted composite takes, for the same reason:
/// this file outlives the session and is what every tier reads back.
const STORE_FILTER: FilterType = FilterType::Lanczos3;

/// Writes `bytes` into `dir` under their content hash, bounded, deduplicated, and atomically.
///
/// **The one funnel every byte-holding writer goes through**, so the store cannot end up with two
/// size policies. Returns the stored path, or `None` for something that isn't a decodable image
/// or that could not be written.
///
/// A source inside both bounds is stored byte-identical and never decoded at all — only its
/// header is read, which is also what rejects a container this build cannot draw.
pub(crate) fn store_image(bytes: &[u8], ext: &str, dir: &Path) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let (width, height) = image_decode::memory_dimensions(bytes, MAX_SOURCE_DIM)?;

    let over_bounds =
        width > STORE_MAX_DIM || height > STORE_MAX_DIM || bytes.len() > STORE_MAX_BYTES;
    let normalized = over_bounds.then(|| normalized_bytes(bytes, width, height)).flatten();

    // Hash what lands on disk, not what arrived: the filename has to describe the file, or the
    // `exists()` guard below starts answering about bytes nobody stored.
    let (stored, ext) = match normalized.as_deref() {
        Some(jpeg) => (jpeg, "jpg"),
        None => (bytes, ext),
    };
    let file_path = dir.join(stored_name(&compute_hash(stored), ext));
    if !file_path.exists()
        && let Err(e) = write_atomic(dir, &file_path, stored)
    {
        log::warn!("Failed to write artwork cache {}: {}", file_path.display(), e);
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}

/// `bytes` re-encoded inside the store's bounds, or `None` where that would not pay.
///
/// **Never returns something larger than the source**, which is what makes the cap forgiving: a
/// cheaply-encoded source is always at risk of growing under a fixed quality, so a cap set a
/// little too low can waste CPU here but can never make the store bigger.
///
/// A source over the *byte* bound alone keeps its dimensions — `fit_within` never enlarges and
/// has nothing to shrink, so it is re-encoded in place.
fn normalized_bytes(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let decoded = image_decode::decode_memory_capped(bytes, MAX_SOURCE_DIM)?;
    let (target_w, target_h) =
        image_decode::fit_within(width, height, STORE_MAX_DIM, STORE_MAX_DIM);
    let resized = resize_rgb8(&decoded, target_w, target_h, STORE_FILTER)?;

    let mut encoded = Vec::new();
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, STORE_JPEG_QUALITY);
    image::DynamicImage::ImageRgb8(resized).write_with_encoder(encoder).ok()?;
    (encoded.len() < bytes.len()).then_some(encoded)
}

/// Copies an external image file into the artwork cache, deduplicating by content hash.
pub(crate) fn cache_image_file(source_path: &Path, artwork_dir: &Path) -> Option<String> {
    let data = std::fs::read(source_path).ok()?;
    let ext = source_path.extension().and_then(|e| e.to_str()).map_or("jpg", stored_extension);
    store_image(&data, ext, artwork_dir)
}

/// Extracts the best cover picture from a lofty tag, hashes it with BLAKE3,
/// and saves it to the artwork cache directory. Returns the absolute path
/// to the cached image file, or None if no picture is found.
///
/// Memoized on the *source* bytes, because [`store_image`]'s `exists()` guard cannot help here:
/// the stored name has to describe the stored file, so an over-cap cover is decoded and re-encoded
/// before the guard is even reachable. Every track on an album carries the same picture, so
/// without the memo that whole cost is paid once per track and thrown away all but once.
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

    // Priority: CoverFront > CoverBack > first available
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
        // Unknown / explicit JPEG → fall back to "jpg" extension.
        _ => "jpg",
    };

    memoized_path(&cover_cache.source_to_stored, compute_hash(picture.data()), |_| {
        store_image(picture.data(), ext, artwork_dir)
    })
}

/// Unified artwork lookup: checks external cover files first, then embedded tag artwork.
/// Returns the cached artwork path, or None if no artwork is found from either source.
pub fn find_and_cache_artwork(
    file_path: &Path,
    tag: Option<&Tag>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
) -> Option<String> {
    // A memoized `None` — an unreadable or empty cover — falls through to embedded artwork the
    // same way an uncached one does.
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
) -> Option<image::RgbImage> {
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let (tw, th) = (f64::from(width), f64::from(height));

    // Scale so the image fully covers the target region
    let scale = (tw / iw).max(th / ih);
    let scaled_w = f64_to_pixel(iw * scale);
    let scaled_h = f64_to_pixel(ih * scale);

    let resized = resize_rgb8(img, scaled_w, scaled_h, filter)?;

    // Center-crop to target dimensions
    let x = (scaled_w.saturating_sub(width)) / 2;
    let y = (scaled_h.saturating_sub(height)) / 2;
    Some(image::imageops::crop_imm(&resized, x, y, width, height).to_image())
}

/// Side of the collage [`compose_artwork`] persists.
///
/// **Derived from [`STORE_MAX_DIM`] rather than spelled beside it**: the composite is written
/// *into* the store, so a larger canvas would be encoded once and immediately re-encoded by the
/// normalizer — a second generation loss on the one artwork path that runs per playlist edit. It
/// still clears both tiers that read a composite back, the playlist grid card and the detail hero.
/// [`compose_cover`] takes its side from the caller instead, the hero's largest consumer being a
/// tile it would only have to resize a second time.
pub(crate) const COMPOSITE_SIZE: u32 = STORE_MAX_DIM;

/// The composite is written into the store, so a canvas over the cap would be re-encoded the
/// moment it got there. Silent when it breaks — a doubled encode on every playlist edit — so it
/// is asserted where it cannot be skipped.
const _: () = assert!(COMPOSITE_SIZE <= STORE_MAX_DIM);

/// Resampler for the curated heroes' collage. Bilinear rather than the Lanczos3 the persisted one
/// takes: this canvas is downscaled again on the way to a cover tile and drawn at a fraction of
/// that, so the sharper filter's extra source taps land in pixels no surface ever resolves.
const HERO_FILTER: FilterType = FilterType::Bilinear;

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
    /// The source at this index would not decode, or would not resample into its tile.
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
        let tile = resize_to_cover(&source, width * half, height * half, filter)
            .ok_or(ComposeStop::Unreadable(index))?;
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
        let encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut hashing, STORE_JPEG_QUALITY);
        if let Err(e) = image::DynamicImage::ImageRgb8(canvas).write_with_encoder(encoder) {
            log::warn!("Failed to encode composite JPEG: {e}");
            return None;
        }
        if let Err(e) = hashing.flush() {
            log::warn!("Failed to flush composite JPEG: {e}");
            return None;
        }
        hashing.hasher.finalize().to_hex()[..HASH_HEX_LEN].to_string()
    };
    let filename = stored_name(&hash_hex, "jpg");
    let file_path = artwork_dir.join(&filename);

    if let Err(e) = persist_unless_exists(tmp, &file_path) {
        log::warn!("Failed to write composite artwork {}: {}", file_path.display(), e);
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}

/// Convert a non-negative f64 pixel coordinate (post `.round()` from image
/// resize math) into a `u32`. Saturates to `u32::MAX` if the value
/// somehow overflows; clamps NaN/negatives to 0.
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
