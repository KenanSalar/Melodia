use material_colors::color::Argb;
use material_colors::hct::Hct;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::services::material_you::{extract_seeds_from_rgb8, extract_source_argb_from_rgb8};
use crate::ui::aurora::{TINT_TONE, dither_tile, tints};
use crate::ui::backdrop::SEED_COUNT;

/// The default accent, standing in for "no artwork" wherever a fallback is exercised.
const THEME_ACCENT: u32 = 0x00cb_a6f7;

/// A cover whose quantize separated three hues.
const THREE_HUES: [Option<u32>; SEED_COUNT] =
    [Some(0x00c0_3030), Some(0x0030_c030), Some(0x0030_30c0)];

/// A monochrome sleeve: one seed, two tints owed to the filling rule.
const ONE_HUE: [Option<u32>; SEED_COUNT] = [Some(0x00c0_3030), None, None];

fn hct_of(rgb: u32) -> Hct {
    Hct::new(Argb::from_u32(rgb))
}

/// Smallest signed distance between two hue angles, so a rotation across 0° is measured as the
/// step it is rather than as ~360.
fn hue_gap(a: f64, b: f64) -> f64 {
    let raw = (a - b).abs() % 360.0;
    raw.min(360.0 - raw)
}

/// A buffer of `colours`, tiled one per row — enough distinct pixels for `QuantizerCelebi` to
/// find each as a cluster.
fn buffer_of(colours: &[[u8; 3]]) -> SharedPixelBuffer<Rgb8Pixel> {
    let side = 32u32;
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(side, side);
    let px = buf.make_mut_slice();
    for (i, slot) in px.iter_mut().enumerate() {
        let [r, g, b] = colours[(i / side as usize) % colours.len()];
        *slot = Rgb8Pixel { r, g, b };
    }
    buf
}

// --- the seed list ------------------------------------------------------------

/// The invariant the whole widening rests on: `Score` clears its shortlist on every hue-separation
/// pass and pushes the top-scored survivor unconditionally, so entry 0 can't depend on how many
/// were asked for. Without this, moving the backdrop to three seeds would silently re-colour every
/// tier solved off `accent_argb`.
#[test]
fn the_first_seed_is_the_same_however_many_are_asked_for() {
    let buf = buffer_of(&[[200, 40, 40], [40, 200, 40], [40, 40, 200], [200, 200, 40]]);

    let one = extract_source_argb_from_rgb8(&buf);
    let many = extract_seeds_from_rgb8(&buf, SEED_COUNT).into_iter().next();

    assert_eq!(one, many);
    assert!(one.is_some(), "a four-hue buffer must quantize to something");
}

/// A greyscale sleeve is filtered out by `Score`'s chroma cutoff, and the crate's own answer there
/// is Google Blue — a brand colour, not a fact about this record. `seed_from_pixels` hands the real
/// dominant over as the fallback, and the ranked call has to keep doing so.
#[test]
fn a_monochrome_sleeve_seeds_from_its_own_grey() {
    let buf = buffer_of(&[[90, 90, 90], [120, 120, 120]]);

    let seeds = extract_seeds_from_rgb8(&buf, SEED_COUNT);

    let chroma = seeds.first().map(|&first| hct_of(first).get_chroma());
    assert!(chroma.is_some(), "the dominant fallback means a grey buffer still answers");
    assert!(
        chroma.is_some_and(|c| c < 5.0),
        "a grey buffer must not answer with a hue, got {seeds:#08x?}"
    );
}

// --- the washes ---------------------------------------------------------------

/// Every tint sits at one tone, so the only axis they differ on is hue. A tint brighter than its
/// neighbours turns its wash into a lightness ramp, which is the one thing a blurred cover never
/// produced.
#[test]
fn every_tint_lands_on_one_tone() {
    for seeds in [THREE_HUES, ONE_HUE, [None; SEED_COUNT]] {
        for tint in tints(seeds, THEME_ACCENT) {
            let tone = hct_of(tint.rgb).get_tone();
            assert!(
                (tone - TINT_TONE).abs() < 1.0,
                "tint {:#08x} at tone {tone}, wanted {TINT_TONE}",
                tint.rgb
            );
        }
    }
}

/// `Score` ranks by usability rather than by saturation, so an ordinary cover's second and third
/// seeds are a near-white and a near-black carrying almost no chroma. Taken as they came they
/// dilute the dominant and the surface converges on grey — the floor is what stops that, and it
/// only works because the tone is set *before* the chroma is asked for.
#[test]
fn a_washed_out_seed_is_lifted_to_carry_colour() {
    // Tone 96 and tone 3: the shapes `Score` actually returns beside a vivid dominant.
    let washed_out = [Some(0x00c0_3030), Some(0x00f2_efee), Some(0x0005_0408)];

    for tint in tints(washed_out, THEME_ACCENT) {
        let chroma = hct_of(tint.rgb).get_chroma();
        assert!(chroma > 20.0, "tint {:#08x} came back grey at chroma {chroma}", tint.rgb);
    }
}

/// The rule that keeps a monochrome record looking monochrome: a rotated hue is a guess, and on a
/// low-chroma sleeve it survives gamut mapping as almost nothing, so washing it on at full
/// strength would stack three near-identical ramps into a lightness gradient the record never had.
#[test]
fn a_synthesized_tint_is_washed_on_more_faintly_than_a_real_one() {
    let real = tints(THREE_HUES, THEME_ACCENT);
    assert!(real.iter().all(|t| t.weight >= 1.0), "a separated cover washes every tint in full");

    let [first, second, third] = tints(ONE_HUE, THEME_ACCENT);
    assert!(first.weight >= 1.0, "the seed the artwork gave is not a guess");
    assert!(second.weight < first.weight, "the fills must not match the seed's weight");
    assert!(third.weight < first.weight, "the fills must not match the seed's weight");
}

/// `Score` returns fewer than asked rather than reaching for near-duplicates, and never pads. A
/// duotone cover therefore arrives two seeds short of a tint and the filling rule owes the rest.
#[test]
fn a_short_list_is_filled_from_its_own_hue_and_not_the_theme() {
    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    let seed_hue = hct_of(0x00c0_3030).get_hue();

    for tint in tints(ONE_HUE, THEME_ACCENT) {
        let gap_from_seed = hue_gap(hct_of(tint.rgb).get_hue(), seed_hue);
        let gap_from_accent = hue_gap(hct_of(tint.rgb).get_hue(), accent_hue);
        assert!(
            gap_from_seed <= 30.0,
            "fill {:#08x} drifted {gap_from_seed}° from the album; accent is {gap_from_accent}° away",
            tint.rgb
        );
    }
}

/// Two fills rotating the same way would stack into a near-duplicate pair; either side of the
/// source is what keeps three washes reading as three.
#[test]
fn the_two_fills_land_either_side_of_the_seed() {
    let [first, second, third] = tints(ONE_HUE, THEME_ACCENT);

    assert!(
        hue_gap(hct_of(first.rgb).get_hue(), hct_of(0x00c0_3030).get_hue()) < 5.0,
        "tint 0 is the seed's own hue, got {:#08x}",
        first.rgb
    );
    assert!(
        hue_gap(hct_of(second.rgb).get_hue(), hct_of(third.rgb).get_hue()) > 30.0,
        "the two fills collapsed together: {:#08x} / {:#08x}",
        second.rgb,
        third.rgb
    );
}

/// `Score` optimises for hue *separation* — it starts at 90° and walks down only when the artwork
/// can't supply it — which is the opposite of what a backdrop wants: overlapping washes composite
/// in sRGB, whose midpoint between distant hues is grey, so raw seeds make the more colourful
/// cover the muddier one. The dominant keeps its own hue; the others are pulled to it.
#[test]
fn widely_separated_seeds_are_pulled_into_one_arc() {
    // A quadrant apart, which `Score` will happily return from a colourful sleeve.
    let scattered = [Some(0x00c0_3030), Some(0x0030_c030), Some(0x0030_30c0)];
    let [first, second, third] = tints(scattered, THEME_ACCENT);

    let anchor = hct_of(first.rgb).get_hue();
    assert!(
        hue_gap(anchor, hct_of(0x00c0_3030).get_hue()) < 5.0,
        "the dominant must keep its own hue, got {:#08x}",
        first.rgb
    );
    for pulled in [second, third] {
        let gap = hue_gap(hct_of(pulled.rgb).get_hue(), anchor);
        assert!(gap <= 45.0, "tint {:#08x} sits {gap}° off the dominant", pulled.rgb);
    }
}

/// No artwork at all is the only path that may reach for the theme, and it still owes three
/// distinguishable tints rather than one colour washed on three times.
#[test]
fn no_seeds_at_all_falls_back_to_the_theme_accent() {
    let [first, second, third] = tints([None; SEED_COUNT], THEME_ACCENT);

    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    assert!(
        hue_gap(hct_of(first.rgb).get_hue(), accent_hue) < 5.0,
        "tint 0 must be the accent's own hue, got {:#08x}",
        first.rgb
    );
    assert!(second.rgb != first.rgb && third.rgb != first.rgb, "the fills duplicated the accent");
}

// --- the dither tile -----------------------------------------------------------

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
