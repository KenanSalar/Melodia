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

/// The numbers `ui::aurora::PEAK_TONE` was derived from.
///
/// That constant is what the WCAG solve targets on this surface, and it is stated rather than
/// measured — there is no buffer to measure. Its derivation reads the span and offset (which fix
/// how far the ramps carry and how far apart their centres sit) and the four peaks (which fix how
/// much alpha meets at the strongest point). Move any of them and the peak has to be re-derived,
/// which is a change with no symptom on screen: the foreground stays legible right up until it
/// isn't.
#[test]
fn the_peak_tone_is_derived_from_the_geometry_below() {
    for binding in [
        "blob-span: root.diagonal * 1.3;",
        "blob-offset: root.diagonal * 0.35;",
        "blob-step: root.blob-offset * 0.707;",
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
