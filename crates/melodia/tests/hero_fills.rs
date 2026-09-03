//! Two fill rules asked of every hero surface in the Slint tree.
//!
//! `with-alpha` replaces where `transparentize` multiplies, so a fill spelled that way discards
//! whatever alpha the tier underneath carries — on the aurora, the neutral ink's, and with it the
//! whole mechanism by which the wash reads through. It looks right on the blur arm, where the
//! tier is opaque and the two spellings agree.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// Nothing derived from the chrome tier may *set* its alpha. `with-alpha` replaces where
/// `transparentize` multiplies, so a fill spelled that way discards whatever alpha `chrome` carries
/// — on the aurora the neutral ink's, and the whole mechanism by which the wash reads through. It
/// looks right on the blur arm, where the tier is opaque and the two spellings agree. Every tier is
/// held, not just the chrome's: each carries an alpha Rust solved per arm.
#[test]
fn no_fill_derived_from_the_chrome_tier_sets_its_alpha() {
    const TIERS: [&str; 12] = [
        "chrome",
        "placeholder",
        "chrome-text",
        "chip-fill",
        "on-backdrop",
        "on-backdrop-muted",
        "np-accent-bright",
        "np-chrome-text",
        "np-chip-fill",
        "np-viz",
        "np-on-backdrop",
        "np-on-backdrop-muted",
    ];

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        for tier in TIERS {
            assert!(
                !src.contains(&format!("{tier}.with-alpha(")),
                "{path} sets alpha on `{tier}` — use `transparentize`, which multiplies, or the \
                 aurora's neutral tier is painted opaque and stops letting the wash through"
            );
        }
    }
}

/// Every coverless hero square paints from `placeholder`, and none of them from `chrome`. The two
/// tiers hold the same value on the blur, so reverting a mount builds, passes review and is wrong
/// only under a setting CI never turns on — where `chrome` is a neutral ink and the square becomes
/// a pale slab with a lamp on it. A per-file pair, fill and glyph being separate bindings.
#[test]
fn every_coverless_hero_square_paints_from_the_placeholder_tier() {
    const MOUNTS: [&str; 3] = [
        "components/hero/library-tab-band.slint",
        "components/hero/mosaic-hero-tile.slint",
        "views/my-library-view.slint",
    ];

    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);

    let mut seen = 0;
    for (path, code) in &sources {
        // The chrome tier is legitimate everywhere else in the tree — chips, discs, the
        // visualizer — so only the three squares are asked about.
        if !MOUNTS.contains(&path.as_str()) {
            continue;
        }
        seen += 1;
        assert!(
            code.contains("HeroBackdrop.placeholder.transparentize(0.85)"),
            "{path} lost the placeholder fill — `chrome` is a neutral ink on the aurora"
        );
        assert!(
            code.contains("HeroBackdrop.placeholder;"),
            "{path} lost the placeholder glyph — `chrome` is a neutral ink on the aurora"
        );
        assert!(
            !code.contains("HeroBackdrop.chrome.transparentize(0.85)"),
            "{path} still fills a coverless square from the chrome tier"
        );
    }
    assert_eq!(seen, MOUNTS.len(), "a mount was renamed and this pin stopped reading it");
}
