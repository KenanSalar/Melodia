use super::clamp_tab;

/// The tab count Slint declares today. Kept local so a change to
/// `SettingsPage.tab-count` doesn't silently rewrite what these assert.
const TABS: i32 = 5;

#[test]
fn clamp_tab_passes_through_valid_indices() {
    for tab in 0..TABS {
        assert_eq!(clamp_tab(tab, TABS), tab);
    }
}

#[test]
fn clamp_tab_pulls_out_of_range_back_in() {
    // A `views.json` from a build with more tabs, and a corrupt negative.
    assert_eq!(clamp_tab(99, TABS), TABS - 1);
    assert_eq!(clamp_tab(TABS, TABS), TABS - 1);
    assert_eq!(clamp_tab(-1, TABS), 0);
}

/// `clamp(0, -1)` panics, so the upper bound is floored at 0. Not reachable
/// while the global declares five tabs, but the arithmetic shouldn't be the
/// thing that decides that.
#[test]
fn clamp_tab_survives_a_zero_tab_count() {
    assert_eq!(clamp_tab(0, 0), 0);
    assert_eq!(clamp_tab(7, 0), 0);
}

const GLOBAL: &str = include_str!("../../../melodia-ui/ui/globals/settings-page.slint");
const ROUTER: &str = include_str!("../../../melodia-ui/ui/views/settings/settings-tabs.slint");
const VIEW: &str = include_str!("../../../melodia-ui/ui/views/settings-view.slint");

/// One tab page per tab, by name so a failure says which file.
const PAGES: [(&str, &str); 5] = [
    ("library-page", include_str!("../../../melodia-ui/ui/views/settings/pages/library-page.slint")),
    (
        "playback-page",
        include_str!("../../../melodia-ui/ui/views/settings/pages/playback-page.slint"),
    ),
    (
        "interface-page",
        include_str!("../../../melodia-ui/ui/views/settings/pages/interface-page.slint"),
    ),
    (
        "services-page",
        include_str!("../../../melodia-ui/ui/views/settings/pages/services-page.slint"),
    ),
    ("about-page", include_str!("../../../melodia-ui/ui/views/settings/pages/about-page.slint")),
];

/// The `N` in `SettingsPage`'s `tab-count: N;`.
fn declared_tab_count() -> Option<usize> {
    GLOBAL
        .split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
}

/// The body of an inline `name: [ … ];` array literal in `settings-view.slint`.
fn array_body(marker: &str) -> Option<&'static str> {
    VIEW.split_once(marker)
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
}

/// `SettingsPage.tab-count` is the sole definition of how many tabs exist —
/// Rust clamps the persisted index against it instead of carrying its own
/// const. Nothing else in the build notices when it drifts from the tabs
/// actually declared: a sixth tab added without bumping it stays clickable but
/// can never be restored from `views.json`, and a bump without a matching
/// router branch selects a branch that mounts nothing.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let declared = declared_tab_count();
    assert_eq!(
        declared.and_then(|n| i32::try_from(n).ok()),
        Some(TABS),
        "settings-page.slint must declare `out property <int> tab-count: {TABS};`"
    );
    let count = declared.unwrap_or_default();

    // Line-anchored: `in-out property <int> tab-idx` contains the same
    // substring, and it's the seat of the index, not one of the constants.
    let indices = GLOBAL
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("out property <int> tab-"))
        .filter(|line| !line.starts_with("out property <int> tab-count"))
        .count();
    assert_eq!(
        indices, count,
        "settings-page.slint's `tab-*` index constants don't add up to `tab-count`"
    );

    let branches = ROUTER
        .lines()
        .filter(|line| line.contains("root.tab-index == SettingsPage.tab-"))
        .count();
    assert_eq!(branches, count, "settings-tabs.slint's router branches don't cover every tab");

    let search_mounts =
        ROUTER.lines().filter(|line| line.contains(":=") && line.contains("Page {}")).count();
    assert_eq!(
        search_mounts, count,
        "settings-tabs.slint's search branch must mount every tab page — a card that isn't \
         mounted there is unreachable by search"
    );

    let labels = array_body("labels: [");
    let icons = array_body("icons: [");
    assert!(
        labels.is_some() && icons.is_some(),
        "the tab bar's `labels`/`icons` must stay inline `[ … ];` array literals in \
         settings-view.slint"
    );
    let labels = labels.unwrap_or_default();
    assert_eq!(labels.split(',').count(), count, "the tab bar's `labels` array is the wrong length");
    // Counting `@tr(` too pins the "inline literal, never Rust-seeded"
    // contract: `@tr` registers msgids at codegen, so a `[string]` filled from
    // Rust would render untranslated.
    assert_eq!(
        labels.matches("@tr(\"").count(),
        count,
        "every tab label must stay an inline `@tr(\"…\")` literal"
    );
    assert_eq!(
        icons.unwrap_or_default().split(',').count(),
        count,
        "the tab bar's `icons` array is the wrong length"
    );
}

/// The tab's own name is part of every card's search term, and the page that
/// mounts the card is what supplies it. Omit `tab-name:` on a mount and the
/// section falls back to an empty string: it still matches its own title, so
/// the page looks fine, but the card drops out of a search for the tab it
/// lives on — the exact query "Interface" and "Services" exist to answer.
/// Nothing in the build notices, which is why it's pinned here.
#[test]
fn every_mounted_section_carries_its_tab_name() {
    assert_eq!(
        Some(PAGES.len()),
        declared_tab_count(),
        "there must be one tab page file per tab declared in settings-page.slint"
    );

    for (page, src) in PAGES {
        let mounts: Vec<&str> =
            src.lines().map(str::trim).filter(|line| line.contains("Section {")).collect();
        assert!(!mounts.is_empty(), "{page}.slint mounts no section — the scan matched nothing");
        for mount in mounts {
            assert!(
                mount.contains("tab-name:"),
                "{page}.slint mounts a section without `tab-name:`, so it can't be found by \
                 searching for its tab: {mount}"
            );
        }
    }
}
