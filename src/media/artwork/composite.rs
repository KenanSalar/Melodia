//! Laying 1–4 covers out on one canvas.
//!
//! Two entry points over one layout table, and the difference between them is what an unreadable
//! source costs. [`compose_cover`] drops it and picks the layout from what survives — the curated
//! heroes recompose from whatever the database's top four currently are, so a cover deleted
//! underneath should cost its slot rather than the banner. [`compose_artwork`] refuses, because
//! the mosaic picker previewed that file slot for slot and the persisted thumbnail is a promise.
//! Collapsing the two onto one function persists a collage nobody chose.
//!
//! They differ on cost as well as on strictness: the persisted thumbnail composes at
//! [`COMPOSITE_SIZE`] through Lanczos3, where a hero takes its side from the caller through
//! [`HERO_FILTER`].

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::media::image_decode::{FilterType, MAX_SOURCE_DIM, decode_capped};

use super::{
    HASH_HEX_LEN, HashingWriter, STORE_JPEG_QUALITY, STORE_MAX_DIM, persist_unless_exists,
    resize_to_cover, stored_name,
};

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
