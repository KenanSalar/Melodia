//! Shared 2×2 cover-mosaic atlas + blur composition.
//!
//! Both hero surfaces (Favorites, Recently Played) paint a blurred backdrop
//! built from up to 4 cover paths: tile the sources into a 2×2 atlas,
//! downscale to the now-playing-tier `BLUR_TARGET`, and run
//! `image::imageops::fast_blur` (parity with
//! `now_playing_artwork::decode_artwork`), then measure the result for the
//! hero's colour solve. The per-view `hero.rs` files own the data source and
//! the `write_crossfade_slot` application; this module owns the CPU-bound
//! image work, and the quantize behind that measurement is the heaviest of it.

use std::path::Path;

use image::imageops::fast_blur;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};
use crate::ui::backdrop::BackdropSample;
use crate::ui::util::{BLUR_SIGMA, BLUR_TARGET, buffer_from_rgb};

/// Side length of one tile in the 2×2 atlas.
const PER_TILE: u32 = BLUR_TARGET / 2;

/// A composed mosaic backdrop and what the hero's colour solve needs to know
/// about it. Both halves are produced by the same blocking call, so the scrim
/// can't fall out of step with the blur it is darkening.
pub(crate) struct MosaicBlur {
    pub(crate) blur: SharedPixelBuffer<Rgb8Pixel>,
    pub(crate) sample: BackdropSample,
}

/// Compose up to 4 source images into a `BLUR_TARGET × BLUR_TARGET`
/// 2×2 atlas, blur it, and measure the result. Mirrors the picker's mosaic
/// layout for the 4-tile case; the 1 / 2 / 3 / 0 cases fall back to "fill the
/// whole atlas with the available tiles" so a partially-populated mosaic still
/// produces a usable hero backdrop. Runs on the blocking pool.
///
/// Returns `None` when every source decode failed — the caller clears
/// `has-blur` so the gradient floor shows through.
pub(crate) fn compose_mosaic_blur(paths: &[String]) -> Option<MosaicBlur> {
    use image::{ImageBuffer, RgbImage};

    if paths.is_empty() {
        return None;
    }

    let mut atlas: RgbImage = ImageBuffer::new(BLUR_TARGET, BLUR_TARGET);

    // Decode each source (capped at 4) into a per-tile thumbnail.
    let mut tiles: Vec<RgbImage> = Vec::with_capacity(4);
    for p in paths.iter().take(4) {
        if let Some(tile) = decode_tile(Path::new(p)) {
            tiles.push(tile);
        }
    }
    if tiles.is_empty() {
        return None;
    }

    // Lay tiles into the atlas. Layouts mirror the CoverMosaic
    // component for visual parity with the foreground mosaic:
    //   1 tile  → fill whole atlas
    //   2 tiles → left / right halves
    //   3 tiles → left full-height + right column split top/bottom
    //   4 tiles → 2×2 grid
    // The atlas is then heavily blurred so the exact layout is
    // imperceptible — but the tile-count branching keeps colour
    // distribution consistent with the visible mosaic above.
    match tiles.len() {
        1 => {
            blit(&mut atlas, &tiles[0], 0, 0, BLUR_TARGET, BLUR_TARGET);
        }
        2 => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, BLUR_TARGET);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, BLUR_TARGET);
        }
        3 => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, BLUR_TARGET);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[2], PER_TILE, PER_TILE, PER_TILE, PER_TILE);
        }
        _ => {
            blit(&mut atlas, &tiles[0], 0, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[1], PER_TILE, 0, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[2], 0, PER_TILE, PER_TILE, PER_TILE);
            blit(&mut atlas, &tiles[3], PER_TILE, PER_TILE, PER_TILE, PER_TILE);
        }
    }

    let blur = buffer_from_rgb(&fast_blur(&atlas, BLUR_SIGMA));
    let sample = BackdropSample::measure(&blur);
    Some(MosaicBlur { blur, sample })
}

/// Decode one cover at its tile size. Bounded so a forged header can't
/// allocate gigabytes before the downscale kicks in.
fn decode_tile(path: &Path) -> Option<image::RgbImage> {
    let decoded = decode_capped(path, MAX_SOURCE_DIM).ok()?;
    Some(decoded.thumbnail_exact(BLUR_TARGET, BLUR_TARGET).to_rgb8())
}

/// Stretch-copy `src` into `dst` at the given destination rectangle.
/// `src` is already a square `BLUR_TARGET × BLUR_TARGET` thumbnail
/// (`decode_tile`'s output), so the per-tile blit is a sub-block of
/// the larger atlas; sampling is nearest-neighbour because the blur
/// pass that immediately follows obliterates any aliasing.
fn blit(dst: &mut image::RgbImage, src: &image::RgbImage, dx: u32, dy: u32, dw: u32, dh: u32) {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return;
    }
    for y in 0..dh {
        for x in 0..dw {
            let sx = x * sw / dw;
            let sy = y * sh / dh;
            let px = *src.get_pixel(sx, sy);
            dst.put_pixel(dx + x, dy + y, px);
        }
    }
}

#[cfg(test)]
#[path = "tests/mosaic_blur_tests.rs"]
mod tests;
