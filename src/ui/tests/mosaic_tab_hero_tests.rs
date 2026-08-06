//! Source-level pins on `melodia-ui/ui/components/hero/mosaic-tab-hero.slint`.
//!
//! The banner Favorites and Recently Played share. Each thing below is a fix that
//! was paid for once and is invisible in review — which is the whole reason the
//! band is one file rather than two — so they live beside it here rather than
//! under either host, the `tab_bar_tests.rs` arrangement for the same reason.
//!
//! **Five of these used to be a second copy of `library_tab_band_tests.rs`**, worded
//! the same way because the two sources were: the header row's own fixes. They are
//! `tab_search_header_tests.rs` now, once, and what stays here is the half only this
//! file can answer — that the band still mounts that row, hands it hero tiers, and
//! forwards what its two pages read back.

const HERO: &str = include_str!("../../../melodia-ui/ui/components/hero/mosaic-tab-hero.slint");

/// The value of a `name:` binding, up to its terminating `;`.
fn binding(name: &str) -> &'static str {
    HERO.split_once(name)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value)
}

/// A mirrored width only reaches the bar through `changed width`, and `changed`
/// doesn't fire when the first layout settles directly on the final value — which is
/// every window opened at its size. Without the mount timer the seed is never
/// corrected, and a roomy window draws icon-only tabs until something resizes it. It
/// only looks fixed coming out of the miniplayer, where the floor's answer happens to
/// be the right one.
///
/// The mirror is the band's; **what it is seeded to** is `TabSearchHeader.row-floor`,
/// pinned once for all three hosts by
/// `tab_search_header_tests::every_tabbed_page_mounts_the_shared_row`.
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

/// `TabSearchHeader`'s four brushes all default to `Theme.*` tokens, which is right
/// for Settings and wrong on a banner — and a mount that omits one still builds and
/// still looks correct in Settings, so nothing else catches it.
///
/// `active-color` is the one this exists for. It was left at the default long after
/// the other three moved, and it drives the selected label, its FILL=1 icon *and* the
/// underline from one input, so the omission is three surfaces at once. Two things
/// make it wrong rather than merely inconsistent: the band takes its hue from the
/// mosaic now, and a theme accent has no contrast floor against it — Latte's mauve
/// lands near 1.7:1 on the pinned band, under even the 3:1 non-text bar, where
/// `HeroBackdrop.chrome` is solved to clear it.
///
/// Asserted as "reads *some* `HeroBackdrop` tier" rather than pinning which one: the
/// tier a brush should take is a design call that may move, but reaching for `Theme.*`
/// here is a bug at any tier.
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

    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            mount.contains(&format!("{prop}: HeroBackdrop.")),
            "the hero band's header must pass `{prop}` a `HeroBackdrop` tier — omitting it falls \
             back to the component's `Theme.*` default, which is a theme value on a band that is \
             no longer theme-seeded"
        );
    }
}

/// The band forwards everything the shared header publishes, under the names its two
/// pages already read.
///
/// The row itself is pinned by `tab_search_header_tests`; what only this file can
/// check is that the forwards exist, because a band that mounts the header and drops
/// them compiles, paints correctly, and silently loses the tab slide's direction and
/// every compact tooltip. Two-way aliases rather than one-way bindings, so nothing
/// here can be orphaned by a write.
#[test]
fn the_band_forwards_what_the_shared_row_publishes() {
    for prop in ["tab-enter-from", "tab-anim-armed", "tip-w", "tip-h", "tip-label", "tip-visible"]
    {
        assert!(
            HERO.contains(&format!("{prop} <=> header.{prop};")),
            "the hero band must re-publish `{prop}` off the shared header — its pages read that \
             name"
        );
    }
    // The two positional anchors can't be plain aliases: they are relative to the
    // header, and a page's frame is relative to the band.
    for prop in ["tip-x", "tip-y"] {
        assert!(
            binding(&format!("out property <length> {prop}:")).contains(&format!("header.{prop}")),
            "the band's `{prop}` must offset the header's own by their `absolute-position` delta"
        );
    }
}

/// The banner's height is derived, not a literal, so growing the artwork or the bar
/// can't leave it cropping its own contents. The row's contribution comes off the
/// header rather than a restated `48px`, which is what the band used to carry.
#[test]
fn the_hero_height_is_derived_from_what_it_stacks() {
    let height = binding("out property <length> hero-height:");
    assert!(
        height.contains("header.row-h") && height.contains("Theme.hero-artwork"),
        "`hero-height` must sum the header row's own height and the artwork tile — a literal \
         drifts silently the moment either grows"
    );
}
