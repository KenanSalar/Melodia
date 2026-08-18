use super::dither_tile;

/// Uniform by construction, because the tile is composited at one 8-bit level: whether a pixel
/// rounds up is decided by its own value against a fixed threshold, so a bunched histogram would
/// dither some parts of the surface and not others.
#[test]
fn the_dither_tile_uses_every_level_evenly() {
    let tile = dither_tile();
    let mut histogram = [0u32; 256];
    for pixel in tile.as_slice() {
        histogram[usize::from(pixel.r)] += 1;
    }

    let (Some(&thinnest), Some(&densest)) = (histogram.iter().min(), histogram.iter().max()) else {
        unreachable!("a 256-entry histogram is never empty")
    };
    assert!(thinnest > 0, "some levels never occur, so the tile dithers unevenly");
    assert!(densest - thinnest <= 1, "histogram spans {thinnest}..={densest}, wanted flat");
}

/// Blue, not white. At one level of amplitude the tile is nearly a one-bit pattern and how it
/// spaces itself is all there is to see: white noise clumps into blotches at the low frequencies
/// the eye is most sensitive to, where blue noise spaces evenly and disappears. Measured as mean
/// neighbour contrast, which white noise leaves at ~0.33.
#[test]
fn the_dither_tile_is_shaped_toward_blue() {
    let tile = dither_tile();
    let side = usize::try_from(tile.width()).unwrap_or(0);
    let levels = tile.as_slice();

    let mut total = 0.0_f64;
    let mut pairs = 0.0_f64;
    for row in 0..side {
        for col in 0..side {
            let here = f64::from(levels[row * side + col].r);
            // Wrapping, so the measurement also covers the seam the tile repeats across.
            for (down, right) in [(0, 1), (1, 0), (1, 1)] {
                let neighbour = levels[((row + down) % side) * side + (col + right) % side].r;
                total += (here - f64::from(neighbour)).abs();
                pairs += 1.0;
            }
        }
    }

    let contrast = total / pairs / 255.0;
    assert!(contrast > 0.36, "neighbour contrast {contrast:.3} is white-noise flat");
}

/// The whole point is that it is imperceptible: composited at one 255th, the tile moves what is
/// under it by a single quantization step. Six read as a film of dust over the surface.
#[test]
fn the_dither_tile_is_laid_on_at_one_level() {
    let tile = dither_tile();
    assert!(
        tile.as_slice().iter().all(|pixel| pixel.a == 1),
        "the dither's alpha left 1/255, which is the difference between grain and dust"
    );
}
