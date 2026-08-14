//! Source pins for the brand mark in `components/custom-titlebar.slint`.
//!
//! The mark is the one element in the window whose colour used to come from an
//! asset rather than from the palette, and every pin here guards a line whose
//! removal is invisible on the default theme: a reviewer running Mocha sees the
//! same mark either way, and only a light palette shows the ~1.2:1 wash-out.

use crate::test_support::strip_line_comments;

const TITLEBAR: &str = include_str!("../../../melodia-ui/ui/components/custom-titlebar.slint");
const THEME: &str = include_str!("../../../melodia-ui/ui/theme.slint");
const LOGO: &str = include_str!("../../../melodia-ui/ui/assets/icons/logo-without-background.svg");

/// The bar declares exactly one `Image`, so its body needs no disambiguation.
fn logo_mount() -> String {
    let code = strip_line_comments(TITLEBAR);
    let rest = code.split_once("Image {").map_or("", |(_, rest)| rest);
    assert!(
        !rest.is_empty(),
        "`custom-titlebar.slint` no longer mounts an `Image` — every assertion over that body \
         would walk an empty string and pass"
    );

    let mut depth = 1usize;
    let mut body: Vec<&str> = Vec::new();
    for line in rest.lines() {
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if depth == 0 {
            break;
        }
        body.push(line);
    }
    body.join("\n")
}

/// `Theme.brand-mark`'s binding, whitespace-normalized onto one line.
fn brand_mark_binding() -> String {
    let code = strip_line_comments(THEME);
    let rest = code.split_once("brand-mark:").map_or("", |(_, rest)| rest);
    let binding = rest.split_once(';').map_or("", |(binding, _)| binding);
    assert!(
        !binding.is_empty(),
        "`theme.slint` no longer declares a terminated `brand-mark` binding — every assertion \
         over it would read an empty string and pass"
    );
    binding.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stops the asset paints itself with, lowercased for comparison.
fn asset_stops() -> Vec<String> {
    const NEEDLE: &str = "stop-color=\"";
    LOGO.match_indices(NEEDLE)
        .filter_map(|(at, _)| LOGO[at + NEEDLE.len()..].split_once('"'))
        .map(|(hex, _)| hex.to_ascii_lowercase())
        .collect()
}

/// The mark is recoloured, and from the theme rather than from a sibling asset.
///
/// `colorize` composites a brush through the rasterised SVG's alpha, so a
/// gradient survives it — which is what makes one asset enough. Drop the line and
/// the mark falls back to the file's Mocha pastels: invisible on every light
/// palette, and invisible to a reviewer on a dark one.
#[test]
fn the_titlebar_mark_takes_its_colour_from_the_theme() {
    let mount = logo_mount();
    assert!(
        mount.contains("logo-without-background.svg"),
        "the bar's only `Image` is no longer the logo — this pin is reading the wrong element"
    );
    assert!(
        mount.contains("colorize: Theme.brand-mark"),
        "the logo must be colorized from `Theme.brand-mark`: uncoloured it paints the asset's \
         Mocha stops, which land near 1.2:1 on a light palette"
    );
}

/// The dark arm is the asset's own pair, so the two can't drift.
///
/// The SVG still feeds the tray raster, the launcher icon and
/// `scripts/gen-discord-assets.sh`, none of which can read a Slint theme — so
/// recolouring the mark for *them* is exactly the edit that leaves this copy
/// behind.
#[test]
fn the_marks_dark_arm_still_matches_the_asset() {
    let stops = asset_stops();
    assert_eq!(
        stops.len(),
        2,
        "the logo asset no longer paints two stops — `brand-mark` states a two-stop gradient \
         and would need restating"
    );

    let dark_arm = brand_mark_binding()
        .rsplit_once(" : ")
        .map_or(String::new(), |(_, arm)| arm.to_ascii_lowercase());
    assert!(
        !dark_arm.is_empty(),
        "`brand-mark` is no longer a ternary — a light palette needs an arm of its own"
    );

    for stop in &stops {
        assert!(
            dark_arm.contains(stop.as_str()),
            "`brand-mark`'s dark arm dropped the asset's `{stop}` — the in-app mark and the \
             packaged one must paint the same colours on a dark palette"
        );
    }
}

/// The polarity test measures the surface the bar actually paints.
///
/// `base` is the tempting read, and it agrees with `mantle` on polarity under
/// every palette shipped today — which is what would let a swap here survive
/// review and then be wrong for whichever generated palette straddles the
/// threshold.
#[test]
fn the_mark_measures_the_surface_the_bar_paints() {
    assert!(
        brand_mark_binding().contains("root.is-light(root.mantle)"),
        "`brand-mark` must branch on `mantle`'s luma — that is the brush the bar's root \
         paints, so anything else measures a surface the mark never sits on"
    );
    assert!(
        strip_line_comments(TITLEBAR).contains("background: Theme.mantle"),
        "the bar no longer paints `Theme.mantle`, so `brand-mark` measures the wrong surface \
         — move both together"
    );
}
