//! The shared resize primitive, and the pin that it stays the only one.

use std::collections::BTreeSet;

use super::*;
use crate::test_support::{SRC_DIR, stripped_sources};

/// A floor, so a walk that silently found nothing can't pass vacuously.
const MIN_SOURCES: usize = 200;

/// `image`'s own resamplers, by the spelling a call site uses.
///
/// The method forms carry their leading dot so `resize_exact` in prose or in a path can't match,
/// and `.resize(` is deliberately absent — `LruCache::resize` and `CoverThumbs::resize` share the
/// name and neither touches a pixel.
const IMAGE_RESAMPLERS: [&str; 6] = [
    ".thumbnail(",
    ".thumbnail_exact(",
    ".resize_exact(",
    ".resize_to_fill(",
    "imageops::resize(",
    "imageops::thumbnail(",
];

/// The one file allowed to keep calling them.
///
/// `extract_source_argb` downscales to 64 px to seed a *palette*, so changing its filter moves
/// every generated theme colour. It is also the cold half of that path — the live one reads a
/// `CoverThumbs` buffer that is already through `resize_rgb8`.
const EXEMPT: &str = "services/material_you.rs";

/// This file, which spells every needle in [`IMAGE_RESAMPLERS`] and would otherwise be its own
/// first hit. Skipped rather than forgiven, and held to still naming one — a pin that stopped
/// spelling its needles has stopped checking anything.
const PIN: &str = "media/tests/image_decode_tests.rs";

/// **An equality, not a floor.** A new site reaching for `image`'s own resampler compiles clean
/// and is invisible in review, while costing the per-cover milliseconds this module exists to
/// remove — and, at a tier, silently enlarging a cover smaller than the tile. A containment check
/// is exactly the check that new site walks past.
#[test]
fn resize_rgb8_is_the_only_resampler_outside_the_palette_seed() {
    let mut pin_seen = false;
    let mut found = BTreeSet::new();

    for (path, code) in stripped_sources(SRC_DIR, "rs", MIN_SOURCES) {
        if !IMAGE_RESAMPLERS.iter().any(|needle| code.contains(needle)) {
            continue;
        }
        if path == PIN {
            pin_seen = true;
            continue;
        }
        found.insert(path);
    }

    assert!(pin_seen, "{PIN} no longer names the resamplers it walks for, so it checks nothing");
    let expected: BTreeSet<String> = [EXEMPT.to_owned()].into_iter().collect();
    assert_eq!(
        found, expected,
        "the set of files calling `image`'s own resamplers has moved. An *extra* entry is a call \
         site that should go through `image_decode::resize_rgb8`; a *missing* one means the \
         exemption was removed or renamed and this pin is no longer checking anything."
    );
}

// ── fit_within ──

/// The rule every tier depends on: a cover smaller than the tile is drawn at the tile's size
/// either way, so padding the buffer out to it buys nothing but memory.
#[test]
fn a_rectangle_already_inside_the_bound_is_returned_untouched() {
    assert_eq!(fit_within(128, 128, 448, 448), (128, 128));
    assert_eq!(fit_within(1, 1, 512, 512), (1, 1));
    assert_eq!(fit_within(448, 448, 448, 448), (448, 448));
}

/// One ratio for both axes, so the rectangle's own shape survives. The backdrop band relies on
/// this: clamping the axes independently would make how much it squashes depend on the cover.
#[test]
fn an_oversized_rectangle_shrinks_by_a_single_ratio() {
    assert_eq!(fit_within(1024, 1024, 512, 512), (512, 512));
    assert_eq!(fit_within(1000, 500, 500, 500), (500, 250));
    assert_eq!(fit_within(500, 1000, 500, 500), (250, 500));

    // A landscape target into a square source — the backdrop band's own case, where the *target*
    // is what shrinks.
    let (width, height) = fit_within(128, 85, 120, 120);
    assert!(width <= 120 && height <= 120);
    let target_ratio = f64::from(128) / f64::from(85);
    let fitted_ratio = f64::from(width) / f64::from(height);
    assert!(
        (target_ratio - fitted_ratio).abs() < 0.05,
        "the band's aspect must survive the fit: {target_ratio} vs {fitted_ratio}"
    );
}

/// Integer division must not round an axis away entirely — a zero-width target is a resize the
/// resampler refuses, which would read as an undecodable cover.
#[test]
fn an_extreme_ratio_never_rounds_an_axis_to_zero() {
    let (width, height) = fit_within(4000, 3, 512, 512);
    assert!(width > 0 && height > 0, "got {width}x{height}");
    assert!(width <= 512 && height <= 512);
}
