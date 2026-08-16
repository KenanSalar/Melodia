//! Shared 2×2 cover-mosaic atlas + blur composition.
//!
//! Both mosaic heroes paint a blurred backdrop built from up to four cover paths: tile
//! the sources into a 2×2 atlas, downscale to `BLUR_TARGET`, `fast_blur` it — parity
//! with the single-source decode behind [`crate::ui::artwork_cache`] — then measure the
//! result for the colour solve. The per-view `hero.rs` files own the data source and the
//! application; this owns the CPU-bound work, of which the quantize is the heaviest.

use std::path::Path;

use image::imageops::fast_blur;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::media::image_decode::{MAX_SOURCE_DIM, decode_capped};
use crate::ui::backdrop::BackdropSample;
use crate::ui::util::{BLUR_SIGMA, BLUR_TARGET, buffer_from_rgb};

/// Side length of one tile in the 2×2 atlas.
const PER_TILE: u32 = BLUR_TARGET / 2;

/// A composed mosaic backdrop and what the colour solve needs to know about it. Both
/// halves come out of the same blocking call, so the scrim can't fall out of step with
/// the blur it is darkening.
pub(crate) struct MosaicBlur {
    pub(crate) blur: SharedPixelBuffer<Rgb8Pixel>,
    pub(crate) sample: BackdropSample,
}

/// Compose up to four sources into a square 2×2 atlas, blur it, and measure the result.
/// A partially-populated mosaic fills the whole atlas with what it has, so it still
/// produces a usable backdrop. Blocking pool.
///
/// `None` when every decode failed — the caller clears `has-blur` so the gradient floor
/// shows through.
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

    // The layouts mirror `CoverMosaic`'s. The atlas is then heavily blurred, so the
    // exact arrangement is imperceptible — but branching on the tile count keeps the
    // colour distribution consistent with the visible mosaic above.
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

/// Stretch-copy `src` into `dst` at a destination rectangle. Sampling is
/// nearest-neighbour, the blur pass immediately after obliterating any aliasing.
///
/// The per-pixel `get_pixel` / `put_pixel` is deliberate: the rectangle is at most
/// `BLUR_TARGET²` pixels and runs once per artwork change, so hand-rolled row offsets
/// would buy a fraction of a cold path and cost the reader the coordinates. What a
/// recompose actually spends its time on is the `fast_blur` below and the quantize
/// behind it.
fn blit(dst: &mut image::RgbImage, src: &image::RgbImage, dx: u32, dy: u32, dw: u32, dh: u32) {
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 {
        return;
    }
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let px = *src.get_pixel(sx, sy);
            dst.put_pixel(dx + x, dy + y, px);
        }
    }
}

#[cfg(test)]
#[path = "tests/mosaic_blur_tests.rs"]
mod tests;
