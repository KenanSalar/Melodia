use material_colors::color::{linearized, lstar_from_y, y_from_lstar};
use material_colors::contrast::ratio_of_tones;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::aurora::WASH_COUNT;
use crate::ui::backdrop::{
    BackdropColors, BackdropKind, BackdropSample, CHROME_MAX_TONE, CHROME_RATIO, ThemeTokens,
    chrome_tone, composited_tone, floor_luma, gradient_luma, luma_p90, muted_tone, rgb_lstar,
    scrim_alpha, solve, text_tone,
};

/// Catppuccin Mocha's mauve, the default accent — a realistic seed for the
/// solve tests below.
const SEED: u32 = 0x00cb_a6f7;

/// The two shipped palettes at opposite polarities, copied from
/// `themes::catppuccin`. Not imported from it: these are what the aurora's cap
/// is *argued* against, so a palette edit should fail here and be re-derived
/// rather than silently move every bound.
fn mocha() -> ThemeTokens {
    ThemeTokens {
        base: 0x001e_1e2e,
        text: 0x00cd_d6f4,
        accent: SEED,
    }
}

fn latte() -> ThemeTokens {
    ThemeTokens {
        base: 0x00ef_f1f5,
        text: 0x004c_4f69,
        accent: 0x0088_39ef,
    }
}

/// Build a `BLUR_TARGET`-ish square buffer from a per-pixel closure.
fn buffer_from(side: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> SharedPixelBuffer<Rgb8Pixel> {
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(side, side);
    let px = buf.make_mut_slice();
    for y in 0..side {
        for x in 0..side {
            let [r, g, b] = f(x, y);
            px[(y * side + x) as usize] = Rgb8Pixel { r, g, b };
        }
    }
    buf
}

fn solid(side: u32, v: u8) -> SharedPixelBuffer<Rgb8Pixel> {
    buffer_from(side, |_, _| [v, v, v])
}

/// [`luma_p90`] on a buffer the caller knows is non-empty. The impossible
/// `None` becomes `NaN`, which fails every comparison below rather than
/// slipping through — `unwrap` is denied crate-wide, tests included.
fn p90(buf: &SharedPixelBuffer<Rgb8Pixel>) -> f64 {
    luma_p90(buf.as_bytes()).unwrap_or(f64::NAN)
}

/// [`BackdropSample::measure`] where the sharp and the painted buffer are one — every test
/// below is about the solve rather than about which buffer each half reads, and a synthetic
/// buffer is its own blur anyway. The production split is pinned in `artwork_cache_tests`.
fn measured(buf: &SharedPixelBuffer<Rgb8Pixel>) -> BackdropSample {
    BackdropSample::measure(buf.as_bytes(), buf.as_bytes())
}

/// Independent WCAG relative luminance of one sRGB byte triple, 0..1. Written
/// from the spec rather than reusing the module's helpers so an assertion can't
/// pass by agreeing with a bug in them.
fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    let lin = |c: u8| {
        let n = f64::from(c) / 255.0;
        if n <= 0.040_449_936 {
            n / 12.92
        } else {
            ((n + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG 1.4.3 contrast between a packed `0x00RR_GGBB` foreground and a
/// backdrop given as an HCT tone — the shape every "does this tier survive"
/// assertion below needs.
fn ratio_against_tone(rgb: u32, backdrop_tone: f64) -> f64 {
    let (r, g, b) = unpack(rgb);
    let fg = relative_luminance(r, g, b);
    let bg = y_from_lstar(backdrop_tone) / 100.0;
    (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05)
}

fn unpack(rgb: u32) -> (u8, u8, u8) {
    (((rgb >> 16) & 0xff) as u8, ((rgb >> 8) & 0xff) as u8, (rgb & 0xff) as u8)
}

// --- the linearisation table --------------------------------------------------

#[test]
fn the_linearisation_table_answers_for_every_channel_value() {
    // `pixel_lstar` reads a 256-entry table rather than calling `linearized`
    // three times per pixel. The failure a table invites is one entry short at
    // the top, which leaves pure white — what a bright sleeve is full of, and
    // the case the percentile is built to catch — reading as black.
    //
    // The luma weights sum to 1, so a grey pixel's `y` is exactly its own
    // linearised channel, and the whole curve is reachable through `rgb_lstar`
    // without exposing the table.
    for byte in 0u8..=u8::MAX {
        let grey = u32::from_be_bytes([0, byte, byte, byte]);
        let expected = lstar_from_y(linearized(byte));
        let actual = rgb_lstar(grey);
        assert!(
            (actual - expected).abs() < 1e-9,
            "channel {byte}: table gave {actual}, `linearized` gives {expected}",
        );
    }
}

// --- luma_p90 ---------------------------------------------------------------

#[test]
fn luma_p90_rejects_an_empty_buffer() {
    let buf = SharedPixelBuffer::<Rgb8Pixel>::new(0, 0);
    assert_eq!(luma_p90(buf.as_bytes()), None);
}

#[test]
fn luma_p90_reads_a_white_buffer_as_near_maximum() {
    let luma = p90(&solid(32, 255));
    assert!(luma > 97.0, "pure white read as L*{luma}");
}

#[test]
fn luma_p90_reads_a_black_buffer_as_near_zero() {
    let luma = p90(&solid(32, 0));
    assert!(luma < 3.0, "pure black read as L*{luma}");
}

/// The regression that pins the percentile choice. A sleeve that is mostly
/// black with a white wordmark has a low *mean* — which would say "dark
/// backdrop, brighten the chrome" — while the region the title sits on is
/// near-white. The percentile has to see the mark.
#[test]
fn luma_p90_sees_a_bright_mark_a_mean_would_miss() {
    // 20% of the buffer is white, the rest black.
    let side = 40;
    let buf = buffer_from(side, |_, y| {
        if y < side / 5 {
            [255, 255, 255]
        } else {
            [0, 0, 0]
        }
    });
    let luma = p90(&buf);

    // The mean lightness of this buffer is low — that is the statistic being
    // rejected, spelled out here so the contrast is explicit.
    let mean_lstar = 0.2 * 100.0;
    assert!(
        luma > mean_lstar,
        "p90 (L*{luma}) must exceed the 20%-white mean contribution (L*{mean_lstar})"
    );
    assert!(luma > 90.0, "p90 must land in the white mark, got L*{luma}");
}

#[test]
fn luma_p90_steps_over_a_tail_smaller_than_the_percentile() {
    // 2% white — inside the 10% tail, so the percentile should report the
    // black body, not the speck.
    let side = 50;
    let buf = buffer_from(side, |x, y| {
        if y == 0 && x < side / 2 {
            [255, 255, 255]
        } else {
            [0, 0, 0]
        }
    });
    let luma = p90(&buf);
    assert!(luma < 10.0, "a 1% speck must not drive the scrim, got L*{luma}");
}

// --- BackdropSample ---------------------------------------------------------

/// Each half reads the buffer it was handed, which is the whole reason there are two: the seeds
/// want the sharp downscale and the percentile wants whatever the scrim gets painted over.
/// Handing one buffer twice — what every other test here does — cannot tell the two apart.
#[test]
fn measure_takes_the_hue_off_the_sharp_buffer_and_the_percentile_off_the_painted_one() {
    let sharp = buffer_from(32, |_, _| [220, 30, 30]);
    let painted = solid(32, 0x20);
    let sample = BackdropSample::measure(sharp.as_bytes(), painted.as_bytes());

    assert_eq!(
        sample.luma,
        luma_p90(painted.as_bytes()),
        "the percentile must read the painted buffer"
    );
    assert_ne!(sample.luma, luma_p90(sharp.as_bytes()), "...and must not read the sharp one");

    // Grey would be the answer off `painted`, so a red seed can only have come off `sharp`.
    let (r, g, b) = unpack(sample.accent_argb.unwrap_or(0));
    assert!(r > g && r > b, "the seed must quantize the sharp buffer, got rgb({r}, {g}, {b})");
}

/// An empty buffer must leave both halves empty so the publisher falls back to
/// the theme accent and the gradient floor, rather than seeding the whole band
/// off whatever a degenerate quantize happened to return.
#[test]
fn measure_of_an_empty_buffer_leaves_both_halves_empty() {
    let sample = measured(&SharedPixelBuffer::<Rgb8Pixel>::new(0, 0));
    assert_eq!(sample.accent_argb, None);
    assert_eq!(sample.luma, None);
}

/// The seed is what decides the hue of both layers sandwiching the blur, so a
/// measured cover and the theme accent must not collapse onto one answer. The
/// regression this guards is the hero going back to seeding from `Theme.accent`
/// and painting every album's banner the same colour.
#[test]
fn a_measured_cover_hue_outranks_the_theme_accent() {
    let sample = measured(&buffer_from(32, |_, _| [220, 30, 30]));
    // `None` collapses onto the accent too, which is the same failure — so
    // falling back to `SEED` here makes the assertion below cover both.
    let seed = sample.accent_argb.unwrap_or(SEED);
    let luma = sample.luma.unwrap_or(f64::NAN);
    assert_ne!(
        solve(seed, luma).chrome,
        solve(SEED, luma).chrome,
        "a red cover must not solve to the mauve accent's colour set"
    );
}

/// Channel spread of a packed `0x00RR_GGBB` — a proxy for "is this neutral"
/// that doesn't reuse the HCT machinery the solve runs on.
fn channel_spread(rgb: u32) -> u8 {
    let (r, g, b) = unpack(rgb);
    r.max(g).max(b) - r.min(g).min(b)
}

/// The regression this pins is the hero's chips coming out vivid periwinkle on
/// a greyscale sleeve: `Score` discards every cluster under its chroma cutoff
/// and used to answer Google Blue, which then seeded the whole set. A grey
/// banner has to solve grey chrome.
#[test]
fn a_greyscale_blur_solves_neutral_chrome() {
    let chrome = measured(&solid(32, 0x80)).solve(&mocha(), BackdropKind::Blur).chrome;
    assert!(
        channel_spread(chrome) <= 8,
        "a grey blur must solve neutral chrome, got 0x{chrome:06X}"
    );
}

/// The two ends of the achromatic range, where the seed carries no hue at all
/// and the solve owns every tone: black has to be lifted clear of its own
/// backdrop, white has to be held below tone 100, and both stay neutral.
#[test]
fn a_black_and_a_white_blur_both_solve_legible_chrome() {
    for value in [0x00_u8, 0xff] {
        let sample = measured(&solid(32, value));
        let colors = sample.solve(&mocha(), BackdropKind::Blur);
        let band = composited_tone(sample.luma.unwrap_or(f64::NAN), colors.scrim_alpha);

        let ratio = ratio_against_tone(colors.chrome, band);
        assert!(
            ratio >= CHROME_RATIO,
            "0x{value:02X} sleeve: chrome 0x{:06X} reads {ratio:.2}:1 on its own band",
            colors.chrome
        );
        assert!(
            channel_spread(colors.chrome) <= 8,
            "0x{value:02X} sleeve: chrome 0x{:06X} is not neutral",
            colors.chrome
        );
    }
}

/// A near-white sleeve quantizes to tone 100, above every text band there is,
/// and the chrome tier only ever *raised* a seed's tone — so its chips painted
/// brighter than the title they sit under. The ceiling now bounds the seed and
/// not just the solve.
#[test]
fn the_chrome_tier_stays_inside_its_band() {
    let chrome = measured(&solid(32, 0xff)).solve(&mocha(), BackdropKind::Blur).chrome;
    let tone = rgb_lstar(chrome);
    assert!(
        tone <= CHROME_MAX_TONE + 0.5,
        "a white sleeve solved chrome at tone {tone:.1}, above the {CHROME_MAX_TONE} ceiling"
    );
}

// --- scrim_alpha ------------------------------------------------------------

#[test]
fn scrim_alpha_is_heaviest_on_a_white_backdrop() {
    let a = scrim_alpha(100.0);
    assert!(a > 0.70, "a white backdrop needs a heavy scrim, got {a}");
    assert!(a <= 0.82, "must respect the ceiling, got {a}");
}

#[test]
fn scrim_alpha_floors_on_an_already_dark_backdrop() {
    // Anything at or below the target tone has no darkening left to do.
    for luma in [0.0, 5.0, 20.0, 32.0] {
        let a = scrim_alpha(luma);
        assert!((a - 0.30).abs() < f32::EPSILON, "L*{luma} should take the floor, got {a}");
    }
}

#[test]
fn scrim_alpha_is_monotone_in_backdrop_luma() {
    let mut previous = scrim_alpha(0.0);
    for step in 1..=100 {
        let a = scrim_alpha(f64::from(step));
        assert!(a >= previous, "alpha dropped from {previous} to {a} at L*{step}");
        previous = a;
    }
}

#[test]
fn scrim_alpha_is_snapped_to_whole_percents() {
    for step in 0..=100 {
        let a = scrim_alpha(f64::from(step));
        let percents = a * 100.0;
        assert!((percents - percents.round()).abs() < 1e-3, "L*{step} produced an unsnapped {a}");
    }
}

// --- composited_tone --------------------------------------------------------

/// The whole point of the scrim solve: however bright the cover, what the
/// foreground actually sits on lands in the target band.
#[test]
fn composited_tone_lands_in_the_target_band_for_every_backdrop() {
    for step in 0..=100 {
        let luma = f64::from(step);
        let tone = composited_tone(luma, scrim_alpha(luma));
        assert!(tone <= 33.0, "L*{luma} composited to L*{tone}, above the target band");
    }
}

#[test]
fn composited_tone_never_brightens_the_backdrop() {
    for step in 0..=100 {
        let luma = f64::from(step);
        let tone = composited_tone(luma, scrim_alpha(luma));
        // The scrim is near-black, so compositing can only darken — except at
        // the very bottom, where the scrim is the lighter of the two.
        assert!(tone <= luma.max(9.0), "L*{luma} composited *up* to L*{tone}");
    }
}

// --- the tone solvers -------------------------------------------------------

/// Sweep every backdrop tone the scrim can leave behind and assert each tier
/// clears the WCAG bar it is solved for.
#[test]
fn every_tier_clears_its_contrast_target_across_the_band() {
    for step in 0..=33 {
        let bg = f64::from(step);
        let chrome = ratio_of_tones(chrome_tone(bg), bg);
        let text = ratio_of_tones(text_tone(bg), bg);
        let muted = ratio_of_tones(muted_tone(bg), bg);
        assert!(chrome >= 3.0, "chrome only {chrome}:1 on L*{bg}");
        assert!(text >= 4.5, "text only {text}:1 on L*{bg}");
        assert!(muted >= 3.0, "muted only {muted}:1 on L*{bg}");
    }
}

#[test]
fn text_always_outranks_muted_which_always_outranks_nothing() {
    for step in 0..=33 {
        let bg = f64::from(step);
        assert!(
            text_tone(bg) > muted_tone(bg),
            "hierarchy inverted on L*{bg}: text {} vs muted {}",
            text_tone(bg),
            muted_tone(bg)
        );
    }
}

#[test]
fn chrome_tone_never_drops_below_the_inherited_floor() {
    // The one-sided fix this replaces pinned the accent at tone 70; holding
    // that as the band floor is what guarantees no cover regresses.
    for step in 0..=33 {
        let tone = chrome_tone(f64::from(step));
        assert!(tone >= 70.0, "chrome tone {tone} fell below the old floor");
        assert!(tone <= 92.0, "chrome tone {tone} exceeded the band");
    }
}

// --- floor_luma -------------------------------------------------------------

#[test]
fn floor_luma_is_dark_enough_to_need_no_extra_scrim() {
    let luma = floor_luma();
    assert!(
        luma > 0.0 && luma < 32.0,
        "the art-less gradient floor must already be inside the target band, got L*{luma}"
    );
    let a = scrim_alpha(luma);
    assert!((a - 0.30).abs() < f32::EPSILON, "the floor should take the minimum scrim, got {a}");
}

// --- solve ------------------------------------------------------------------

#[test]
fn solve_keeps_the_scrim_and_floor_dark_whatever_the_seed() {
    for seed in [SEED, 0x00ff_ffff, 0x0000_0000, 0x0000_ff00] {
        let BackdropColors {
            scrim,
            floor_start,
            floor_end,
            ..
        } = solve(seed, 100.0);
        for (name, rgb) in [
            ("scrim", scrim),
            ("floor_start", floor_start),
            ("floor_end", floor_end),
        ] {
            let (r, g, b) = unpack(rgb);
            let y = relative_luminance(r, g, b);
            assert!(y < 0.06, "{name} for seed {seed:#08x} is not dark (Y={y}), rgb={rgb:#08x}");
        }
    }
}

#[test]
fn solve_gives_a_bright_cover_a_heavier_scrim_than_a_dark_one() {
    let bright = solve(SEED, 95.0);
    let dark = solve(SEED, 5.0);
    assert!(
        bright.scrim_alpha > dark.scrim_alpha,
        "bright {} should out-scrim dark {}",
        bright.scrim_alpha,
        dark.scrim_alpha
    );
}

/// A dark cover must end up *lighter*-scrimmed than the fixed 45% this
/// replaces — the artwork gets to show more than it used to.
#[test]
fn solve_relaxes_the_scrim_below_the_old_fixed_alpha_on_a_dark_cover() {
    assert!(solve(SEED, 5.0).scrim_alpha < 0.45);
}

#[test]
fn solve_text_tiers_are_less_saturated_than_the_chrome_tier() {
    // Chroma isn't directly observable from RGB here, so compare the channel
    // spread — a near-neutral colour has a narrow one.
    let spread = |rgb: u32| {
        let (r, g, b) = (
            i32::try_from((rgb >> 16) & 0xff).unwrap_or(0),
            i32::try_from((rgb >> 8) & 0xff).unwrap_or(0),
            i32::try_from(rgb & 0xff).unwrap_or(0),
        );
        r.max(g).max(b) - r.min(g).min(b)
    };
    // A vivid seed, so the chrome tier has real saturation to lose.
    let colors = solve(0x0000_66ff, 95.0);
    assert!(
        spread(colors.text) < spread(colors.chrome),
        "text spread {} should be under chrome spread {}",
        spread(colors.text),
        spread(colors.chrome)
    );
    assert!(spread(colors.muted) < spread(colors.chrome));
}

/// End-to-end: the failure that motivated all of this. Under the old fixed
/// scrim a white sleeve left the tone-70 accent at ~1.4:1.
#[test]
fn a_white_cover_now_clears_the_non_text_bar() {
    let luma = p90(&solid(32, 252));
    let tone = composited_tone(luma, scrim_alpha(luma));
    assert!(
        ratio_of_tones(chrome_tone(tone), tone) >= 3.0,
        "a white sleeve still fails the 3:1 bar (backdrop L*{tone})"
    );
    assert!(ratio_of_tones(text_tone(tone), tone) >= 4.5);
}

// --- the aurora's washes ------------------------------------------------------
//
// **There is no cap to test any more, and that is the arm's whole design.** What used to live here
// — a band `wash_cap` returned, and a walk over every pixel where two washes meet asserting the
// theme's ink still cleared its bar — was the machinery that flattened the surface: holding the
// composite inside a known band means holding every wash above that band on one tone. The washes
// are the quantizer's own answer now, so contrast against `Theme.text` is the cover's to decide.
//
// What still carries a guarantee is the neutral chrome tier, pinned below.

/// The two arms answer different questions, so nothing may leak between them. The blur half is
/// asserted alongside so the pair can't both pass on a solve that ignored its argument.
#[test]
fn the_aurora_arm_publishes_the_theme_and_the_blur_arm_solves() {
    let theme = mocha();
    let sample = measured(&solid(32, 0xff));

    let aurora_arm = sample.solve(&theme, BackdropKind::Aurora);
    assert_eq!(
        (aurora_arm.floor_start, aurora_arm.floor_end),
        (theme.base, theme.base),
        "the aurora's gradient is the theme's own base"
    );
    // Both text tiers are the same neutral ink as the chrome, told apart by weight alone — the
    // theme's own `text`/`subtext1` carry their palette's cast, which over an album's washes is a
    // second hue arguing with them.
    assert_eq!(
        (aurora_arm.text, aurora_arm.muted),
        (aurora_arm.chrome, aurora_arm.chrome),
        "the aurora's text tiers left the neutral ink"
    );
    assert!(
        aurora_arm.muted_alpha < aurora_arm.text_alpha,
        "the aurora's two text tiers are one colour, so a weight gap is the only hierarchy left"
    );

    let blur_arm = sample.solve(&theme, BackdropKind::Blur);
    assert_ne!(
        (blur_arm.chrome, blur_arm.text, blur_arm.muted),
        (theme.accent, theme.text, theme.text),
        "the blur still solves its foreground against what it measured"
    );
}

/// The aurora's chrome is neutral ink at partial alpha, and takes its polarity from the theme.
///
/// **Arrived at the hard way.** Two attempts to derive this tier from the artwork shipped and were
/// reverted: the dominant seed is one of four washes and argues with the other three, and the hue
/// they composite to is a mean nothing on screen actually is. A neutral ink lets the wash it
/// happens to sit on supply the colour, which is right everywhere on the surface at once — and the
/// alpha is the whole mechanism, so an opaque one would be the bug rather than a tuning miss.
///
/// Polarity comes off base-versus-ink, not a variant id: two of the six palettes are generated at
/// runtime and have none to match on.
#[test]
fn the_auroras_chrome_is_neutral_ink_the_wash_reads_through() {
    for (name, theme, expected) in [
        ("mocha", mocha(), 0x00ff_ffff),
        ("latte", latte(), 0x0000_0000),
    ] {
        let colors =
            measured(&buffer_from(32, |_, _| [220, 30, 30])).solve(&theme, BackdropKind::Aurora);

        assert_eq!(
            colors.chrome, expected,
            "{name}'s aurora chrome must be the ink its own base calls for, not a colour"
        );
        assert!(
            colors.chrome_alpha > 0.0 && colors.chrome_alpha < 1.0,
            "{name}'s aurora chrome is opaque — the wash beneath is what colours it, so an opaque \
             tier is a white or black glyph sitting on the surface rather than belonging to it"
        );
    }

    let blur = measured(&buffer_from(32, |_, _| [220, 30, 30])).solve(&mocha(), BackdropKind::Blur);
    assert!(
        (blur.chrome_alpha - 1.0).abs() < f32::EPSILON,
        "the blur's chrome is the colour itself and must stay opaque"
    );
}

/// What holds across every retune of the neutral weights, which is deliberately not their order.
///
/// Two things are structural: no weight may go opaque, the wash reading through being the whole
/// mechanism, and a chip's pill may not reach the lettering on it, a backing that outshines its own
/// label having stopped being a backing. Everything else about how the weights rank is taste, so
/// pinning it here would only make a tune fail a test that agreed with it.
#[test]
fn no_neutral_weight_goes_opaque_or_outshines_what_sits_on_it() {
    let aurora =
        measured(&buffer_from(32, |_, _| [220, 30, 30])).solve(&mocha(), BackdropKind::Aurora);

    for (name, alpha) in [
        ("chrome", aurora.chrome_alpha),
        ("chrome-text", aurora.chrome_text_alpha),
        ("chip-fill", aurora.chip_fill_alpha),
        ("viz", aurora.viz_alpha),
        ("text", aurora.text_alpha),
        ("muted", aurora.muted_alpha),
    ] {
        assert!(
            alpha > 0.0 && alpha < 1.0,
            "the aurora's {name} weight is {alpha} — opaque ink stops letting the record through, \
             and nothing at all is not a tier"
        );
    }
    assert!(
        aurora.chip_fill_alpha < aurora.chrome_text_alpha,
        "the chip pill ({}) reached the lettering on it ({})",
        aurora.chip_fill_alpha,
        aurora.chrome_text_alpha
    );

    // The blur's tier *is* the colour, so everything but the pill is opaque there — asserted so the
    // partial weights above can't be read as a property of both arms.
    let blur = measured(&buffer_from(32, |_, _| [220, 30, 30])).solve(&mocha(), BackdropKind::Blur);
    for (name, alpha) in [
        ("chrome-text", blur.chrome_text_alpha),
        ("viz", blur.viz_alpha),
        ("text", blur.text_alpha),
    ] {
        assert!(
            (alpha - 1.0).abs() < f32::EPSILON,
            "the blur's {name} is a tone it solved per cover and must stay opaque, got {alpha}"
        );
    }
    assert!(blur.chip_fill_alpha < blur.chrome_alpha, "the blur's pill reached its glyph weight");
}

/// An entry with no artwork takes the arm the setting picked, like every other entry.
///
/// It used to keep the blur under either, the aurora's only fallback then being fills with no seed
/// behind them; `aurora::tints` takes the accent as that seed now, so the surface has something to
/// wash and the tiers under it are the theme's. Both arms are asserted, since a guard put back
/// would leave the aurora arm solving the blur's colours and nothing else would notice — the mount
/// gate that used to agree with it is gone.
#[test]
fn an_entry_with_no_artwork_still_follows_the_setting() {
    let theme = mocha();
    let empty = BackdropSample::default();

    let aurora_arm = empty.solve(&theme, BackdropKind::Aurora);
    assert_eq!(
        (aurora_arm.floor_start, aurora_arm.floor_end),
        (theme.base, theme.base),
        "an art-less surface takes the theme's own flat base on the aurora"
    );

    let blur_arm = empty.solve(&theme, BackdropKind::Blur);
    assert_ne!(
        (blur_arm.floor_start, blur_arm.floor_end),
        (theme.base, theme.base),
        "the blur arm keeps the accent-seeded floor this view was built on"
    );
    assert_ne!(
        (aurora_arm.chrome, aurora_arm.text),
        (blur_arm.chrome, blur_arm.text),
        "the two arms must differ, or the equality above passes on an arm that stopped solving"
    );
}

/// A white and a black cover leave the aurora identical — the surface is the theme's, and the
/// measurement reaches nothing on this arm. The blur's own scrim still moves, which is what keeps
/// the equality from comparing one input with itself.
#[test]
fn the_aurora_tiers_ignore_what_the_cover_measured() {
    let theme = mocha();
    let dark = measured(&solid(32, 0x00));
    let bright = measured(&solid(32, 0xff));

    assert!(
        scrim_alpha(dark.luma.unwrap_or(f64::NAN)) < scrim_alpha(bright.luma.unwrap_or(f64::NAN)),
        "the two measurements have to differ, or the tiers prove nothing"
    );

    let dark = dark.solve(&theme, BackdropKind::Aurora);
    let bright = bright.solve(&theme, BackdropKind::Aurora);
    assert_eq!(
        (dark.chrome, dark.text, dark.muted, dark.floor_start),
        (bright.chrome, bright.text, bright.muted, bright.floor_start),
        "a black and a white cover must give one tier set on the aurora"
    );
}

// --- gradient_luma ----------------------------------------------------------

#[test]
fn gradient_luma_of_one_repeated_stop_is_that_stop() {
    for rgb in [
        0x0000_0000,
        0x0080_8080,
        0x00ff_ffff,
        0x00cb_a6f7,
        0x0000_66ff,
    ] {
        let stop = rgb_lstar(rgb);
        let gradient = gradient_luma(rgb, rgb);
        assert!(
            (gradient - stop).abs() < 1e-9,
            "a gradient between {rgb:#08x} and itself should measure L*{stop}, got L*{gradient}"
        );
    }
}

/// Averaging in linear Y rather than in L\* is the whole point of
/// `gradient_luma_lstar` — this is the case that distinguishes them. A plain
/// L\* midpoint understates a dark-to-bright gradient, which is exactly the
/// one the scrim has to get right.
#[test]
fn gradient_luma_outranks_the_plain_lstar_midpoint() {
    let (dark, bright) = (0x0011_1111, 0x00ee_eeee);
    let midpoint = f64::midpoint(rgb_lstar(dark), rgb_lstar(bright));
    let gradient = gradient_luma(dark, bright);
    assert!(
        gradient > midpoint,
        "linear-Y average L*{gradient} should sit above the L* midpoint L*{midpoint}"
    );
}

/// Round-trip: [`floor_luma`] is a *constant* standing in for a measurement,
/// so it has to describe the gradient the solve actually paints. If the floor
/// tones and the constant ever drift, the artwork-less path solves its scrim
/// against a backdrop that isn't on screen.
#[test]
fn floor_luma_matches_the_gradient_the_solve_paints() {
    for seed in [SEED, 0x00ff_ffff, 0x0000_0000, 0x0000_66ff] {
        let colors = solve(seed, 100.0);
        let painted = gradient_luma(colors.floor_start, colors.floor_end);
        let claimed = floor_luma();
        assert!(
            (painted - claimed).abs() < 2.0,
            "seed {seed:#08x}: floor paints L*{painted} but floor_luma() claims L*{claimed}"
        );
    }
}

// --- the hero's two pre-solve text defaults ---------------------------------

/// The hero's two text tiers are solved per artwork, exactly as `np-*` is — but
/// the solve arrives a frame or two after the band first paints, and these
/// literals are what fills that gap. They are declared as *defaults* on
/// `in-out` properties rather than as constants, so nothing about them is
/// enforced by the solve; this is the only thing holding them to the same bar
/// their solved successors clear.
const HERO_ON_BACKDROP: u32 = 0x00f0_eef5;
const HERO_ON_BACKDROP_MUTED: u32 = 0x00c9_c5d3;

/// Brightest tone the composite can present, swept across every backdrop the
/// solve can be handed. Dark covers land *below* the target (more headroom),
/// so the maximum is the only case worth asserting against.
fn worst_composited_tone() -> f64 {
    (0..=100)
        .map(f64::from)
        .map(|luma| composited_tone(luma, scrim_alpha(luma)))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The frame before the solve lands is a real frame, and on the worst backdrop
/// the solve can be handed it has to be readable too.
#[test]
fn the_pre_solve_hero_text_defaults_clear_their_targets_on_the_worst_backdrop() {
    let tone = worst_composited_tone();
    let title = ratio_against_tone(HERO_ON_BACKDROP, tone);
    let muted = ratio_against_tone(HERO_ON_BACKDROP_MUTED, tone);
    assert!(title >= 4.5, "hero title is {title}:1 on the worst backdrop (L*{tone}), owes 4.5:1");
    assert!(muted >= 3.0, "hero meta line is {muted}:1 on the worst backdrop (L*{tone}), owes 3:1");
    assert!(title > muted, "the title must outrank the meta line, got {title}:1 vs {muted}:1");
}

/// The constants above are copies — assert they still match the declarations
/// they mirror, so editing one side can't silently invalidate the contrast test.
///
/// **Spell the whole `in-out` prefix.** `"out property"` is a substring of
/// `"in-out property"`, so the laxer match this replaced went on passing after
/// the tiers stopped being `out` — it asserted nothing about the half of the
/// declaration it named. Reverting them to `out` is caught by the compiler
/// anyway (Slint emits no setter, so `hero_backdrop::write` stops building),
/// which is why the literals are what this test is really for.
#[test]
fn the_hero_text_defaults_match_hero_backdrop_slint() {
    let declarations = include_str!("../../../melodia-ui/ui/globals/hero-backdrop.slint");
    for (name, literal) in [("on-backdrop", "#f0eef5"), ("on-backdrop-muted", "#c9c5d3")] {
        let declaration = format!("in-out property <brush> {name}: {literal};");
        assert!(
            declarations.contains(&declaration),
            "hero-backdrop.slint no longer declares `{declaration}` — if the tier went back to \
             `out`, `hero_backdrop::write` can no longer publish it and the band is stuck on \
             this default; if the literal moved, update the constant here too"
        );
    }
}

/// One fallback set, spelled in three places, and all three have to agree.
///
/// The two globals are the tiers Rust publishes into and `AuroraBackdrop`'s inputs are what
/// a mount that forgot one would paint — so a drift here shows only on the surface that
/// hasn't been solved yet, which is the first frame of a cold open and nothing else.
#[test]
fn the_tint_defaults_agree_across_both_tiers_and_the_component() {
    const TINTS: [&str; WASH_COUNT] = ["#3a2d4a", "#2d3a4a", "#4a2d3a"];

    for (file, source, prefix, kind) in [
        (
            "globals/player.slint",
            include_str!("../../../melodia-ui/ui/globals/player.slint"),
            "np-tint",
            "in-out",
        ),
        (
            "globals/hero-backdrop.slint",
            include_str!("../../../melodia-ui/ui/globals/hero-backdrop.slint"),
            "tint",
            "in-out",
        ),
        (
            "components/aurora-backdrop.slint",
            include_str!("../../../melodia-ui/ui/components/aurora-backdrop.slint"),
            "tint",
            "in",
        ),
    ] {
        for (index, literal) in TINTS.iter().enumerate() {
            let declaration = format!("{kind} property <color> {prefix}-{}: {literal};", index + 1);
            assert!(
                source.contains(&declaration),
                "{file} no longer declares `{declaration}` — the three copies are one fallback \
                 set, and a drift paints a different backdrop on whichever surface hasn't been \
                 solved yet"
            );
        }
    }
}
