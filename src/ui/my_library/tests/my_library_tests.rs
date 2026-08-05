//! Source pins for the My Library page.
//!
//! Every one of these holds something that builds, looks right, and is wrong: a tab
//! whose gate was never mounted, a drill whose origin can't be restored, a section
//! shadow seeded against the wrong question. None of it surfaces at runtime as a
//! failure — it surfaces as a page that quietly refetches, or a back button that lands
//! on the wrong tab.

use super::{NAV_MY_LIBRARY, fold_retired_nav_index};

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/my-library.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/my-library-view.slint");
const APP_WINDOW: &str = include_str!("../../../../melodia-ui/ui/app-window.slint");
const SIDEBAR: &str = include_str!("../../../../melodia-ui/ui/layout/sidebar.slint");

/// The `tab-*` constant names the `MyLibrary` global declares, `tab-count` excluded.
fn declared_tabs() -> Vec<String> {
    GLOBAL
        .lines()
        .filter_map(|line| line.trim().strip_prefix("out property <int> tab-"))
        .filter_map(|rest| rest.split(':').next())
        .filter(|name| *name != "count")
        .map(str::to_owned)
        .collect()
}

fn declared_tab_count() -> usize {
    let digits = GLOBAL
        .split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(digits, _)| digits.trim());
    let parsed = digits.parse::<usize>().ok();
    assert!(
        parsed.is_some(),
        "`MyLibrary.tab-count` must stay a plain `out property <int> tab-count: N;` — it is \
         the sole definition of how many tabs there are, and Rust clamps the persisted tab \
         against it. Found: {digits:?}",
    );
    parsed.unwrap_or_default()
}

/// The contents of an inline `name: [ … ]` array in the mount sheet, `""` if it is gone.
fn sheet_array(name: &str) -> &'static str {
    VIEW.split_once(&format!("{name}: ["))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map_or("", |(items, _)| items)
}

/// `tab-count` is what Rust clamps the persisted tab against, so a build whose count
/// disagrees with its tabs restores onto a branch that mounts nothing. Everything that
/// has to move with it is checked here: the constants, the bar's two parallel arrays,
/// and the sheet's own branches.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let tabs = declared_tabs();
    let count = declared_tab_count();
    assert_eq!(
        tabs.len(),
        count,
        "`MyLibrary` declares {} `tab-*` constants but `tab-count: {count}`: {tabs:?}",
        tabs.len(),
    );

    // The labels have to stay an inline `[@tr(…), …]` literal — a Rust-seeded
    // `[string]` renders untranslated — and the icons array has to match it 1:1.
    let labels = sheet_array("labels");
    assert_eq!(
        labels.matches("@tr(").count(),
        count,
        "the tab bar's `labels` array must be an inline literal carrying one `@tr(…)` per \
         tab: {labels:?}",
    );
    let icons = sheet_array("icons");
    assert_eq!(
        icons.matches('"').count(),
        count * 2,
        "the tab bar's `icons` array must carry one glyph per tab: {icons:?}",
    );

    // Every declared tab has to route to a body and gate a section, or it is a tab that
    // mounts nothing while the bar happily draws it.
    for tab in &tabs {
        assert!(
            VIEW.contains(&format!("MyLibrary.tab-{tab}")),
            "the mount sheet routes no body for `tab-{tab}`",
        );
    }
}

/// **A tab leave has to be the same event as a section leave.** Five gates at one nav
/// index, each naming a different tab — the one thing that makes every existing
/// lifecycle hook (cover release, model clear, `mark_dirty()` → refetch) keep working
/// untouched. A missing mount leaves that view active forever and refetching the whole
/// library behind whichever tab is on screen.
#[test]
fn every_tab_carries_its_own_section_gate() {
    let tabs = declared_tabs();
    for tab in &tabs {
        let mount = format!("tab-index: MyLibrary.tab-{tab};");
        assert_eq!(
            APP_WINDOW.matches(&mount).count(),
            1,
            "`app-window.slint` must mount exactly one `SectionActiveGate` for `tab-{tab}`",
        );
    }
    assert_eq!(
        APP_WINDOW.matches("current-tab: MyLibrary.tab-idx;").count(),
        tabs.len(),
        "every tab-scoped gate needs `current-tab` too — without it `tab-index` compares \
         against the gate's own `-1` default and the section never activates",
    );
}

const ORIGIN_WRITERS: [(&str, &str); 5] = [
    ("cross_tab_nav", include_str!("../../callbacks/cross_tab_nav.rs")),
    ("albums/grid", include_str!("../../callbacks/albums/grid.rs")),
    ("albums/detail", include_str!("../../callbacks/albums/detail.rs")),
    ("artists/detail", include_str!("../../callbacks/artists/detail.rs")),
    ("genres/detail", include_str!("../../callbacks/genres/detail.rs")),
];

const ARTIST_CROSS_TAB: &str = include_str!("../../callbacks/artists/cross_tab.rs");

const DETAIL_GLOBALS: [(&str, &str); 3] = [
    ("AlbumDetail", include_str!("../../../../melodia-ui/ui/globals/albums.slint")),
    ("ArtistDetail", include_str!("../../../../melodia-ui/ui/globals/artists.slint")),
    ("GenreDetail", include_str!("../../../../melodia-ui/ui/globals/genres.slint")),
];

/// **A drill's origin is a pair.** With five views on nav index 3, `origin-nav-index`
/// alone can't tell one tab from another: the guard that stops a mid-fetch nav from
/// yanking the user becomes `3 == 3`, and the back path's restore becomes a no-op that
/// leaves the wrong tab mounted. Both halves are written and cleared together.
#[test]
fn every_detail_records_the_tab_it_was_opened_from() {
    for (name, source) in DETAIL_GLOBALS {
        assert!(
            source.contains("in-out property <int> origin-tab: -1;"),
            "`{name}` no longer declares `origin-tab`",
        );
    }
    for (name, source) in ORIGIN_WRITERS {
        let nav = source.matches("set_origin_nav_index(").count();
        let tab = source.matches("set_origin_tab(").count();
        assert!(nav > 0, "`{name}` no longer writes `origin-nav-index` — pin is stale");
        assert_eq!(
            nav, tab,
            "`{name}` writes `origin-nav-index` {nav}× but `origin-tab` {tab}×; the two \
             halves have to move together",
        );
    }

    // Artist Detail → Album Detail is the one drill that used to stamp the origin
    // itself, with the source tab hardcoded. It goes through the shared hand-off now,
    // so the pair and the mid-fetch guard are spelled once; re-inlining it is what
    // this catches.
    assert!(
        ARTIST_CROSS_TAB.contains("cross_tab_nav::open_album_cross_tab(")
            && !ARTIST_CROSS_TAB.contains("set_origin_nav_index("),
        "`artists/cross_tab` must route through `cross_tab_nav::open_album_cross_tab` \
         rather than stamping the origin pair itself",
    );
}

const SECTION_SEEDS: [(&str, &str, &str); 5] = [
    ("Songs", "tracks", include_str!("../../callbacks/tracks.rs")),
    ("Albums", "albums", include_str!("../../callbacks/albums/lifecycle.rs")),
    ("Artists", "artists", include_str!("../../callbacks/artists/lifecycle.rs")),
    ("Genres", "genres", include_str!("../../callbacks/genres/lifecycle.rs")),
    ("Playlists", "playlists", include_str!("../../callbacks/playlists/lifecycle.rs")),
];

/// `SectionActiveGate` fires on transitions only, and its `ChangeTracker` baselines
/// silently inside `AppWindow::new()` — so each view's synchronous `section_active`
/// shadow has to be *right on its own* at wire time. Seeded against the nav index alone
/// all five answer `true` together: four views wrongly active for a session, each
/// refetching the whole library on every `library_changed` bump behind a tab nobody is
/// looking at.
#[test]
fn every_section_seed_reads_the_mounted_tab() {
    for (variant, name, source) in SECTION_SEEDS {
        let seed = format!("tab_is_mounted(ui, MyLibraryTab::{variant})");
        assert!(
            source.contains(&seed),
            "`callbacks/{name}` must seed `section_active` from `{seed}`, not from the nav \
             index — all five sections share index {NAV_MY_LIBRARY}",
        );
    }
}

/// The sidebar row the five replaced. Visual order follows source order, so the label
/// swap is also what puts My Library where Tracks was — directly under Recently Played.
#[test]
fn the_sidebar_offers_one_row_for_the_whole_page() {
    for gone in ["@tr(\"Tracks\")", "@tr(\"Albums\")", "@tr(\"Artists\")", "@tr(\"Genres\")"] {
        assert!(
            !SIDEBAR.contains(gone),
            "`sidebar.slint` still carries a row for {gone} — those are tabs now",
        );
    }
    assert_eq!(
        SIDEBAR.matches("@tr(\"My Library\")").count(),
        1,
        "expected exactly one My Library sidebar row",
    );
}

/// A `views.json` written by a released build still holds 4–7. They route nowhere now,
/// so without the fold those installs boot onto `PlaceholderView`.
#[test]
fn the_retired_indices_fold_onto_the_page_that_absorbed_them() {
    for retired in 4..=7 {
        assert_eq!(fold_retired_nav_index(retired), NAV_MY_LIBRARY);
    }
    for kept in [0, 1, 2, 3, 8, 9] {
        assert_eq!(fold_retired_nav_index(kept), kept, "{kept} is a live index");
    }
    // Out of range in either direction is the caller's problem, not the fold's.
    assert_eq!(fold_retired_nav_index(-1), -1);
    assert_eq!(fold_retired_nav_index(42), 42);
}
