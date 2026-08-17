//! Source-level pins on `components/aurora-backdrop.slint`.
//!
//! Nothing pinned here shows on screen until an edge case arrives, and each was paid for once:
//! the count has to be fixed or `Brush::interpolate` crosses mismatched gradients, a ramp ending
//! on the `transparent` keyword darkens instead of thinning, the dither's `image-fit` decides
//! whether it dithers or mottles, a mount reading a global directly is what stops one component
//! serving both backdrop tiers, and the blob geometry is what `ui::aurora`'s coverage — and so
//! `wash_cap`, and so how bright the surface is allowed to get — is derived from.

// Comments dropped: every anchor here is a gradient literal or a geometry binding, and the prose
// above them argues about gradients, stop counts, corners and `transparent`.
use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, binding_value, normalize_ws as normalized,
    strip_line_comments as code, stripped_sources,
};
use crate::ui::aurora::{BLOB_PEAKS, REACH_FRACTION};
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

/// Slint and Rust state the same geometry, `ui::aurora`'s coverage figures being a closed form over
/// exactly these numbers and `wash_cap` a closed form over those. Moving one here is a change with
/// no symptom on screen: the foreground stays legible right up until it isn't.
#[test]
fn the_coverage_constants_describe_the_geometry_below() {
    let src = code(AURORA);
    let reach = normalized(binding_value(&src, "blob-reach:"));
    let fraction =
        reach.strip_prefix("root.host-diagonal * ").and_then(|number| number.parse::<f64>().ok());
    // Loose enough for however many places the `.slint` spells 1/√2 to, tight enough that another
    // fraction is another geometry.
    assert!(
        fraction.is_some_and(|f| (f - REACH_FRACTION).abs() < 1e-3),
        "`blob-reach` is `{reach}` — re-derive `ui::aurora`'s coverage before changing it here"
    );

    // The side of a square whose gradient reaches zero at `blob-reach`; the ramp and the rect are
    // one number apart, so a stop moved without it silently rescales every wash.
    assert!(code(AURORA).contains("blob-span: root.blob-reach / 0.495;"));

    for peak in BLOB_PEAKS {
        let binding = format!("peak: {peak};");
        assert!(code(AURORA).contains(&binding), "no wash carries `{binding}`");
    }
}

/// The washes anchor at the four corners, walking round the rectangle.
///
/// Fractions of each axis put all four centres *inside* the element with the reach floored on the
/// short one, so every ramp covered every pixel: four vivid washes averaged to one flat tone, on a
/// banner and on Now Playing alike. From a corner each wash owns the region nearest it and meets
/// only its two edge neighbours — which is the adjacency `around_the_wheel` hands them, four
/// corners being a cycle exactly as the hue wheel is.
#[test]
fn the_washes_are_anchored_at_the_corners() {
    const LEFT: &str = "x: -root.blob-span / 2;";
    const RIGHT: &str = "x: root.width - root.blob-span / 2;";
    const TOP: &str = "y: -root.blob-span / 2;";
    const BOTTOM: &str = "y: root.height - root.blob-span / 2;";

    let src = normalized(&code(AURORA));
    for gone in ["blob-x(", "blob-y(", "long-side", "short-side"] {
        assert!(
            !src.contains(gone),
            "`{gone}` is back, so the centres are inside the element again"
        );
    }

    // Walking round rather than down a list: two washes at one corner is two colours nothing
    // separates, and a diagonal pair as neighbours is the grey composite the ordering exists to
    // avoid.
    for (across, down) in [(LEFT, TOP), (RIGHT, TOP), (RIGHT, BOTTOM), (LEFT, BOTTOM)] {
        let anchored = format!("{across} {down}");
        assert_eq!(
            src.matches(&anchored).count(),
            1,
            "no wash anchors at `{anchored}`, so the four don't take a corner each"
        );
    }
}

/// Nothing darkens the periphery.
///
/// A vignette sat over this stack while the model was a self-contained dark surface, where the only
/// question was how to frame a bright middle. Under washes anchored at the corners it is the direct
/// inverse: the corners are where the album's colour now is, and neutral black over them is what the
/// backdrop was dull for. Deleted rather than defaulted off, a property nothing sets being a layer
/// waiting to be switched back on.
#[test]
fn no_layer_darkens_the_periphery() {
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        assert!(!src.contains("vignette"), "{path} paints a vignette over an aurora corner");
    }
}

/// Nothing in Slint writes the backdrop choice.
///
/// The `in` qualifier on `Theme.aurora-backdrop` already makes a write fail to compile, so what
/// this holds is the *reason*: the two arms publish unrelated tiers, so a live flip leaves one
/// stack painting the other's colours, and the tiers decide at construction whether to build a
/// blurred half at all. A `Ctrl+Shift+B` arm did exactly that while both models were being
/// judged; the pin outlives a later loosening of the qualifier.
#[test]
fn nothing_in_slint_writes_the_backdrop_choice() {
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        assert!(
            !src.contains("Theme.aurora-backdrop ="),
            "{path} writes `Theme.aurora-backdrop`, which the two tiers already read once at boot"
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
