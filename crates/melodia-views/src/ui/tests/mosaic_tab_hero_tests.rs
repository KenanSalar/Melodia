//! Source-level pins on `crates/melodia-ui/ui/components/hero/mosaic-tab-hero.slint`.
//!
//! The banner Favorites and Recently Played share. Each pin below is a fix paid for once
//! and invisible in review, so they live beside it here rather than under either host.
//! The header row's own fixes are `tab_search_header_tests.rs`; what stays here is the
//! half only this file can answer — that the band still mounts that row, hands it hero
//! tiers, and forwards what its two pages read back.

use crate::test_support::{MIN_SLINT_SOURCES, UI_DIR, binding_value, stripped_sources};

const HERO: &str = include_str!("../../../../melodia-ui/ui/components/hero/mosaic-tab-hero.slint");

/// The value of a `name:` binding in the hero, up to its terminating `;`.
fn binding(name: &str) -> &'static str {
    binding_value(HERO, name)
}

/// A mirrored width only reaches the bar through `changed width`, and `changed` doesn't
/// fire when the first layout settles directly on the final value — which is every window
/// opened at its size. Without the mount timer the seed is never corrected and a roomy
/// window draws icon-only tabs until something resizes it.
///
/// The mirror is the band's; **what it is seeded to** is `TabSearchHeader.row-floor`,
/// pinned once for all three hosts.
#[test]
fn the_page_width_mirror_has_a_mount_seed() {
    assert!(
        HERO.contains("changed width => { self.page-w = self.width; }"),
        "the hero band must mirror its width imperatively — a live `root.width` read feeding a \
         child's size re-enters layout"
    );

    let timer = HERO
        .split_once("Timer {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(
        timer.contains("root.page-w = root.width"),
        "the hero band's mount Timer must re-run the `page-w` mirror — `changed` never fires for \
         a window born at its final size"
    );
}

/// `TabSearchHeader`'s four brushes default to `Theme.*` tokens, which is right for
/// Settings and wrong on a banner — and a mount that omits one still builds and still
/// looks correct in Settings, so nothing else catches it.
///
/// `active-color` is the one this exists for: it drives the selected label, its FILL=1 icon
/// *and* the underline from one input, so an omission is three surfaces at once, and a
/// theme accent has no contrast floor against a band whose hue rides the mosaic. Asserted
/// as "reads *some* `HeroBackdrop` tier" rather than pinning which, the tier being a design
/// call that may move where `Theme.*` is a bug at any tier.
///
/// **They reach the bar through eased mirrors**, and this band needs that more than the
/// morphing one: `apply_hero_blur` posts an `invoke_from_event_loop` of its own after the
/// atlas compose, so the title, tile and chips are on screen a hop before the colours. The
/// bar itself must not ease them — see `tab_bar_tests`.
#[test]
fn the_hero_tab_bar_takes_every_brush_from_the_backdrop() {
    let mount = HERO
        .split_once("header := TabSearchHeader {")
        .and_then(|(_, rest)| rest.split_once("tab-selected(i) =>"))
        .map_or("", |(body, _)| body);
    assert!(
        !mount.is_empty(),
        "the hero band no longer mounts `header := TabSearchHeader` ahead of its callbacks"
    );

    for (prop, mirror) in [
        ("label-color", "hero-label"),
        ("active-color", "hero-active"),
        ("hover-fill", "hero-hover"),
        ("divider-color", "hero-divider"),
    ] {
        assert!(
            mount.contains(&format!("{prop}: root.{mirror};")),
            "the hero band's header must take `{prop}` from the `{mirror}` mirror — a tier passed \
             straight through snaps when the mosaic's solve lands a hop after the band is drawn"
        );
        assert!(
            binding(&format!("property <brush> {mirror}:")).contains("HeroBackdrop."),
            "`{mirror}` must read a `HeroBackdrop` tier — omitting it falls back to the \
             component's `Theme.*` default, which is a theme value on a band that is no longer \
             theme-seeded"
        );
    }

    assert!(
        HERO.contains("animate hero-label, hero-active, hero-hover, hero-divider {"),
        "the four mirrors exist to be eased — unanimated they are the tiers again with two more \
         names"
    );
}

/// The band forwards everything the shared header publishes, under the names its two pages
/// already read. The row itself is pinned by `tab_search_header_tests`; what only this file
/// can check is that the forwards exist, a band that mounts the header and drops them
/// compiling, painting correctly, and silently losing the tab slide's direction and every
/// compact tooltip. Two-way aliases, so nothing here can be orphaned by a write.
#[test]
fn the_band_forwards_what_the_shared_row_publishes() {
    for prop in [
        "tab-enter-from",
        "tab-anim-armed",
        "tip-w",
        "tip-h",
        "tip-label",
        "tip-visible",
    ] {
        assert!(
            HERO.contains(&format!("{prop} <=> header.{prop};")),
            "the hero band must re-publish `{prop}` off the shared header — its pages read that \
             name"
        );
    }
    // The two positional anchors can't be plain aliases: they are relative to the header,
    // and a page's frame is relative to the band.
    for prop in ["tip-x", "tip-y"] {
        assert!(
            binding(&format!("out property <length> {prop}:")).contains(&format!("header.{prop}")),
            "the band's `{prop}` must offset the header's own by their `absolute-position` delta"
        );
    }
}

/// The banner's height is derived, not a literal, so growing the artwork or the bar can't
/// leave it cropping its own contents.
#[test]
fn the_hero_height_is_derived_from_what_it_stacks() {
    let height = binding("out property <length> hero-height:");
    assert!(
        height.contains("header.row-h") && height.contains("Theme.hero-artwork"),
        "`hero-height` must sum the header row's own height and the artwork tile — a literal \
         drifts silently the moment either grows"
    );
}

/// `CoverMosaic` lays a live 2×2 out in Slint, and this banner used to draw one — four lazy
/// per-tile lookups beside a second composition of the same covers for the backdrop. It draws one
/// composed collage now, and this stops that branch returning: the picker is the component's whole
/// remaining audience, and it wants the live form, its tiles following an uncomposed selection.
#[test]
fn the_cover_mosaic_is_the_pickers_alone() {
    let mounts: Vec<String> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(_, src)| src.contains("CoverMosaic {"))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        mounts,
        ["components/dialog/playlist-mosaic-picker.slint"],
        "only the playlist mosaic picker may mount `CoverMosaic` — a hero wanting a live \
         mosaic is a hero composing its covers twice"
    );
}
