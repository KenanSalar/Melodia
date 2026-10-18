use std::time::{Duration, Instant};

use melodia_core::themes::Palette;

use super::{Fade, Painted, Shown, Source, blend, ease_in_out};

const FROM: u32 = 0x10_2030;
const TO: u32 = 0xa0_b0c0;

/// Eighteen slots that differ from one another, so a slot read from its neighbour shows. Real
/// palettes repeat slots (Material 3's dark one has `crust == base`).
fn palette(seed: u32) -> Palette {
    Palette {
        base: seed,
        mantle: seed + 1,
        crust: seed + 2,
        surface0: seed + 3,
        surface1: seed + 4,
        surface2: seed + 5,
        overlay0: seed + 6,
        overlay1: seed + 7,
        overlay2: seed + 8,
        text: seed + 9,
        subtext0: seed + 10,
        subtext1: seed + 11,
        border: seed + 12,
        card_control: seed + 13,
        card_control_hover: seed + 14,
        red: seed + 15,
        green: seed + 16,
        yellow: seed + 17,
    }
}

/// [`palette`] plus three more distinct slots, the ink set by hand rather than derived.
fn painted(seed: u32) -> Painted {
    Painted {
        palette: palette(seed),
        accent: seed + 18,
        accent_text: seed + 19,
        mantle_unfocused: seed + 20,
    }
}

/// `Painted` carries no `Debug`, so the assertions compare this instead.
fn slots(painted: &Painted) -> (Palette, u32, u32, u32) {
    (painted.palette, painted.accent, painted.accent_text, painted.mantle_unfocused)
}

fn landed(source: Source, target: Painted) -> Shown {
    Shown { target, source, current: target, fade: None }
}

fn fading(source: Source, from: Painted, target: Painted) -> Shown {
    let fade = Fade { from, started: Instant::now(), length: Duration::from_millis(250) };
    Shown { target, source, current: from, fade: Some(fade) }
}

/// A new cover under Material You is the one change the backdrops beside the window cross over,
/// so it is the one the window eases too. A pick, the first artwork palette and the drop back to
/// a theme all land at once.
#[test]
fn only_a_new_dynamic_palette_over_a_dynamic_one_eases() {
    let (shown, other) = (painted(FROM), painted(TO));
    let rows = [
        (Source::Static, Source::Static, other),
        (Source::Static, Source::Dynamic, other),
        (Source::Dynamic, Source::Static, other),
        (Source::Dynamic, Source::Dynamic, shown),
        (Source::Dynamic, Source::Dynamic, other),
    ];

    let eases = rows.map(|(was, now, target)| landed(was, shown).eases_into(&target, now));

    assert_eq!(eases, [false, false, false, false, true]);
}

/// Snapping to the palette already fading in would jump the fade to its end.
#[test]
fn a_repaint_of_the_fading_target_is_left_to_the_fade() {
    let (from, target) = (painted(FROM), painted(TO));
    let mid_fade = fading(Source::Dynamic, from, target);

    let answers = [
        mid_fade.is_fading_to(&target, Source::Dynamic),
        landed(Source::Dynamic, target).is_fading_to(&target, Source::Dynamic),
        mid_fade.is_fading_to(&from, Source::Dynamic),
        mid_fade.is_fading_to(&target, Source::Static),
    ];

    assert_eq!(answers, [true, false, false, false], "(target, landed, other target, static)");
}

#[test]
fn a_blend_lands_on_its_ends() {
    let (from, to) = (painted(FROM), painted(TO));

    let ends = (slots(&blend(&from, &to, 0.0)), slots(&blend(&from, &to, 1.0)));

    assert_eq!(ends, (slots(&from), slots(&to)), "(start, end)");
}

/// The ink flips at a luminance threshold, so one derived from the blended accent would jump
/// mid-fade where every other slot glides.
#[test]
fn the_accent_ink_glides_rather_than_flipping() {
    let pale = Painted::new(palette(FROM), 0x00f5_e0dc, FROM);
    let deep = Painted::new(palette(TO), 0x001e_1e2e, TO);
    assert_ne!(pale.accent_text, deep.accent_text, "the two accents must take different inks");

    let mid = blend(&pale, &deep, 0.5).accent_text;

    assert!(
        ![pale.accent_text, deep.accent_text].contains(&mid),
        "the ink read {mid:06x} halfway, one end's own, so it flipped rather than glided"
    );
}

/// The backdrops beside the window ease on Slint's `ease-in-out`, so the window has to land with
/// them. The expected values are `cubic-bezier(0.42, 0, 0.58, 1)` solved independently.
#[test]
fn the_fade_eases_on_slints_ease_in_out() {
    const TOLERANCE: f32 = 1e-4;
    const CURVE: [(f32, f32); 7] = [
        (0.0, 0.0),
        (0.1, 0.019_722),
        (0.25, 0.129_162),
        (0.5, 0.5),
        (0.75, 0.870_838),
        (0.9, 0.980_278),
        (1.0, 1.0),
    ];

    let missed: Vec<(f32, f32)> = CURVE
        .into_iter()
        .map(|(progress, want)| (progress, ease_in_out(progress), want))
        .filter(|(_, got, want)| (got - want).abs() > TOLERANCE)
        .map(|(progress, got, _)| (progress, got))
        .collect();

    assert!(missed.is_empty(), "off the curve at (progress, eased): {missed:?}");
}
