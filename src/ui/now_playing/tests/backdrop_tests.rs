use material_colors::contrast::ratio_of_tones;
use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::ui::now_playing::backdrop::{
    BackdropColors, chrome_tone, composited_tone, floor_luma, luma_p90, muted_tone, scrim_alpha,
    solve, text_tone,
};

/// A Catppuccin-Mocha-ish mauve, the default accent — a realistic seed for the
/// solve tests below.
const SEED: u32 = 0x00cb_a6f7;

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
    luma_p90(buf).unwrap_or(f64::NAN)
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

// --- luma_p90 ---------------------------------------------------------------

#[test]
fn luma_p90_rejects_an_empty_buffer() {
    let buf = SharedPixelBuffer::<Rgb8Pixel>::new(0, 0);
    assert_eq!(luma_p90(&buf), None);
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
        if y < side / 5 { [255, 255, 255] } else { [0, 0, 0] }
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
        if y == 0 && x < side / 2 { [255, 255, 255] } else { [0, 0, 0] }
    });
    let luma = p90(&buf);
    assert!(
        luma < 10.0,
        "a 1% speck must not drive the scrim, got L*{luma}"
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
        assert!(
            (a - 0.30).abs() < f32::EPSILON,
            "L*{luma} should take the floor, got {a}"
        );
    }
}

#[test]
fn scrim_alpha_is_monotone_in_backdrop_luma() {
    let mut previous = scrim_alpha(0.0);
    for step in 1..=100 {
        let a = scrim_alpha(f64::from(step));
        assert!(
            a >= previous,
            "alpha dropped from {previous} to {a} at L*{step}"
        );
        previous = a;
    }
}

#[test]
fn scrim_alpha_is_snapped_to_whole_percents() {
    for step in 0..=100 {
        let a = scrim_alpha(f64::from(step));
        let percents = a * 100.0;
        assert!(
            (percents - percents.round()).abs() < 1e-3,
            "L*{step} produced an unsnapped {a}"
        );
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
        assert!(
            tone <= 33.0,
            "L*{luma} composited to L*{tone}, above the target band"
        );
    }
}

#[test]
fn composited_tone_never_brightens_the_backdrop() {
    for step in 0..=100 {
        let luma = f64::from(step);
        let tone = composited_tone(luma, scrim_alpha(luma));
        // The scrim is near-black, so compositing can only darken — except at
        // the very bottom, where the scrim is the lighter of the two.
        assert!(
            tone <= luma.max(9.0),
            "L*{luma} composited *up* to L*{tone}"
        );
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
    assert!(
        (a - 0.30).abs() < f32::EPSILON,
        "the floor should take the minimum scrim, got {a}"
    );
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
            let (r, g, b) = (
                ((rgb >> 16) & 0xff) as u8,
                ((rgb >> 8) & 0xff) as u8,
                (rgb & 0xff) as u8,
            );
            let y = relative_luminance(r, g, b);
            assert!(
                y < 0.06,
                "{name} for seed {seed:#08x} is not dark (Y={y}), rgb={rgb:#08x}"
            );
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
