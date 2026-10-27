//! Laying 1–4 covers out on one canvas.
//!
//! Two entry points over one layout table, differing in what an unreadable source costs.
//! [`compose_cover`] drops it and lays out what survives: the curated heroes recompose from the
//! current top four, and a cover deleted underneath should cost its slot, not the banner.
//! [`compose_artwork`] refuses, because the mosaic picker previewed that set slot for slot and the
//! persisted thumbnail has to be the collage the user chose.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use melodia_core::error::describe;

use crate::media::image::image_decode::{FilterType, MAX_SOURCE_DIM, decode_capped};

use super::{
    HASH_HEX_LEN, HashingWriter, STORE_JPEG_QUALITY, STORE_MAX_DIM, persist_unless_exists,
    resize_to_cover, stage_in, stored_name,
};

/// Side of the collage [`compose_artwork`] persists. It is written into the store, so any larger
/// and the normalizer would re-encode it on arrival, a second generation loss per playlist edit.
pub const COMPOSITE_SIZE: u32 = STORE_MAX_DIM;

const _: () = assert!(COMPOSITE_SIZE <= STORE_MAX_DIM);

/// Bilinear rather than the persisted collage's Lanczos3: this canvas is downscaled again to a
/// cover tile, so the sharper filter's extra taps land in pixels no surface resolves.
const HERO_FILTER: FilterType = FilterType::Bilinear;

/// `(x, y, width, height)` per source in **half-canvas units**, so one table serves any side,
/// indexed by `len - 1`. Data rather than a `match` so the decode loop can walk it one source at a
/// time and never hold four full-size decodes.
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

/// Composes 1–4 sources into a `side` square, dropping any that won't decode.
///
/// `side` wants to be the size the collage is drawn at: the caller reduces it to a cover tile
/// next, so a larger canvas is resized twice and drawn once. **Blocking.**
pub fn compose_cover(source_paths: &[PathBuf], side: u32) -> Option<image::RgbImage> {
    let mut sources: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();

    loop {
        match compose_exact(&sources, side, HERO_FILTER) {
            Ok(canvas) => return Some(canvas),
            Err(ComposeStop::NoLayout) => return None,
            // Drop exactly what the pass stopped on. A readability pre-filter would decode every
            // path twice and still miss a source the cap refuses, whose header reads fine.
            Err(ComposeStop::Unreadable(index)) => {
                sources.remove(index);
            }
        }
    }
}

fn layout_for(len: usize) -> Option<&'static [(u32, u32, u32, u32)]> {
    COMPOSITE_LAYOUTS.get(len.checked_sub(1)?).copied()
}

/// Why [`compose_exact`] gave up, which the two callers answer differently.
enum ComposeStop {
    /// No sources or more than four, decided before anything is decoded.
    NoLayout,
    /// The source at this index would not decode, or would not resample into its tile.
    Unreadable(usize),
}

/// One all-or-nothing pass, holding a single decode at a time.
fn compose_exact(
    sources: &[&Path],
    side: u32,
    filter: FilterType,
) -> Result<image::RgbImage, ComposeStop> {
    let rects = layout_for(sources.len()).ok_or(ComposeStop::NoLayout)?;
    // An odd side leaves an unpainted hairline down two edges.
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

/// [`compose_exact`] persisted into `artwork_dir` under its content hash, strict where
/// [`compose_cover`] is lenient.
pub fn compose_artwork(source_paths: &[PathBuf], artwork_dir: &Path) -> Option<String> {
    let sources: Vec<&Path> = source_paths.iter().map(PathBuf::as_path).collect();
    let canvas = compose_exact(&sources, COMPOSITE_SIZE, FilterType::Lanczos3).ok()?;

    let tmp = match stage_in(artwork_dir) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("Failed to create composite tempfile: {}", describe(&e));
            return None;
        }
    };
    let hash_hex = {
        let mut hashing = HashingWriter::new(BufWriter::new(tmp.as_file()));
        let encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut hashing, STORE_JPEG_QUALITY);
        if let Err(e) = image::DynamicImage::ImageRgb8(canvas).write_with_encoder(encoder) {
            log::warn!("Failed to encode composite JPEG: {}", describe(&e));
            return None;
        }
        if let Err(e) = hashing.flush() {
            log::warn!("Failed to flush composite JPEG: {}", describe(&e));
            return None;
        }
        hashing.hasher.finalize().to_hex()[..HASH_HEX_LEN].to_string()
    };
    let filename = stored_name(&hash_hex, "jpg");
    let file_path = artwork_dir.join(&filename);

    if let Err(e) = persist_unless_exists(tmp, &file_path) {
        log::warn!("Failed to write composite artwork {}: {}", file_path.display(), describe(&e));
        return None;
    }
    Some(file_path.to_string_lossy().into_owned())
}
