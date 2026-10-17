use melodia_testkit::{block_body, strip_line_comments};

use super::SettingsCategory;

const SETTINGS_PAGE: &str = include_str!("../settings_page.rs");

/// The category count Slint declares today. Kept local so a change to
/// `SettingsPage.category-count` doesn't silently rewrite what these assert.
const CATEGORIES: i32 = 10;

const GLOBAL: &str = include_str!("../../../../../melodia-ui/ui/globals/settings-page.slint");
const ROUTER: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/settings-router.slint");
const VIEW: &str = include_str!("../../../../../melodia-ui/ui/views/settings-view.slint");

const SECTION_CARD: &str =
    include_str!("../../../../../melodia-ui/ui/components/settings/section-card.slint");

/// The shared rows and divider the section cards are built from, by name so a failure says which
/// file, each with the inset that keeps its content clear of the card's rounded corners.
const CARD_CHILDREN: [(&str, &str, &str); 3] = [
    (
        "setting-row",
        include_str!("../../../../../melodia-ui/ui/components/settings/setting-row.slint"),
        "HorizontalLayout { padding: Theme.pad-lg;",
    ),
    (
        "setting-row-stacked",
        include_str!("../../../../../melodia-ui/ui/components/settings/setting-row-stacked.slint"),
        "HorizontalLayout { padding: Theme.pad-lg;",
    ),
    (
        "section-divider",
        include_str!("../../../../../melodia-ui/ui/components/settings/section-divider.slint"),
        "x: Theme.pad-lg; y: 0; width: parent.width - 2 * Theme.pad-lg;",
    ),
];

/// One page per category, by name so a failure says which file.
const PAGES: [(&str, &str); 10] = [
    (
        "library-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/library-page.slint"),
    ),
    (
        "playback-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/playback-page.slint"),
    ),
    (
        "output-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/output-page.slint"),
    ),
    (
        "appearance-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/appearance-page.slint"),
    ),
    (
        "window-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/window-page.slint"),
    ),
    (
        "radio-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/radio-page.slint"),
    ),
    (
        "lyrics-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/lyrics-page.slint"),
    ),
    (
        "integrations-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/integrations-page.slint"),
    ),
    (
        "updates-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/updates-page.slint"),
    ),
    (
        "about-page",
        include_str!("../../../../../melodia-ui/ui/views/settings/pages/about-page.slint"),
    ),
];

/// The `N` in `SettingsPage`'s `category-count: N;`.
fn declared_category_count() -> Option<usize> {
    melodia_testkit::declared_count(GLOBAL, "category-count")
}

/// The body of an inline `name: [ … ];` array literal in `settings-view.slint`.
fn array_body(marker: &str) -> Option<&'static str> {
    melodia_testkit::array_body(VIEW, marker)
}

/// `SettingsPage.category-count` is the sole definition of how many categories exist —
/// Rust clamps the persisted index against it instead of carrying its own const. Nothing
/// else in the build notices when it drifts from the categories actually declared: an
/// eleventh added without bumping it stays clickable but can never be restored from
/// `views.json`, and a bump without a matching router branch selects a branch that mounts
/// nothing.
#[test]
fn category_count_matches_the_categories_slint_declares() {
    let declared = declared_category_count();
    assert_eq!(
        declared.and_then(|n| i32::try_from(n).ok()),
        Some(CATEGORIES),
        "settings-page.slint must declare `out property <int> category-count: {CATEGORIES};`"
    );
    let count = declared.unwrap_or_default();

    // Line-anchored: `in-out property <int> category-idx` contains the same
    // substring, and it's the seat of the index, not one of the constants.
    let indices = GLOBAL
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("out property <int> category-"))
        .filter(|line| !line.starts_with("out property <int> category-count"))
        .count();
    assert_eq!(
        indices, count,
        "settings-page.slint's `category-*` index constants don't add up to `category-count`"
    );

    let branches = ROUTER
        .lines()
        .filter(|line| line.contains("root.category-index == SettingsPage.category-"))
        .count();
    assert_eq!(branches, count, "settings-router.slint's branches don't cover every category");

    let search_mounts =
        ROUTER.lines().filter(|line| line.contains(":=") && line.contains("Page {}")).count();
    assert_eq!(
        search_mounts, count,
        "settings-router.slint's search branch must mount every category page — a card that \
         isn't mounted there is unreachable by search"
    );

    let labels = array_body("labels: [");
    let icons = array_body("icons: [");
    assert!(
        labels.is_some() && icons.is_some(),
        "the panel's `labels`/`icons` must stay inline `[ … ];` array literals in \
         settings-view.slint"
    );
    let labels = labels.unwrap_or_default();
    assert_eq!(labels.split(',').count(), count, "the panel's `labels` array is the wrong length");
    // Counting `@tr(` too pins the "inline literal, never Rust-seeded"
    // contract: `@tr` registers msgids at codegen, so a `[string]` filled from
    // Rust would render untranslated.
    assert_eq!(
        labels.matches("@tr(\"").count(),
        count,
        "every category label must stay an inline `@tr(\"…\")` literal"
    );

    // `category_from_index` falls back to Library, so an eleventh category without a
    // variant logs as `Settings/Library` — right-looking, and wrong on the one category
    // nobody added a name for.
    assert_eq!(
        SettingsCategory::ALL.len(),
        count,
        "`SettingsCategory` needs one variant per category the page declares"
    );
    assert_eq!(
        icons.unwrap_or_default().split(',').count(),
        count,
        "the panel's `icons` array is the wrong length"
    );
}

/// The category's own name is part of every card's search term, and the page that
/// mounts the card is what supplies it. Omit `category-name:` on a mount and the
/// section falls back to an empty string: it still matches its own title, so the
/// page looks fine, but the card drops out of a search for the category it lives
/// on — the exact query "Integrations" exists to answer. Nothing in the build
/// notices, which is why it's pinned here.
#[test]
fn every_mounted_section_carries_its_category_name() {
    assert_eq!(
        Some(PAGES.len()),
        declared_category_count(),
        "there must be one page file per category declared in settings-page.slint"
    );

    for (page, src) in PAGES {
        let mounts: Vec<&str> =
            src.lines().map(str::trim).filter(|line| line.contains("Section {")).collect();
        assert!(!mounts.is_empty(), "{page}.slint mounts no section — the scan matched nothing");
        for mount in mounts {
            assert!(
                mount.contains("category-name:"),
                "{page}.slint mounts a section without `category-name:`, so it can't be found \
                 by searching for its category: {mount}"
            );
        }
    }
}

/// A rounded clip over children is an offscreen layer in `FemtoVG`, reallocated whenever the card's
/// width moves, which below the cap is every frame of a resize: the card half of the Settings page
/// stuttering through the miniplayer swap. It also blurs the card's text on `HiDPI`.
#[test]
fn no_section_card_clips_its_rounded_surface() {
    let card = melodia_testkit::code_tokens(SECTION_CARD);

    let clips = card.matches("clip: true").count();

    assert_eq!(clips, 0, "section-card.slint clips its rounded surface again");
}

/// What made dropping the clip safe: nothing a card holds reaches its corners. A row or divider
/// that loses its inset now paints square across the card's rounded edge, where the clip used to
/// hide it.
#[test]
fn every_card_child_insets_its_content_from_the_rounded_edge() {
    let flush: Vec<&str> = CARD_CHILDREN
        .into_iter()
        .filter(|(_, src, inset)| !melodia_testkit::code_tokens(src).contains(inset))
        .map(|(name, _, _)| name)
        .collect();

    assert!(
        flush.is_empty(),
        "these card children no longer inset from the card's edge: {flush:?}"
    );
}

/// Every deep link into Settings writes the category before the nav index, so the page mounts on
/// the body it is meant to show rather than on whichever category it was last left at.
///
/// Both writes go through the callbacks that already own an `IndexPersist`. Reaching for
/// `library::settings::set_settings_category` here instead would build, persist, and be a seventh
/// writer: `crates/melodia/tests/index_persist.rs` pins that count at six, but it walks
/// `melodia-views` for the *setter*, so it would never see this ordering.
#[test]
fn opening_settings_on_a_category_writes_the_category_before_the_nav_index() {
    let src = strip_line_comments(SETTINGS_PAGE);
    let body = src
        .find("pub fn open_on(")
        .and_then(|at| src[at..].find("{\n").map(|rel| at + rel))
        .and_then(|open| block_body(&src, open))
        .unwrap_or_default();

    let writes = (
        body.find("invoke_category_changed(idx)"),
        body.find("invoke_persist_selected_index(NAV_SETTINGS)"),
    );
    assert!(
        matches!(writes, (Some(category), Some(nav)) if category < nav),
        "the category must be written before the nav index, and both through the persisting \
         callbacks: {writes:?}"
    );
    assert!(
        !body.contains("set_settings_category"),
        "the deep link must reuse the category-changed callback, never the disk setter"
    );
}
