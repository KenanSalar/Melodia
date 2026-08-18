use material_colors::color::Argb;
use material_colors::hct::Hct;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use super::{WASH_MAX_TONE, WASH_SHADE_FACTOR};
use crate::services::material_you::{extract_source_argb_from_rgb8, population_seeds};
use crate::ui::aurora::{WASH_COUNT, dither_tile, tints};
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens};

/// The default accent, standing in for "no artwork" wherever a fallback is exercised.
const THEME_ACCENT: u32 = 0x00cb_a6f7;

/// Catppuccin Mocha — the shipped default. What these tests are about is what the washes keep,
/// which is everything: the theme reaches them only as the fallback for a set with no seeds at all.
fn mocha() -> ThemeTokens {
    ThemeTokens {
        base: 0x001e_1e2e,
        text: 0x00cd_d6f4,
        accent: THEME_ACCENT,
    }
}

/// A cover whose quantize separated a full set of hues — the real seeds off `Real for Me`.
const MANY_HUES: [Option<u32>; SEED_COUNT] = [
    Some(0x0038_718b),
    Some(0x00cc_2841),
    Some(0x0030_3446),
    Some(0x007d_5c79),
];

/// A monochrome sleeve: one seed, the rest owed to the filling rule.
const ONE_HUE: [Option<u32>; SEED_COUNT] = [Some(0x00c0_3030), None, None, None];

/// Side of every fixture buffer. Spelled so a test can hand [`buffer_of`] exactly this many
/// entries and place rows verbatim rather than cycling.
const FIXTURE_SIDE: u32 = 32;

fn hct_of(rgb: u32) -> Hct {
    Hct::new(Argb::from_u32(rgb))
}

/// Smallest signed distance between two hue angles, so a rotation across 0° is measured as the
/// step it is rather than as ~360.
fn hue_gap(a: f64, b: f64) -> f64 {
    let raw = (a - b).abs() % 360.0;
    raw.min(360.0 - raw)
}

/// A buffer of `colours`, cycled one per row. At [`FIXTURE_SIDE`] entries the cycle is the identity
/// and the caller is stating each row, which is how the weighted fixtures below get their
/// proportions.
fn buffer_of(colours: &[[u8; 3]]) -> SharedPixelBuffer<Rgb8Pixel> {
    let side = FIXTURE_SIDE as usize;
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(FIXTURE_SIDE, FIXTURE_SIDE);
    let px = buf.make_mut_slice();
    for (i, slot) in px.iter_mut().enumerate() {
        let [r, g, b] = colours[(i / side) % colours.len()];
        *slot = Rgb8Pixel { r, g, b };
    }
    buf
}

/// A neutral ramp, one grey per row — the case that used to yield a single seed.
fn greyscale_ramp() -> SharedPixelBuffer<Rgb8Pixel> {
    let rows: Vec<[u8; 3]> = (0..FIXTURE_SIDE)
        .map(|row| {
            let level = 24 + u8::try_from(row).unwrap_or(0) * 7;
            [level, level, level]
        })
        .collect();
    buffer_of(&rows)
}

// --- the seed list ------------------------------------------------------------

/// The finding the extractor was swapped for. `Score` discards every cluster under its own chroma
/// cutoff, so a neutral sleeve survives it once — leaving three of four washes invented by hue
/// rotation off the fourth, at a third of its strength. Median cut has no such cutoff and answers
/// with the greys the record is made of.
#[test]
fn a_greyscale_ramp_yields_a_full_set_of_seeds() {
    let seeds = population_seeds(&greyscale_ramp(), SEED_COUNT);

    assert_eq!(seeds.len(), SEED_COUNT, "got {seeds:#08x?}");
    for (index, seed) in seeds.iter().enumerate() {
        for other in seeds.iter().skip(index + 1) {
            assert_ne!(seed, other, "two seeds off a ramp came back identical");
        }
    }
}

/// A backdrop wants what the cover mostly *is*, and that is a different question from what makes
/// the best accent. `Score` picks the vivid stripe here — defensible for a seed, wrong for a
/// surface meant to feel like the record.
#[test]
fn the_first_seed_is_what_the_cover_mostly_is() {
    const SLATE: [u8; 3] = [48, 52, 70];
    const VIVID: [u8; 3] = [204, 40, 65];

    let mut rows = vec![SLATE; FIXTURE_SIDE as usize - 1];
    rows.push(VIVID);

    let seeds = population_seeds(&buffer_of(&rows), SEED_COUNT);

    let Some(&first) = seeds.first() else {
        unreachable!("a two-colour buffer must quantize to something")
    };
    // Loosely: median cut answers with a box average off a 5-bit histogram, not with the input.
    let channels = [(first >> 16) & 0xff, (first >> 8) & 0xff, first & 0xff];
    for (got, want) in channels.iter().zip(SLATE) {
        let gap = got.abs_diff(u32::from(want));
        assert!(gap <= 8, "seed {first:#08x} is not the bulk colour ({gap} off on a channel)");
    }
    assert!(
        seeds.iter().any(|&seed| (seed >> 16) & 0xff > 0xc0),
        "the vivid stripe went missing entirely, got {seeds:#08x?}"
    );
}

/// The two shapes that aren't a cover, and the crate answers them the same way unless one is
/// caught: a near-white sleeve is filtered down to nothing, and so is an empty buffer, but only the
/// second means "there is no artwork here".
#[test]
fn an_empty_buffer_and_a_white_one_are_told_apart() {
    let empty = SharedPixelBuffer::<Rgb8Pixel>::new(0, 0);
    assert!(population_seeds(&empty, SEED_COUNT).is_empty());

    let white = population_seeds(&buffer_of(&[[255, 255, 255]]), SEED_COUNT);
    assert_eq!(white, vec![0x00ff_ffff], "a white cover answers with its own white");
}

/// Material You's seed keeps `Score`, so it keeps needing the dominant handed over as the
/// fallback — the crate's own answer on a colourless sleeve is Google Blue, a brand colour rather
/// than a fact about this record.
#[test]
fn a_monochrome_sleeve_seeds_from_its_own_grey() {
    let seed = extract_source_argb_from_rgb8(&buffer_of(&[[90, 90, 90], [120, 120, 120]]));

    let chroma = seed.map(|first| hct_of(first).get_chroma());
    assert!(chroma.is_some(), "the dominant fallback means a grey buffer still answers");
    assert!(
        chroma.is_some_and(|c| c < 5.0),
        "a grey buffer must not answer with a hue, got {seed:#08x?}"
    );
}

// --- the washes ---------------------------------------------------------------

/// A measured wash is the quantizer's colour, bit for bit, in the quantizer's own slot.
///
/// **The pin the whole arm rests on.** Every pass that ever sat between the two — a tone band, a
/// hue seating — took the record's value structure with it: the band merged every wash above it
/// onto one tone, so a four-colour sleeve painted as two and the surface read as a tint. Anything
/// reintroduced between `population_seeds` and here has to argue with this test.
#[test]
fn a_measured_wash_is_the_seed_untouched() {
    let painted = tints(MANY_HUES, &mocha());

    for (slot, seed) in MANY_HUES.iter().take(WASH_COUNT).enumerate() {
        let Some(seed) = *seed else {
            unreachable!("MANY_HUES is a full set")
        };
        assert_eq!(
            painted[slot].rgb, seed,
            "wash {slot} came back as {:#08x} against the seed {seed:#08x}",
            painted[slot].rgb
        );
    }
}

/// The paint takes three where the quantizer is asked for four, and the two counts are not one
/// number. Median cut splits to a target, so asking it for three gives different boxes rather than
/// these minus the last. That the paint is a *subset* is a `const _: () = assert!` in
/// `ui::aurora`; this is that it is the front.
#[test]
fn the_paint_takes_the_quantizers_first_three() {
    assert_eq!(tints(MANY_HUES, &mocha()).len(), WASH_COUNT);
}

/// The rule that keeps a monochrome record looking monochrome: a rotated hue is a guess, and on a
/// low-chroma sleeve it survives gamut mapping as almost nothing, so washing it on at full
/// strength would stack near-identical ramps into a lightness gradient the record never had.
#[test]
fn a_synthesized_tint_is_washed_on_more_faintly_than_a_real_one() {
    let real = tints(MANY_HUES, &mocha());
    assert!(real.iter().all(|t| t.weight >= 1.0), "a separated cover washes every tint in full");

    let [seeded, fills @ ..] = tints(ONE_HUE, &mocha());
    assert!(seeded.weight >= 1.0, "the seed the artwork gave is not a guess");
    for fill in fills {
        assert!(fill.weight < seeded.weight, "a fill matched the seed's weight");
    }
}

/// A near-white sleeve is filtered down to a single box, so a short list is still a shape the
/// quantizer produces and the filling rule still owes the rest.
#[test]
fn a_short_list_is_filled_from_its_own_hue_and_not_the_theme() {
    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    let seed_hue = hct_of(0x00c0_3030).get_hue();

    for tint in tints(ONE_HUE, &mocha()) {
        let gap_from_seed = hue_gap(hct_of(tint.rgb).get_hue(), seed_hue);
        let gap_from_accent = hue_gap(hct_of(tint.rgb).get_hue(), accent_hue);
        // One analogous step either side, the whole fan three washes needs.
        assert!(
            gap_from_seed <= 30.0,
            "fill {:#08x} drifted {gap_from_seed}° from the album; accent is {gap_from_accent}° away",
            tint.rgb
        );
    }
}

/// Fills rotating the same way by the same step would stack into near-duplicates; a fan either
/// side of the source is what keeps three washes reading as three.
#[test]
fn the_fills_fan_out_around_the_seed() {
    let painted = tints(ONE_HUE, &mocha());
    let hues: Vec<f64> = painted.iter().map(|tint| hct_of(tint.rgb).get_hue()).collect();

    assert!(
        hue_gap(hues[0], hct_of(0x00c0_3030).get_hue()) < 5.0,
        "tint 0 is the seed's own hue, got {:#08x}",
        painted[0].rgb
    );
    for (index, hue) in hues.iter().enumerate() {
        for other in hues.iter().skip(index + 1) {
            let gap = hue_gap(*hue, *other);
            assert!(gap > 15.0, "two tints collapsed to {gap}° apart");
        }
    }
}

/// A black-and-white record gets a black-and-white backdrop.
///
/// This used to take a chroma floor scaled by how colourful the whole image measured, because
/// `Score` handed over seeds carrying a few points of chroma either way and a wash covering the
/// surface needs very little of it to read as a colour. **The property is now the extractor's**:
/// what comes back is what the cover is made of, so a grey record yields greys and nothing lifts
/// them. Run end to end rather than off recorded seeds — the claim is about the pair.
#[test]
fn a_greyscale_cover_stays_grey() {
    let mut seeds = [None; SEED_COUNT];
    for (slot, seed) in seeds.iter_mut().zip(population_seeds(&greyscale_ramp(), SEED_COUNT)) {
        *slot = Some(seed);
    }

    for tint in tints(seeds, &mocha()) {
        let chroma = hct_of(tint.rgb).get_chroma();
        assert!(chroma < 6.0, "tint {:#08x} carries chroma {chroma} off a grey cover", tint.rgb);
    }
}

/// A cover of two colours must come out as two colours.
///
/// An earlier pass pulled every seed into a 40° arc of the dominant, on the reasoning that
/// overlapping washes composite in sRGB and its midpoint between distant hues is grey. Measured on
/// a real blue-and-red sleeve, that turned seeds at 231°/17°/304° into 231°/271°/270° — three
/// violets, the record's most vivid colour discarded. Separation is the Slint side's job, by
/// giving each sweep an edge; the solve's job is to hand over what the artwork had.
#[test]
fn a_multi_coloured_cover_keeps_its_colours_apart() {
    let blue_and_red = [
        Some(0x0038_718b),
        Some(0x00cc_2841),
        Some(0x0030_3446),
        Some(0x007d_5c79),
    ];
    let painted = tints(blue_and_red, &mocha());

    // The painted set is the quantizer's first `WASH_COUNT`; the box it drops is not this test's.
    for seed in blue_and_red.into_iter().take(WASH_COUNT).flatten() {
        let nearest = painted
            .iter()
            .map(|tint| hue_gap(hct_of(tint.rgb).get_hue(), hct_of(seed).get_hue()))
            .fold(f64::INFINITY, f64::min);
        assert!(nearest < 10.0, "seed {seed:#08x} has no wash within {nearest}° of it");
    }

    let hues: Vec<f64> = painted.iter().map(|tint| hct_of(tint.rgb).get_hue()).collect();
    let widest = hues
        .iter()
        .flat_map(|hue| hues.iter().map(move |other| hue_gap(*hue, *other)))
        .fold(0.0, f64::max);
    assert!(widest > 90.0, "blue and red collapsed to {widest}° apart");
}

/// No artwork at all is the only path that may reach for the theme, and it still owes a set of
/// distinguishable tints rather than one colour washed on repeatedly — the accent's own hue
/// throughout, and none of it above the wash ceiling.
#[test]
fn no_seeds_at_all_falls_back_to_the_theme_accent() {
    let [first, fills @ ..] = tints([None; SEED_COUNT], &mocha());

    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    assert!(
        hue_gap(hct_of(first.rgb).get_hue(), accent_hue) < 5.0,
        "tint 0 must be the accent's own hue, got {:#08x}",
        first.rgb
    );
    assert!(
        hct_of(first.rgb).get_tone() <= WASH_MAX_TONE + 1.0,
        "the accent went in as ink rather than as ground: tone {}",
        hct_of(first.rgb).get_tone()
    );
    for fill in fills {
        assert!(fill.rgb != first.rgb, "a fill duplicated the accent");
    }
}

/// The accent stands in as **two seeds**, not as one and not as a rotation origin.
///
/// Left as the origin, every wash comes out at the fill weight and the surface reads as unpainted
/// `Theme.base`; seated alone, the pair the sweeps need to have any value structure is a fan of two
/// ghosts and the band reads as a flat tint. Asserted against the genre shape rather than against
/// the numbers, since what is claimed is that the two coverless heroes are one case.
#[test]
fn an_empty_set_washes_the_pair_a_genre_hands_over() {
    let art_less = tints([None; SEED_COUNT], &mocha());
    let genre = tints([Some(0x00c0_3030), Some(0x0080_2020), None, None], &mocha());

    let weights: Vec<f32> = art_less.iter().map(|tint| tint.weight).collect();
    let reference: Vec<f32> = genre.iter().map(|tint| tint.weight).collect();
    assert_eq!(weights, reference, "an art-less set must carry a seated pair's weights");
    assert!(weights[0] >= 1.0 && weights[1] >= 1.0, "both seated washes are laid on in full");
}

/// The second seated wash is the darker one, which is the whole of what the pair buys: one colour
/// swept three ways is a tint whichever way it runs.
#[test]
fn the_seated_accent_pair_runs_light_to_dark() {
    let [lit, shade, _] = tints([None; SEED_COUNT], &mocha());

    let lit_tone = hct_of(lit.rgb).get_tone();
    let shade_tone = hct_of(shade.rgb).get_tone();
    assert!(shade_tone < lit_tone, "the pair has no value structure: {lit_tone} then {shade_tone}");
    assert!(
        (shade_tone - lit_tone * WASH_SHADE_FACTOR).abs() < 1.0,
        "the shade drifted off its factor: {shade_tone} against {}",
        lit_tone * WASH_SHADE_FACTOR
    );
}

/// A **ceiling, not a tone** — an accent already deep enough to be ground reaches the wash carrying
/// its own chroma. Latte's mauve is that accent, and it is the half of this a `set_tone` would break
/// invisibly: on Mocha, whose accent is a pastel, the two spellings agree.
#[test]
fn an_accent_below_the_ceiling_washes_untouched() {
    const LATTE_ACCENT: u32 = 0x0088_39ef;
    let latte = ThemeTokens {
        base: 0x00ef_f1f5,
        text: 0x004c_4f69,
        accent: LATTE_ACCENT,
    };

    assert!(
        hct_of(LATTE_ACCENT).get_tone() < WASH_MAX_TONE,
        "fixture no longer sits under the ceiling, so this asserts nothing"
    );
    let [lit, ..] = tints([None; SEED_COUNT], &latte);
    assert_eq!(lit.rgb, LATTE_ACCENT, "a seated accent lost chroma it never had to spend");
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
