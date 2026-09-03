//! `image_decode::resize_rgb8` is the tree's only resampler, bar the palette seed.
//!
//! A new site reaching for `image`'s own compiles clean and is invisible in review, while
//! costing the per-cover milliseconds that module exists to remove — and, at a tier, silently
//! enlarging a cover smaller than the tile.

use std::collections::BTreeSet;

use melodia_testkit::rust_sources;

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
const EXEMPT: &str = "media/image/material_you.rs";

/// **An equality, not a floor.** A new site reaching for `image`'s own resampler compiles clean
/// and is invisible in review, while costing the per-cover milliseconds this module exists to
/// remove — and, at a tier, silently enlarging a cover smaller than the tile. A containment check
/// is exactly the check that new site walks past.
#[test]
fn resize_rgb8_is_the_only_resampler_outside_the_palette_seed() {
    let mut found = BTreeSet::new();

    for (path, code) in rust_sources() {
        if IMAGE_RESAMPLERS.iter().any(|needle| code.contains(needle)) {
            found.insert(path);
        }
    }

    let expected: BTreeSet<String> = [EXEMPT.to_owned()].into_iter().collect();
    assert_eq!(
        found, expected,
        "the set of files calling `image`'s own resamplers has moved. An *extra* entry is a call \
         site that should go through `image_decode::resize_rgb8`; a *missing* one means the \
         exemption was removed or renamed and this pin is no longer checking anything."
    );
}
