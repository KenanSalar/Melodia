use material_colors::color::Argb;
use material_colors::hct::Hct;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::services::material_you::{extract_seeds_from_rgb8, extract_source_argb_from_rgb8};
use crate::ui::aurora::{TINT_TONE, dither_tile, tints};
use crate::ui::backdrop::SEED_COUNT;

/// The default accent, standing in for "no artwork" wherever a fallback is exercised.
const THEME_ACCENT: u32 = 0x00cb_a6f7;

/// A cover whose quantize separated a full set of hues — the real seeds off `Real for Me`.
const MANY_HUES: [Option<u32>; SEED_COUNT] = [
    Some(0x0038_718b),
    Some(0x00cc_2841),
    Some(0x0030_3446),
    Some(0x007d_5c79),
];

/// Mean chroma of a cover with real colour in it — measured, two ordinary sleeves sit at 22–24.
const COLOURFUL: f64 = 22.0;

/// A monochrome sleeve: one seed, the rest owed to the filling rule.
const ONE_HUE: [Option<u32>; SEED_COUNT] = [Some(0x00c0_3030), None, None, None];

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
    let many = extract_seeds_from_rgb8(&buf, SEED_COUNT).seeds.into_iter().next();

    assert_eq!(one, many);
    assert!(one.is_some(), "a four-hue buffer must quantize to something");
}

/// A greyscale sleeve is filtered out by `Score`'s chroma cutoff, and the crate's own answer there
/// is Google Blue — a brand colour, not a fact about this record. `seed_from_pixels` hands the real
/// dominant over as the fallback, and the ranked call has to keep doing so.
#[test]
fn a_monochrome_sleeve_seeds_from_its_own_grey() {
    let buf = buffer_of(&[[90, 90, 90], [120, 120, 120]]);

    let quantized = extract_seeds_from_rgb8(&buf, SEED_COUNT);

    let chroma = quantized.seeds.first().map(|&first| hct_of(first).get_chroma());
    assert!(chroma.is_some(), "the dominant fallback means a grey buffer still answers");
    assert!(
        chroma.is_some_and(|c| c < 5.0),
        "a grey buffer must not answer with a hue, got {:#08x?}",
        quantized.seeds
    );
    assert!(
        quantized.chroma < 5.0,
        "a grey buffer must read as colourless overall, got {}",
        quantized.chroma
    );
}

// --- the washes ---------------------------------------------------------------

/// Every tint sits at one tone, so the only axis they differ on is hue. A tint brighter than its
/// neighbours turns its wash into a lightness ramp, which is the one thing a blurred cover never
/// produced.
#[test]
fn every_tint_lands_on_one_tone() {
    for seeds in [MANY_HUES, ONE_HUE, [None; SEED_COUNT]] {
        for tint in tints(seeds, COLOURFUL, THEME_ACCENT) {
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
    let washed_out = [
        Some(0x00c0_3030),
        Some(0x00f2_efee),
        Some(0x0005_0408),
        Some(0x00ee_ecef),
    ];

    for tint in tints(washed_out, COLOURFUL, THEME_ACCENT) {
        let chroma = hct_of(tint.rgb).get_chroma();
        assert!(chroma > 20.0, "tint {:#08x} came back grey at chroma {chroma}", tint.rgb);
    }
}

/// The rule that keeps a monochrome record looking monochrome: a rotated hue is a guess, and on a
/// low-chroma sleeve it survives gamut mapping as almost nothing, so washing it on at full
/// strength would stack near-identical ramps into a lightness gradient the record never had.
#[test]
fn a_synthesized_tint_is_washed_on_more_faintly_than_a_real_one() {
    let real = tints(MANY_HUES, COLOURFUL, THEME_ACCENT);
    assert!(real.iter().all(|t| t.weight >= 1.0), "a separated cover washes every tint in full");

    let [seeded, fills @ ..] = tints(ONE_HUE, COLOURFUL, THEME_ACCENT);
    assert!(seeded.weight >= 1.0, "the seed the artwork gave is not a guess");
    for fill in fills {
        assert!(fill.weight < seeded.weight, "a fill matched the seed's weight");
    }
}

/// `Score` returns fewer than asked rather than reaching for near-duplicates, and never pads. A
/// duotone cover therefore arrives two seeds short of a tint and the filling rule owes the rest.
#[test]
fn a_short_list_is_filled_from_its_own_hue_and_not_the_theme() {
    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    let seed_hue = hct_of(0x00c0_3030).get_hue();

    for tint in tints(ONE_HUE, COLOURFUL, THEME_ACCENT) {
        let gap_from_seed = hue_gap(hct_of(tint.rgb).get_hue(), seed_hue);
        let gap_from_accent = hue_gap(hct_of(tint.rgb).get_hue(), accent_hue);
        // The fan reaches two analogous steps at its far end, which is what four of them costs
        // over three — still nearer the album than the app's own accent by a wide margin.
        assert!(
            gap_from_seed <= 55.0,
            "fill {:#08x} drifted {gap_from_seed}° from the album; accent is {gap_from_accent}° away",
            tint.rgb
        );
    }
}

/// Fills rotating the same way by the same step would stack into near-duplicates; a fan either
/// side of the source is what keeps four washes reading as four.
#[test]
fn the_fills_fan_out_around_the_seed() {
    let painted = tints(ONE_HUE, COLOURFUL, THEME_ACCENT);
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
/// Such a sleeve still quantizes to seeds carrying a few points of chroma — noise and a hint of
/// tint in a near-black field — and **neither bound may take them at face value**: lifted to the
/// floor they painted it red and violet, and left at their own 9 they still washed the whole
/// surface mauve, a tint covering everything needing very little chroma to read as a colour. The
/// seeds can't be asked which case they are, this one's 9.4 sitting below a colourful cover's
/// 12.6; only the image separates them.
#[test]
fn a_greyscale_cover_stays_grey() {
    // The real seeds off `Fade Into Darkness`, whose mean chroma measures ~5.
    let greyscale = [
        Some(0x0005_0103),
        Some(0x0069_94a0),
        Some(0x005b_84a4),
        Some(0x0003_0611),
    ];

    for tint in tints(greyscale, 5.1, THEME_ACCENT) {
        let chroma = hct_of(tint.rgb).get_chroma();
        assert!(chroma < 6.0, "tint {:#08x} carries chroma {chroma} off a grey cover", tint.rgb);
    }

    // The same seeds under colourful artwork still get the lift — the image decides, not the seed.
    let lifted = tints(greyscale, COLOURFUL, THEME_ACCENT);
    assert!(
        hct_of(lifted[0].rgb).get_chroma() > 30.0,
        "the band stopped opening for artwork that has colour"
    );
}

/// A cover of two colours must come out as two colours.
///
/// An earlier pass pulled every seed into a 40° arc of the dominant, on the reasoning that
/// overlapping washes composite in sRGB and its midpoint between distant hues is grey. Measured on
/// a real blue-and-red sleeve, that turned seeds at 231°/17°/304° into 231°/271°/270° — three
/// violets, the record's most vivid colour discarded. Separation is the Slint side's job, by
/// giving each blob a region; the solve's job is to hand over what the artwork had.
#[test]
fn a_multi_coloured_cover_keeps_its_colours_apart() {
    let blue_and_red = [
        Some(0x0038_718b),
        Some(0x00cc_2841),
        Some(0x0030_3446),
        Some(0x007d_5c79),
    ];
    let painted = tints(blue_and_red, COLOURFUL, THEME_ACCENT);

    // Matched by hue rather than by slot: paint order is the hue wheel, not `Score`'s ranking.
    for seed in blue_and_red.into_iter().flatten() {
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

/// Consecutive washes are consecutive on the hue wheel: the blob positions are fixed, so the rank
/// a colour arrives in decides which other colour it overlaps, and `Score`'s order once put two
/// complementary pairs on top of each other. The failure is invisible in review — every colour in
/// the solve is correct and only the composite is wrong.
#[test]
fn the_washes_are_seated_around_the_hue_wheel() {
    // Deliberately adversarial: ranked so the two complementary pairs land adjacent.
    let complementary_by_rank = [
        Some(0x0063_4def),
        Some(0x00e5_2508),
        Some(0x00b7_eaed),
        Some(0x00de_eacc),
    ];
    let painted = tints(complementary_by_rank, COLOURFUL, THEME_ACCENT);
    let anchor = hct_of(painted[0].rgb).get_hue();
    let turn = |rgb: u32| (hct_of(rgb).get_hue() - anchor).rem_euclid(360.0);

    for pair in painted.windows(2) {
        assert!(
            turn(pair[0].rgb) <= turn(pair[1].rgb),
            "the washes stopped walking one way round the wheel, so two of them can be \
             complementary neighbours again"
        );
    }

    // The anchor stays the top-ranked seed: it carries the strongest wash and the base gradient
    // under it is solved from the same seed, so starting elsewhere strands the floor's own hue.
    let dominant = hue_gap(anchor, hct_of(0x0063_4def).get_hue());
    assert!(dominant < 10.0, "wash 0 is {dominant}° off the dominant seed");
}

/// No artwork at all is the only path that may reach for the theme, and it still owes a set of
/// distinguishable tints rather than one colour washed on repeatedly.
#[test]
fn no_seeds_at_all_falls_back_to_the_theme_accent() {
    let [first, fills @ ..] = tints([None; SEED_COUNT], COLOURFUL, THEME_ACCENT);

    let accent_hue = hct_of(THEME_ACCENT).get_hue();
    assert!(
        hue_gap(hct_of(first.rgb).get_hue(), accent_hue) < 5.0,
        "tint 0 must be the accent's own hue, got {:#08x}",
        first.rgb
    );
    for fill in fills {
        assert!(fill.rgb != first.rgb, "a fill duplicated the accent");
    }
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
