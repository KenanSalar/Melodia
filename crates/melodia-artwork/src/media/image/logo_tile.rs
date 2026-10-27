//! A fetched station logo, composed into the square opaque tile a card draws and filed in the
//! radio-logo store.
//!
//! A directory's logo field is a favicon field, and two of the shapes it serves don't survive a
//! card. A wide wordmark is stretched, the tiers resizing aspect-blind into a square. A transparent
//! mark loses its alpha, every tier being RGB8, and lands on whatever bytes sat beneath it. Both
//! are fixed once, at fetch, so the store holds ordinary square artwork and no tier needs a mode.

use std::path::Path;

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use melodia_core::entities::radio::StoredLogo;

use crate::media::image::artwork::{self, STORE_MAX_DIM};
use crate::media::image::image_decode::{self, FilterType, fit_within, resize_rgb8};

/// Shortest edge worth storing. Favicon-sized rather than what the surfaces would like: the field
/// *is* a favicon field, and holding out for more mostly bought the monogram. What it still keeps
/// out is a tracker's 1×1 pixel, gated once at the writer so no tier has to reject one.
const MIN_LOGO_DIM: u32 = 32;

/// Alpha at or below which a pixel is ground rather than faint: an exporter's clear pixels aren't
/// always exactly zero, and a mark's soft edge must not read as ground. Decides the ground only;
/// whether to composite at all is [`is_opaque`]'s, and that one is exact.
const TRANSPARENT_ALPHA: u8 = 8;

/// How much of the border ring must be opaque for it to name the source's own ground. Short of 1.0
/// so a rounded-corner icon still answers with its fill.
const OPAQUE_RING_FRACTION: f64 = 0.75;

/// Mark luma past which the dark ground wins: where WCAG's 3:1 non-text contrast against white
/// runs out, Rec. 709 luma standing in for luminance. Deliberately high, a mark drawn for
/// transparency having been drawn for a web page and so usually a light one needing rescue.
const WHITE_GROUND_MAX_LUMA: f64 = 149.0;

/// Neutral rather than taken from the mark, which would argue with every colour in a logo.
const GROUND_LIGHT: Rgb<u8> = Rgb([0xff, 0xff, 0xff]);
const GROUND_DARK: Rgb<u8> = Rgb([0x1a, 0x1a, 0x1a]);

/// `bytes` filed in the store under `dir`, composed where they need it, or `None` where nothing
/// drawable arrived. The floor is read off the header, so a source too small costs no decode.
/// **Blocking.**
pub fn store(bytes: &[u8], extension: &str, dir: &Path) -> Option<StoredLogo> {
    let (width, height) = image_decode::memory_dimensions(bytes, image_decode::MAX_SOURCE_DIM)?;
    if width < MIN_LOGO_DIM || height < MIN_LOGO_DIM {
        return None;
    }
    // Held to the end: `compose` flattens at full resolution, and the store decodes again to bound
    // an oversized source. The download's byte cap bounds only the compressed size, which a flat
    // icon stays under at any dimension.
    let _oversized = image_decode::large_decode_guard(u64::from(width) * u64::from(height));
    // `None` stores the source's own bytes: already fine, or its tile wouldn't encode.
    let tile = match tile_for(bytes) {
        Tile::Composed(tile) => encoded_png(tile),
        Tile::SourceIsFine => None,
        Tile::Undrawable => return None,
    };

    let path = match tile.as_deref() {
        Some(tile) => artwork::store_image(tile, "png", dir),
        None => artwork::store_image(bytes, extension, dir),
    }?;
    // A failed stat says nothing about whether the file is drawable, so report what arrived.
    let stored = std::fs::metadata(&path).map_or(bytes.len() as u64, |meta| meta.len());
    Some(StoredLogo { path, bytes: stored })
}

/// A source the decoder refuses is [`Tile::SourceIsFine`]: the header read already showed the
/// store can hold it, and without a decode there's no evidence it wanted a tile.
fn tile_for(bytes: &[u8]) -> Tile {
    match image_decode::decode_memory_capped(bytes, image_decode::MAX_SOURCE_DIM) {
        Some(decoded) => compose(decoded),
        None => Tile::SourceIsFine,
    }
}

/// PNG rather than the store's JPEG: a tile is flat ground behind a hard-edged mark, which JPEG
/// rings around and PNG holds in a few kilobytes.
fn encoded_png(tile: RgbImage) -> Option<Vec<u8>> {
    let mut encoded = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut encoded);
    DynamicImage::ImageRgb8(tile).write_with_encoder(encoder).ok()?;
    Some(encoded)
}

/// What [`compose`] decided. "Already fine" and "nothing worth storing" both come back without a
/// tile and mean opposite things to the caller, hence three answers.
enum Tile {
    Composed(RgbImage),
    /// Already square and opaque. Storing its own bytes keeps `store_image`'s byte-identical path.
    SourceIsFine,
    /// Needs a tile and has no opaque pixel to take a ground from. Refused, the untreated source
    /// painting the empty square this module exists to prevent, so the card draws its monogram.
    Undrawable,
}

/// `decoded` as a square opaque tile, or why there isn't one. By value so `into_rgba8` is free on a
/// source that already is one, where `to_rgba8` copies.
fn compose(decoded: DynamicImage) -> Tile {
    let (width, height) = (decoded.width(), decoded.height());
    let square = width == height;
    if square && !decoded.color().has_alpha() {
        return Tile::SourceIsFine;
    }

    let source = decoded.into_rgba8();
    // Plenty of sources carry an alpha channel they never use. Exact rather than
    // `TRANSPARENT_ALPHA`: the tiers drop the channel, so anything short of opaque is flattened.
    if square && source.pixels().copied().all(is_opaque) {
        return Tile::SourceIsFine;
    }

    let Some(ground) = ground_colour(&source) else {
        return Tile::Undrawable;
    };
    // Before the downscale: resampling straight alpha against undefined colour is where the halo
    // around a mark's edge comes from.
    let Some(flattened) = flatten(&source, ground).map(DynamicImage::ImageRgb8) else {
        return Tile::Undrawable;
    };

    // The store caps anything larger on the way in.
    let side = width.max(height).min(STORE_MAX_DIM);
    let (mark_w, mark_h) = fit_within(width, height, side, side);
    let Some(mark) = resize_rgb8(&flattened, mark_w, mark_h, FilterType::Lanczos3) else {
        return Tile::Undrawable;
    };

    let mut tile = RgbImage::from_pixel(side, side, ground);
    image::imageops::replace(
        &mut tile,
        &mark,
        i64::from((side - mark_w) / 2),
        i64::from((side - mark_h) / 2),
    );
    Tile::Composed(tile)
}

fn is_transparent(pixel: Rgba<u8>) -> bool {
    pixel.0[3] <= TRANSPARENT_ALPHA
}

fn is_opaque(pixel: Rgba<u8>) -> bool {
    pixel.0[3] == u8::MAX
}

/// The ring's own colour where it has one, so a branded rectangle stays the tile it was, otherwise
/// a neutral picked against the mark. `None` with no opaque pixel.
fn ground_colour(source: &RgbaImage) -> Option<Rgb<u8>> {
    if let Some(ring) = modal_opaque_ring(source) {
        return Some(ring);
    }
    let luma = mark_luma(source)?;
    Some(if luma > WHITE_GROUND_MAX_LUMA { GROUND_DARK } else { GROUND_LIGHT })
}

/// The most common opaque colour on the source's border, or `None` when too little of it is opaque
/// to be a ground. Ties break on the colour: `HashMap` order varies per process, and an unstable
/// pick would store a new content-addressed file on every fetch of the same logo.
fn modal_opaque_ring(source: &RgbaImage) -> Option<Rgb<u8>> {
    let (width, height) = source.dimensions();
    let mut tally: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    let mut ring = 0u32;
    let mut opaque = 0u32;

    let mut tally_pixel = |pixel: &Rgba<u8>| {
        ring += 1;
        if is_transparent(*pixel) {
            return;
        }
        opaque += 1;
        *tally.entry([pixel.0[0], pixel.0[1], pixel.0[2]]).or_default() += 1;
    };

    for x in 0..width {
        tally_pixel(source.get_pixel(x, 0));
        if height > 1 {
            tally_pixel(source.get_pixel(x, height - 1));
        }
    }
    for y in 1..height.saturating_sub(1) {
        tally_pixel(source.get_pixel(0, y));
        if width > 1 {
            tally_pixel(source.get_pixel(width - 1, y));
        }
    }

    if ring == 0 || f64::from(opaque) < f64::from(ring) * OPAQUE_RING_FRACTION {
        return None;
    }
    tally.into_iter().max_by_key(|(rgb, count)| (*count, *rgb)).map(|(rgb, _)| Rgb(rgb))
}

/// Rec. 709 luma of the mark alone. Averaging the clear field in as black is how a light mark ends
/// up asking for a light ground.
fn mark_luma(source: &RgbaImage) -> Option<f64> {
    let mut sum = 0f64;
    let mut counted = 0u32;
    for pixel in source.pixels() {
        if is_transparent(*pixel) {
            continue;
        }
        let [r, g, b, _] = pixel.0;
        sum +=
            0.0722f64.mul_add(f64::from(b), 0.2126f64.mul_add(f64::from(r), 0.7152 * f64::from(g)));
        counted += 1;
    }
    (counted > 0).then(|| sum / f64::from(counted))
}

/// `source` composited over a flat `ground`, in gamma space as the renderer would have done it.
fn flatten(source: &RgbaImage, ground: Rgb<u8>) -> Option<RgbImage> {
    let mut out = Vec::with_capacity(source.len() / 4 * 3);
    for pixel in source.chunks_exact(4) {
        for (channel, under) in ground.0.into_iter().enumerate() {
            out.push(over(pixel[channel], pixel[3], under));
        }
    }
    RgbImage::from_raw(source.width(), source.height(), out)
}

/// One channel of `top` at `alpha` over `under`, rounded to the nearest byte.
fn over(top: u8, alpha: u8, under: u8) -> u8 {
    let alpha = u32::from(alpha);
    let blended = u32::from(top) * alpha + u32::from(under) * (255 - alpha);
    u8::try_from((blended + 127) / 255).unwrap_or(u8::MAX)
}

#[cfg(test)]
#[path = "tests/logo_tile_tests.rs"]
mod tests;
