//! The page's own wiring, and the two mistakes it can make silently.
//!
//! Both are about the seam between `globals/radio.slint` and this slice: a tab the global declares
//! that Rust cannot resolve, and a model the global declares that Rust never installs. Neither is
//! a build failure and neither is visible in review.

use super::RadioTab;

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/radio.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/radio-view.slint");
const MOD: &str = include_str!("../mod.rs");

/// The tab count Slint declares today. Kept local so a change to `Radio.tab-count` doesn't
/// silently rewrite what this asserts.
const TABS: usize = 3;

/// `Radio.tab-count` is the sole definition of how many sub-views exist — `seed_tab` clamps the
/// persisted `views.json` index against it instead of carrying its own const. Nothing else in the
/// build notices when it drifts from the tabs actually declared: a fourth tab added without
/// bumping it stays clickable but can never be restored from `views.json`, and a bump without a
/// matching body branch leaves the page blank on that tab.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let declared = crate::test_support::declared_tab_count(GLOBAL);
    assert_eq!(
        declared,
        Some(TABS),
        "radio.slint must declare `out property <int> tab-count: {TABS};`"
    );

    // Line-anchored: `in-out property <int> tab-idx` shares the substring, and it is the seat of
    // the index rather than one of the constants.
    let indices = GLOBAL
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("out property <int> tab-"))
        .filter(|line| !line.starts_with("out property <int> tab-count"))
        .count();
    assert_eq!(indices, TABS, "`Radio`'s `tab-*` constants don't add up to `tab-count`");

    // Anchored on the branch's own shape rather than on the comparison alone: the sheet reads
    // `tab-idx` again for the count line, the placeholder and the pill row, and none of those is
    // a sub-view. It also pins that every body is wrapped — one mounted bare would appear without
    // the sideways enter the others play.
    let branches = VIEW
        .lines()
        .filter(|line| line.contains("if Radio.tab-idx == Radio.tab-"))
        .filter(|line| line.contains(": ViewTransition {"))
        .count();
    assert_eq!(
        branches, TABS,
        "radio-view.slint must mount one `ViewTransition` body branch per tab — a tab with no \
         branch shows a blank page"
    );

    for marker in ["tab-labels: [", "tab-icons: ["] {
        let body = crate::test_support::array_body(VIEW, marker);
        assert!(
            body.is_some(),
            "the band's `{marker}…];` must stay an inline array literal — `@tr` folds msgids at \
             codegen, so a Rust-seeded `[string]` renders untranslated"
        );
        assert_eq!(
            body.unwrap_or_default().split(',').count(),
            TABS,
            "`{marker}` needs one entry per tab"
        );
    }
}

/// `tab_from_index` ends in a default arm, so a tab added to the global without one here resolves
/// to Browse rather than to nothing — and `ui::view_tag` logs whichever it landed on. Pinned as a
/// source read because the resolve takes a live Slint global.
#[test]
fn an_unknown_tab_index_resolves_to_browse() {
    let source = include_str!("../tabs.rs");
    let body = source
        .split_once("pub fn tab_from_index")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(!body.is_empty(), "`tab_from_index` moved or changed shape, so this pin reads nothing");
    assert!(
        body.trim_end().ends_with("RadioTab::Browse\n    }"),
        "`tab_from_index` must end in an unconditional `RadioTab::Browse` arm"
    );

    // One variant per declared tab, so a fourth `tab-*` constant can't quietly share a variant.
    let variants = source
        .split_once("pub enum RadioTab {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or(0, |(body, _)| body.lines().filter(|l| l.trim_end().ends_with(',')).count());
    assert_eq!(variants, TABS, "`RadioTab` needs one variant per tab the global declares");
}

/// **Every station grid the global declares has to be handed a `VecModel` at install.**
///
/// This is the one failure in the page that is silent at both ends: an unbound Slint array is a
/// model of its own kind, so `write_grid`'s downcast misses, the grid stays empty, and the only
/// report is one log line per attempted write. It has cost a round already — the Favorites and
/// Recently Played grids shipped declared but never installed, and both tabs simply painted their
/// empty state while the database held the rows.
#[test]
fn every_station_grid_the_global_declares_is_handed_a_model() {
    let declared: Vec<String> = GLOBAL
        .lines()
        .map(str::trim_start)
        .filter_map(|line| line.strip_prefix("in-out property <[RadioStationGridRow]> "))
        .filter_map(|rest| rest.split_once(';'))
        .map(|(name, _)| name.trim().replace('-', "_"))
        .collect();

    assert_eq!(declared.len(), TABS, "one grid model per tab is what this page is: {declared:?}");

    let install = MOD
        .split_once("fn install_models")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);
    assert!(!install.is_empty(), "`install_models` moved or changed shape");

    for name in &declared {
        assert!(
            install.contains(&format!("g.set_{name}(")),
            "`Radio.{name}` is declared but `install_models` never gives it a `VecModel` — every \
             write to it downcasts to nothing and the grid paints empty"
        );
    }

    // The function is reached from `install`, not merely present.
    assert!(
        MOD.contains("install_models(cx.app);"),
        "`install` must call `install_models` before anything can write a grid"
    );
}

/// The two local tabs share one module keyed on this enum, and `cache` splits on `Recent` with
/// everything else falling to the kept list. A third local tab added without an arm there would
/// silently draw the Favorites cache under its own name.
#[test]
fn the_kept_cache_splits_on_the_recent_tab_alone() {
    let source = include_str!("../kept.rs");
    let body = source
        .split_once("fn cache(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(body.contains("RadioTab::Recent => &radio_ui.recent"));
    assert!(
        body.contains("_ => &radio_ui.kept"),
        "the fall-through must be the kept list, which is what Browse's own callers never reach"
    );
    // Cheap proof the enum this splits on is the one under test.
    assert_ne!(RadioTab::Recent, RadioTab::Favorites);
}

/// Every property the station form declares is answered by `RadioForm.reset()`.
///
/// The add dialog opens on whatever the last close left behind, so a property the reset forgets is
/// the previous station's value riding into the next one — and for the four `can-edit-*` flags it
/// is worse than stale, `false` being a field the dialog does not draw at all. Neither is a build
/// failure, and both read on screen as a form that is simply wrong rather than as a bug with a
/// cause.
#[test]
fn the_station_form_resets_every_property_it_declares() {
    const FORMS: &str = include_str!("../../../../melodia-ui/ui/globals/dialog-forms.slint");
    let src = crate::test_support::strip_line_comments(FORMS);

    let global = src
        .find("export global RadioForm")
        .and_then(|at| src[at..].find('{').map(|rel| at + rel))
        .and_then(|open| crate::test_support::block_body(&src, open))
        .unwrap_or_default();
    let reset = global
        .find("public function reset()")
        .and_then(|at| global[at..].find('{').map(|rel| at + rel))
        .and_then(|open| crate::test_support::block_body(global, open))
        .unwrap_or_default();
    assert!(!reset.is_empty(), "RadioForm must keep a reset() for both openers to share");

    let declared: Vec<&str> = global
        .match_indices("in-out property <")
        .filter_map(|(at, _)| global[at..].split_once('>'))
        .filter_map(|(_, rest)| rest.trim_start().split([';', ':']).next())
        .map(str::trim)
        .collect();
    assert!(declared.len() >= 12, "only {} properties found — the walk is broken", declared.len());

    let missing: Vec<&&str> =
        declared.iter().filter(|name| !reset.contains(&format!("root.{name} ="))).collect();
    assert!(missing.is_empty(), "reset() leaves these for the next dialog to inherit: {missing:?}");
}
