use super::{SettingsTab, chunk_indices};

/// The tab count Slint declares today. Kept local so a change to
/// `SettingsPage.tab-count` doesn't silently rewrite what these assert.
const TABS: i32 = 5;

#[test]
fn chunk_indices_fills_rows_left_to_right() {
    assert_eq!(chunk_indices(7, 3), vec![vec![0, 1, 2], vec![3, 4, 5], vec![6]]);
    assert_eq!(chunk_indices(6, 3), vec![vec![0, 1, 2], vec![3, 4, 5]]);
    assert_eq!(chunk_indices(2, 5), vec![vec![0, 1]]);
}

#[test]
fn chunk_indices_has_no_rows_for_nothing_to_place() {
    assert!(chunk_indices(0, 4).is_empty());
    assert!(chunk_indices(-3, 4).is_empty());
}

/// `per_row` comes from a measured width, which is zero for the frame before
/// the first layout reports one — so it has to floor at one item per row
/// rather than loop forever or divide by zero.
#[test]
fn chunk_indices_floors_a_degenerate_row_width_at_one() {
    assert_eq!(chunk_indices(3, 0), vec![vec![0], vec![1], vec![2]]);
    assert_eq!(chunk_indices(3, -1), vec![vec![0], vec![1], vec![2]]);
}

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/settings-page.slint");
const ROUTER: &str = include_str!("../../../../melodia-ui/ui/views/settings/settings-tabs.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/settings-view.slint");

/// One tab page per tab, by name so a failure says which file.
const PAGES: [(&str, &str); 5] = [
    ("library-page", include_str!("../../../../melodia-ui/ui/views/settings/pages/library-page.slint")),
    (
        "playback-page",
        include_str!("../../../../melodia-ui/ui/views/settings/pages/playback-page.slint"),
    ),
    (
        "interface-page",
        include_str!("../../../../melodia-ui/ui/views/settings/pages/interface-page.slint"),
    ),
    (
        "services-page",
        include_str!("../../../../melodia-ui/ui/views/settings/pages/services-page.slint"),
    ),
    ("about-page", include_str!("../../../../melodia-ui/ui/views/settings/pages/about-page.slint")),
];

/// The `N` in `SettingsPage`'s `tab-count: N;`.
fn declared_tab_count() -> Option<usize> {
    crate::test_support::declared_tab_count(GLOBAL)
}

/// The body of an inline `name: [ … ];` array literal in `settings-view.slint`.
fn array_body(marker: &str) -> Option<&'static str> {
    crate::test_support::array_body(VIEW, marker)
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

    // `tab_from_index` ends in a default arm, so a sixth tab without a variant
    // logs as `Settings/Library` — right-looking, and wrong on the one tab
    // nobody added a name for.
    assert_eq!(
        SettingsTab::ALL.len(),
        count,
        "`SettingsTab` needs one variant per tab the page declares"
    );
    assert_eq!(
        icons.unwrap_or_default().split(',').count(),
        count,
        "the tab bar's `icons` array is the wrong length"
    );
}

/// The header row is drawn from `page-w` for one frame before the first layout
/// reports the truth, and that seed has to be the row's own floor rather than a
/// plausible page width. Seeded wide, the bar believes it can afford five
/// full-width tabs, draws them into a panel that can't seat them, and they spill
/// under the search bar — which is what a miniplayer → full swap reliably
/// produced. Seeded at the floor it draws icons and widens once. A literal reads
/// as harmless to anyone who hasn't seen it fail, so pin that it's derived.
///
/// The floor itself is `TabSearchHeader.row-floor`'s to compute, and
/// `tab_search_header_tests` holds it to the sum. What this page owes is reading it
/// rather than re-deriving one, which is what it and both hero bands used to do.
#[test]
fn the_page_width_seed_is_the_rows_floor() {
    let seed = VIEW
        .split_once("property <length> page-w:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map(|(value, _)| value)
        .unwrap_or_default();

    assert!(
        seed.contains("header.row-floor"),
        "settings-view.slint must seed `page-w` from the shared header row's published floor, \
         not from a guessed page width: {seed:?}"
    );
}

/// A card grows with the panel until it reaches `SettingsPage.card-cap`, and
/// only once *two* capped cards fit does a second column appear — so a card's
/// width is monotonic and the flip resizes nothing. That holds only while the
/// threshold is reserved against the cap itself. Spell it as its own literal
/// and it can sit below the cap, at which point the column divides before ever
/// reaching it: the cap is still in the source, still reads as the card's
/// maximum, and is simply unreachable. What you see is a card that grows
/// without limit and then halves, which looks like the cap was never applied.
#[test]
fn the_two_column_flip_reserves_two_full_cards() {
    let threshold = VIEW
        .split_once("property <bool> two-col:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map(|(value, _)| value)
        .unwrap_or_default();

    assert!(
        threshold.contains("2 * SettingsPage.card-cap"),
        "settings-view.slint must reserve the two-column flip against two full \
         `SettingsPage.card-cap` columns, not against a literal of its own: {threshold:?}"
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

/// Every `SettingsPage.<name>(N)` index in `src`, in source order.
fn grid_indices(src: &str, name: &str) -> Vec<usize> {
    let marker = format!("SettingsPage.{name}(");
    src.match_indices(&marker)
        .filter_map(|(at, _)| src[at + marker.len()..].split_once(')'))
        .filter_map(|(digits, _)| digits.trim().parse().ok())
        .collect()
}

/// A page's columns place themselves in the body grid by a hand-written index.
/// Repeat one and two columns land in the same cell — Slint stacks them, so the
/// page loses a whole column with nothing in the build to say so, and only in
/// the two-column layout. Skip one and the grid grows a hole.
///
/// The **cap of two** is the subtler half. `body-cols` is only ever 1 or 2, so
/// a third column would fold back onto the first: `grid-col(2)` is `2 % 2`, and
/// its `grid-row(2)` of 1 is where column 1 already sits when stacked. Nothing
/// errors — one column simply paints on top of another, at one width and not
/// the other.
#[test]
fn every_column_takes_its_own_cell() {
    for (page, src) in PAGES {
        let columns = grid_indices(src, "grid-row");
        assert!(!columns.is_empty(), "{page}.slint places no column — the scan matched nothing");
        assert!(
            columns.len() <= 2,
            "{page}.slint declares {} columns; the body is never wider than two, so the third \
             onwards would share a cell with an earlier one",
            columns.len()
        );

        let expected: Vec<usize> = (0..columns.len()).collect();
        for name in ["grid-row", "grid-col"] {
            assert_eq!(
                grid_indices(src, name),
                expected,
                "{page}.slint must pass `{name}(0)`..`{name}({})` once each, in source order",
                columns.len().saturating_sub(1)
            );
        }
    }
}
