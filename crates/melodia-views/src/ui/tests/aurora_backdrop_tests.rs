//! Source-level pins on `components/aurora-backdrop.slint`.
//!
//! Nothing pinned here shows on screen until an edge case arrives, and each was paid for once: a
//! varying stop count crosses mismatched gradients in `Brush::interpolate`, a ramp ending on the
//! `transparent` keyword darkens instead of thinning, the dither's `image-fit` decides whether it
//! dithers or mottles, a mount reading a global directly stops one component serving both backdrop
//! tiers, and the three headings are what leave a corner showing the floor.

// Comments stripped: every anchor is a gradient literal or a geometry binding, and the prose above
// them names the same tokens.
use crate::ui::aurora::WASH_COUNT;
use melodia_testkit::{normalize_ws as normalized, strip_line_comments as code};

const AURORA: &str = include_str!("../../../../melodia-ui/ui/components/aurora-backdrop.slint");

/// The wash count is fixed, and fixed at the number Rust solves for. `Brush::interpolate` blends
/// gradient→gradient only at a matching stop *and* element count; anything else flattens through a
/// solid colour halfway. A fourth sweep with no fourth wash would paint an uninitialised property.
#[test]
fn slint_paints_exactly_the_tints_rust_solves() {
    let mounts = code(AURORA).matches("AuroraSweep {").count();
    assert_eq!(
        mounts, WASH_COUNT,
        "{mounts} sweep mounts against {WASH_COUNT} washes — the two counts are one contract"
    );

    for tint in 1..=WASH_COUNT {
        let property = format!("tint-{tint}");
        assert!(
            code(AURORA).contains(&format!("in property <color> {property}")),
            "no `{property}` input, so one mount is painting a default"
        );
    }
}

/// No ramp ends on the `transparent` keyword. `FemtoVG` interpolates stops in straight RGBA, so
/// fading to rgba(0,0,0,0) drags red, green and blue toward black on the way: the layer darkens
/// across its own length instead of thinning, and three of those read as wedges painted on. Ending
/// on the same colour at zero alpha keeps RGB flat and lets alpha do the work. The base gradient's
/// gated arm is the exception — both its stops are `transparent`, so there is no colour to drag.
#[test]
fn no_layer_fades_through_black() {
    for (line_number, line) in code(AURORA).lines().enumerate() {
        let is_ramp =
            line.contains("@radial-gradient") || line.contains("root.tint.transparentize");
        if !is_ramp || !line.contains("transparent,") && !line.contains("transparent)") {
            continue;
        }
        assert!(
            line.contains("transparent, transparent"),
            "line {} fades a colour to the `transparent` keyword: {}",
            line_number + 1,
            line.trim()
        );
    }
}

/// The dither tile is mapped one texel to one physical pixel. Outside a layout `image-fit` defaults
/// to `fill`, which scales the tile to the whole element *before* tiling it — one texel becomes a
/// block tens of pixels across and the noise draws as mottling. `preserve` is the only mode keeping
/// the source pitch, and `pixelated` stops filtering averaging it back toward the flat colour.
#[test]
fn the_dither_keeps_its_own_pitch() {
    for binding in [
        "image-fit: ImageFit.preserve;",
        "image-rendering: ImageRendering.pixelated;",
        "horizontal-tiling: ImageTiling.repeat;",
        "vertical-tiling: ImageTiling.repeat;",
    ] {
        assert!(code(AURORA).contains(binding), "the dither dropped `{binding}`");
    }
}

/// The three sweeps keep their headings, and they are three rather than four. **The asymmetry is the
/// effect**: near-orthogonal headings leave each sweep owning an edge and one corner showing the
/// floor, where a fourth or three evenly spaced average back to the one flat tone this replaced.
#[test]
fn the_sweeps_keep_their_headings() {
    let src = normalized(&code(AURORA));

    for heading in ["heading: 127deg;", "heading: 217deg;", "heading: 336deg;"] {
        assert_eq!(
            src.matches(heading).count(),
            1,
            "no sweep carries `{heading}` — the near-orthogonal set is what leaves a corner bare"
        );
    }
    assert_eq!(
        code(AURORA).matches("AuroraSweep {").count(),
        WASH_COUNT,
        "the mount count left `ui::aurora::WASH_COUNT`, so Rust is solving washes nothing paints"
    );
}

/// Each ramp is 55% at its near edge and gone by 70.71% of the gradient line. `transparentize`
/// *multiplies*, so 0.45 leaves a measured wash at 55% while a synthesized one keeps the lower
/// weight `ui::aurora` gave it — `with-alpha` would wash a guess on as hard as a fact. The far stop
/// is 1/√2, where a 45° traversal reaches the opposite corner, so nothing bands at the edges.
#[test]
fn each_sweep_fades_between_the_same_two_stops() {
    let src = normalized(&code(AURORA));
    assert!(
        src.contains("root.tint.transparentize(0.45) 0%"),
        "the near edge left 55%, so a measured wash lands at a weight the set wasn't tuned for"
    );
    assert_eq!(
        src.matches("root.tint.transparentize(1.0) 70.71%").count(),
        2,
        "both arms owe the same far stop, or the shown/hidden pair cross through a solid"
    );
}

/// Every colour arrives as an input, so one component can serve both backdrop tiers. Now Playing
/// reads `Player.np-*` and the two bands read `HeroBackdrop.*`; a component naming either could only
/// mount on one, which is why the blur it replaces had to be spelled inline a second time. A
/// `Theme.*` brush flips polarity with the variant, painting one album near-white under a light theme
/// while the foreground stays solved for dark.
#[test]
fn the_backdrop_names_no_global() {
    for global in [
        "Player.",
        "HeroBackdrop.",
        "Theme.base",
        "Theme.text",
        "Theme.accent",
    ] {
        assert!(
            !code(AURORA).contains(global),
            "`{global}` reached into the shared backdrop — it can then serve only that tier"
        );
    }
}
