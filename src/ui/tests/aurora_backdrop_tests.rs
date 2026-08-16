//! Source-level pins on `components/aurora-backdrop.slint`.
//!
//! Nothing pinned here shows on screen until an edge case arrives, and each was paid for once:
//! the count has to be fixed or `Brush::interpolate` crosses mismatched gradients, a ramp ending
//! on the `transparent` keyword darkens instead of thinning, the dither's `image-fit` decides
//! whether it dithers or mottles, a mount reading a global directly is what stops one component
//! serving both backdrop tiers, and the blob geometry is what `ui::aurora::PEAK_TONE` is derived
//! from.

// Comments dropped: every anchor here is a gradient literal, and the prose above them argues
// about gradients, stop counts and `transparent`.
use crate::test_support::strip_line_comments as code;
use crate::ui::backdrop::SEED_COUNT;

const AURORA: &str = include_str!("../../../melodia-ui/ui/components/aurora-backdrop.slint");

/// The blob count is fixed, and fixed at the number Rust solves for.
///
/// `Brush::interpolate` blends gradient→gradient only at a matching stop *and* element count;
/// anything else flattens through a solid colour halfway through the fade. A fourth blob added in
/// Slint without a fourth seed would also paint whatever the uninitialised property holds.
#[test]
fn slint_paints_exactly_the_tints_rust_solves() {
    let mounts = code(AURORA).matches("AuroraBlob {").count();
    assert_eq!(
        mounts, SEED_COUNT,
        "{mounts} blob mounts against {SEED_COUNT} seeds — the two counts are one contract"
    );

    for tint in 1..=SEED_COUNT {
        let property = format!("tint-{tint}");
        assert!(
            code(AURORA).contains(&format!("in property <color> {property}")),
            "no `{property}` input, so one mount is painting a default"
        );
    }
}

/// No ramp ends on the `transparent` keyword.
///
/// `FemtoVG` interpolates stops in straight RGBA, so fading to rgba(0,0,0,0) drags red, green and
/// blue toward black on the way: the layer darkens across its own length instead of thinning, and
/// three of those at three angles read as wedges painted onto the surface. Ending on the same
/// colour at zero alpha keeps RGB flat and lets alpha do the work — which is what Amberol's
/// `color-mix(in srgb, <colour> 0%, transparent)` spells out the long way.
///
/// The base gradient's own gated arm is the exception and stays legible as one: both its stops are
/// `transparent`, so there is no colour to drag anywhere.
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

/// The dither tile is mapped one texel to one physical pixel.
///
/// Outside a layout `image-fit` defaults to `fill`, which scales the tile to the whole element
/// *before* tiling it — one texel becomes a block tens of pixels across and the noise draws as
/// visible mottling rather than disappearing. `preserve` is the only mode that keeps the source
/// pitch, and `pixelated` stops filtering averaging the tile back toward the flat colour it is
/// there to break up.
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

/// The numbers `ui::aurora::PEAK_TONE` was derived from — how far the ramps carry, how far apart
/// their centres sit, and how much alpha meets at the strongest point. Move any of them and the
/// peak has to be re-derived, which is a change with no symptom on screen: the foreground stays
/// legible right up until it isn't.
#[test]
fn the_peak_tone_is_derived_from_the_geometry_below() {
    for binding in [
        "blob-reach: max(root.long-side * 0.315, root.short-side * 0.8);",
        "blob-span: root.blob-reach / 0.495;",
        "peak: 0.5;",
        "peak: 0.46;",
        "peak: 0.42;",
        "peak: 0.38;",
    ] {
        assert!(
            code(AURORA).contains(binding),
            "`{binding}` moved — re-derive `ui::aurora::PEAK_TONE` before changing it here"
        );
    }
}

/// The washes are laid out as fractions of each axis, never of the diagonal.
///
/// A diagonal-derived layout is right on a square host and collapses on a wide one: the centres
/// converge horizontally and land off-element vertically, so only two of the four reach any pixel
/// and each covers the whole surface. It painted a banner flat mauve out of four vivid washes while
/// looking perfectly right on Now Playing — the shape the numbers were tuned against.
#[test]
fn the_washes_are_laid_out_against_the_axes_rather_than_the_diagonal() {
    assert!(
        !code(AURORA).contains("diagonal"),
        "the layout went back to the diagonal — it hides the failure on the shape it was tuned on"
    );
    // A pair sharing an along-fraction is two centres in one column, i.e. the same collapse
    // wearing different arithmetic.
    for along in ["0.15", "0.383", "0.617", "0.85"] {
        assert_eq!(
            code(AURORA).matches(&format!("blob-x({along},")).count(),
            1,
            "no wash sits at {along} along the long axis, so the four don't span it"
        );
    }
}

/// Every colour arrives as an input, so one component can serve both backdrop tiers.
///
/// Now Playing reads `Player.np-*` and the two bands read `HeroBackdrop.*`; a component naming
/// either could only ever mount on one, which is exactly why the blur it replaces had to be
/// spelled inline a second time. A `Theme.*` brush would be worse than wrong — theme brushes flip
/// polarity with the variant, so one album would paint a dark surface under one and a near-white
/// one under another while the foreground stayed solved for dark.
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
