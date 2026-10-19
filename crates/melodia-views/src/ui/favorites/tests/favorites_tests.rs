use std::sync::Arc;

use super::rows::Scrolls;
use super::{FavoritesTab, FavoritesUi};
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_testkit::write_test_png;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GLOBAL: &str = include_str!("../../../../../melodia-ui/ui/globals/curated.slint");
const VIEW: &str = include_str!("../../../../../melodia-ui/ui/views/favorites-view.slint");
const MOST_PLAYED_TAB: &str =
    include_str!("../../../../../melodia-ui/ui/views/favorites/most-played-tab.slint");
const ARTISTS_TAB: &str =
    include_str!("../../../../../melodia-ui/ui/views/favorites/artists-tab.slint");
const GRID: &str =
    include_str!("../../../../../melodia-ui/ui/components/grid/entity-card-grid.slint");
const SONGS: &str = include_str!("../songs.rs");
const SONGS_TAB: &str =
    include_str!("../../../../../melodia-ui/ui/views/favorites/songs-tab.slint");
const SUBVIEWS: &str = include_str!("../callbacks/subviews.rs");

/// `Favorites.tracks` feeds one element in the whole tree, under the Songs
/// tab's `if` — so off that tab, every keyset load the Songs path runs
/// reaches nothing.
///
/// Two things hold that, and each fails differently. `requery_tracks`' bail is
/// what skips the query; drop it and the tab gate becomes decorative, every
/// keystroke on a grid tab reloading a list nobody can see. `show_tracks`'
/// re-check is what survives a pick landing mid-load; drop it and a load the
/// pick overtook replaces the list the tab left behind, without the dirty flag
/// that tells the next visit to reload it.
///
/// Neither shows on screen, which is why they are pinned rather than trusted.
#[test]
fn the_songs_model_is_written_only_while_its_tab_is_mounted() {
    assert!(
        SONGS_TAB.contains("rows: Favorites.tracks;"),
        "songs-tab.slint is the sole reader of `Favorites.tracks` — if that moved, this gate needs re-deriving"
    );

    let requery = SONGS
        .split_once("pub fn requery_tracks(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .unwrap_or_default();
    assert!(
        requery.contains("if fav_ui.active_tab() != FavoritesTab::Songs {"),
        "`requery_tracks` must bail before the query when Songs isn't mounted"
    );

    let show = SONGS
        .split_once("fn show_tracks(")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map(|(body, _)| body)
        .unwrap_or_default();
    assert!(
        show.contains("|| fav_ui.active_tab() != FavoritesTab::Songs"),
        "`show_tracks` must re-check the tab — a pick can land while the load is in flight"
    );
}

/// The tab count Slint declares today. Kept local so a change to
/// `Favorites.tab-count` doesn't silently rewrite what this asserts.
const TABS: usize = 3;

/// The `N` in `Favorites`'s `tab-count: N;`.
fn declared_tab_count() -> Option<usize> {
    melodia_testkit::declared_count(GLOBAL, "tab-count")
}

/// The body of an inline `name: [ … ];` array literal in `favorites-view.slint`.
fn array_body(marker: &str) -> Option<&'static str> {
    melodia_testkit::array_body(VIEW, marker)
}

/// `Favorites.tab-count` is the sole definition of how many sub-views exist —
/// `seed_tab` clamps the persisted `views.json` index against it instead of
/// carrying its own const. Nothing else in the build notices when it drifts
/// from the tabs actually declared: a fourth tab added without bumping it
/// stays clickable but can never be restored from `views.json`, and a bump
/// without a matching body branch leaves the page blank on that tab.
#[test]
fn tab_count_matches_the_tabs_slint_declares() {
    let declared = declared_tab_count();
    assert_eq!(
        declared,
        Some(TABS),
        "curated.slint's `Favorites` must declare `out property <int> tab-count: {TABS};`"
    );
    let count = declared.unwrap_or_default();

    // Line-anchored: `in-out property <int> tab-idx` shares the substring,
    // and it's the seat of the index, not one of the constants. Scoped to the
    // `Favorites` global so `RecentlyPlayed` growing tabs later can't inflate
    // this count.
    let favorites_global = GLOBAL
        .split_once("export global Favorites {")
        .and_then(|(_, rest)| rest.split_once("export global "))
        .map_or("", |(body, _)| body);
    let indices = favorites_global
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("out property <int> tab-"))
        .filter(|line| !line.starts_with("out property <int> tab-count"))
        .count();
    assert_eq!(indices, count, "`Favorites`'s `tab-*` constants don't add up to `tab-count`");

    assert_eq!(
        FavoritesTab::ALL.len(),
        count,
        "`FavoritesTab` needs one variant per tab the global declares"
    );

    // Anchored on the branch's own shape (`… : ViewTransition {`) rather than
    // on the comparison alone: the hero reads `tab-idx` several more times
    // for its stats line, its placeholder and its two pill gates, and those
    // are not sub-views. It also pins that every sub-view is wrapped — one
    // mounted bare would appear without the sideways enter the others play.
    let branches = VIEW
        .lines()
        .filter(|line| line.contains("if Favorites.tab-idx == Favorites.tab-"))
        .filter(|line| line.contains(": ViewTransition {"))
        .count();
    assert_eq!(
        branches, count,
        "favorites-view.slint must mount one `ViewTransition` body branch per tab — a tab with no \
         branch shows a blank page"
    );

    let labels = array_body("labels: [");
    let icons = array_body("icons: [");
    assert!(
        labels.is_some() && icons.is_some(),
        "the tab bar's `labels`/`icons` must stay inline `[ … ];` array literals in \
         favorites-view.slint"
    );
    let labels = labels.unwrap_or_default();
    assert_eq!(
        labels.split(',').count(),
        count,
        "the tab bar's `labels` array is the wrong length"
    );
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

/// A tab pick has to clear the filter it was made under, and clear it on *both*
/// sides. A Songs needle carried into the Artists grid silently hides cards, and
/// the two halves fail differently: leaving the Slint property set leaves the
/// box holding text the page is no longer filtered by, and leaving the Rust
/// shadow set filters the entering tab's model against it.
#[test]
fn a_tab_pick_clears_the_filter_on_both_sides() {
    let handler = VIEW
        .split_once("tab-selected(i) =>")
        .and_then(|(_, rest)| rest.split_once("Favorites.tab-changed(i);"))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("Favorites.filter = \"\";"),
        "favorites-view.slint's `tab-selected` handler must clear the Slint-side filter before \
         handing the pick to Rust"
    );

    assert!(
        SUBVIEWS.contains("favorites_ui_mod::set_filter(&fu, \"\");"),
        "the tab-change handler must drop the Rust filter shadow to match — the model build and \
         every later fetch read that, not the Slint property"
    );
}

/// Every field a sort pill can ask for has to be one the comparator handles.
///
/// The token is a bare string on both sides — an element of the mount's `fields`
/// array in the Slint, a `match` arm in `grids::sort::sort_artists` — so a typo or
/// a rename on either side compiles, and the pill just quietly sorts by the default
/// arm while painting its arrow as though it had worked. My Library's three rows are
/// pinned the same way, by `ui::my_library::tests`, through the same parser.
///
/// The per-pill contracts this used to count — `reserve-sort-slot`, and
/// `sort-direction` bound to the active field — moved into `SortPillRow` with the
/// row itself, and `ui::my_library::tests` pins them there once for all four rows.
#[test]
fn every_sort_pill_asks_for_a_field_the_comparator_knows() {
    // The arms of `grids::sort::sort_artists`, restated. A field dropped there and
    // left here fails the round-trip below; one added here and not there falls
    // through to the default and this test keeps passing — which is fine, since
    // the default is a defined order, not a no-op.
    const FIELDS: [&str; 2] = ["name", "favorite_count"];

    let arrays = melodia_testkit::sort_mount_arrays(VIEW, "Favorites.artist-sort-field");
    assert!(
        arrays.is_some(),
        "favorites-view.slint must mount a `SortPillRow` bound to \
         Favorites.artist-sort-field"
    );
    // Unreachable past the assert; spelled this way because the crate denies
    // `unwrap`, `expect` and `panic!` in tests as well as in production code.
    let Some((labels, asked)) = arrays else {
        return;
    };
    let asked: Vec<&str> = asked.split(',').map(|f| f.trim().trim_matches('"')).collect();

    assert_eq!(
        asked.len(),
        FIELDS.len(),
        "favorites-view.slint must name one field per pill the Artists tab sorts on"
    );
    for field in &asked {
        assert!(
            FIELDS.contains(field),
            "the Artists sort row asks for `{field}`, a field `sort_artists` has no arm for"
        );
    }

    // The two arrays are indexed against each other, so a label without a field
    // reads past the end and sorts by the empty string — a pill that looks live and
    // does nothing.
    assert_eq!(
        labels.matches("@tr(").count(),
        FIELDS.len(),
        "the Artists sort row must carry one `@tr` label per field"
    );

    assert!(
        VIEW.contains("request-sort(f) => { Favorites.request-artist-sort(f); }"),
        "the Artists sort row must forward its pick to Favorites.request-artist-sort"
    );
}

/// A `pure` callback's result is cached until a dependency is dirtied, so the
/// card's `cover` binding has to *read* `covers-generation` for the prewarm's
/// bump to re-run it. Drop the read and everything still builds and still looks
/// right on a warm tier — a freshly-entered tab just never shows a cover.
#[test]
fn the_grid_card_reads_the_covers_generation() {
    let binding =
        GRID.lines().map(str::trim).find(|line| line.starts_with("cover:")).unwrap_or_default();

    assert!(
        binding.contains("root.covers-generation"),
        "entity-card-grid.slint's `cover` binding must read `covers-generation` — otherwise the \
         host's bump can't invalidate a `pure` callback's cached result"
    );
}

/// Both halves have to be wired at every grid mount: the property, so the bump
/// reaches the cards, and the second callback argument, so Rust knows whether
/// the tier is warm enough to decode on. A mount that forgets either one pins
/// its grid at generation 0 — permanently coverless.
///
/// Counted by arity rather than by name: the one-argument form is a live signature
/// elsewhere in the tree, so a mount wired to it builds and only misbehaves on a cold tab.
#[test]
fn every_grid_mount_forwards_the_covers_generation() {
    // The two grid tabs live in their own files under `views/favorites/`; the
    // page itself only mounts them. Concatenated so the counts below stay one
    // arithmetic rather than one per tab.
    let grids: String = [MOST_PLAYED_TAB, ARTISTS_TAB].concat();
    let mounts = grids.matches("EntityCardGrid {").count();
    assert!(mounts > 0, "the Favorites tabs must still mount an `EntityCardGrid` each");

    assert_eq!(
        grids.matches("covers-generation: Favorites.covers-generation;").count(),
        mounts,
        "every `EntityCardGrid` mount must forward `Favorites.covers-generation`"
    );

    let forwards = grids
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("request-cover("))
        .filter(|line| line.split_once("=>").is_some_and(|(params, _)| params.contains(',')))
        .count();
    assert_eq!(
        forwards, mounts,
        "every `EntityCardGrid` mount must wire a `request-cover` that takes the generation \
         alongside the path and passes it on"
    );
}

/// A leave keeps the picture and drops the pixels. Cleared instead, the re-entry paints the
/// fallback glyph on every card for as long as the re-decode takes, which is the whole reason
/// the leave shrinks rather than releases.
#[test]
fn a_leave_keeps_a_proxy_of_what_the_grid_painted() -> TestResult {
    let (_tmp, path) = write_test_png(512)?;
    let path_str = path.to_str().ok_or("temp path is not UTF-8")?;

    crate::ui::grid_prewarm::prewarm(std::slice::from_ref(&path));
    let warm = crate::ui::grid_prewarm::grid_cover(path_str).size().width;
    assert!(warm > 0, "a prewarm must leave its covers in the tier");

    crate::ui::grid_prewarm::hand_back_covers();
    let proxy = crate::ui::grid_prewarm::grid_cover(path_str).size().width;
    assert!(
        proxy > 0 && proxy < warm,
        "a leave must leave a smaller picture behind, not an empty one \
         (warm {warm}, after the leave {proxy})"
    );
    Ok(())
}

/// Each tab's load is armed wherever the list it fills is emptied and disarmed
/// wherever it is filled — the `RecentlyPlayedUi::grid_dirty` discipline, one
/// flag per tab.
///
/// Three obligations, each a bug on the way in. The loading tab **consumes**
/// its flag (seeded `true`, so a boot onto a tab otherwise pays for its own
/// load twice) and marks the *others*, which is the whole record that a tick
/// went by. The section wipe re-arms every tab beside the lists it just
/// cleared, rather than leaving it to the leave's own `mark_dirty` two files
/// away. And each load re-arms its own on a way of storing nothing, because
/// the tab pick consumes the flag *before* spawning, so nothing else would.
#[test]
fn each_gated_fetch_is_armed_beside_the_cache_it_fills() {
    const LIFECYCLE: &str = include_str!("../callbacks/lifecycle.rs");
    const HANDLE: &str = include_str!("../mod.rs");
    const GRIDS: &str = include_str!("../grids/mod.rs");

    let kick = LIFECYCLE
        .split_once("async fn kick_full_refresh(")
        .and_then(|(_, rest)| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .unwrap_or_default();
    assert!(
        kick.contains("fav_ui.take_tab_dirty(tab);"),
        "`kick_full_refresh` must consume the mounted tab's flag — leaving it set makes the \
         next pick reload a list this call just filled"
    );
    assert!(
        kick.contains("fav_ui.mark_tab_dirty(tab);"),
        "and it must mark the tabs it skipped, or the pick that mounts one has no record that \
         a tick went by"
    );

    let release = HANDLE
        .split_once("pub fn release_section_state(")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map(|(body, _)| body)
        .unwrap_or_default();
    assert!(
        release.contains("self.mark_tab_dirty(tab);"),
        "`release_section_state` must re-arm every tab beside the lists it wipes"
    );

    // `refresh_tracks` asks the section guard twice — before and after its
    // cover prewarm — `requery_tracks` re-arms on its own failed query, and
    // `show_tracks`, which both write through, on a write the tab or the
    // section overtook: five marks in all.
    assert_eq!(
        SONGS.matches("mark_songs_dirty()").count(),
        5,
        "the Songs loads must re-arm on both failed queries, both section bails and a \
         refused write"
    );
    // A grid's load that fails re-arms its own tab through `load_failed`; a leave
    // landing mid-flight is the release's, which marks every tab.
    assert_eq!(
        GRIDS.matches("fn load_failed(").count(),
        2,
        "each grid tab must re-arm its own flag on a failed load"
    );
}

/// A tab pick clears the needle the three tabs share, so a tab left filtered holds ids the next
/// visit doesn't show, and owes a reload. Unfiltered, the list is already what the pick will
/// show.
#[test]
fn only_a_tab_left_filtered_owes_a_reload() {
    const CASES: [(FavoritesTab, &str, bool, &str); 4] = [
        (FavoritesTab::Songs, "ha", true, "Songs, filtered"),
        (FavoritesTab::Songs, "", false, "Songs, unfiltered"),
        (FavoritesTab::Songs, "   ", false, "Songs, a needle of spaces, which trims to nothing"),
        (FavoritesTab::Artists, "ha", true, "a grid tab, filtered"),
    ];
    for (tab, filter, expected, case) in CASES {
        let fav_ui = FavoritesUi::new(Arc::new(CoverThumbs::new()), None, Scrolls::default());
        fav_ui.set_active_tab(tab);
        super::set_filter(&fav_ui, filter);

        let owed = fav_ui.shows_filtered();

        assert_eq!(owed, expected, "{case}");
    }
}
