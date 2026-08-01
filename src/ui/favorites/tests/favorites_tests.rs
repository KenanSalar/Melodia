use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use super::{FavoritesTab, FavoritesUi};
use crate::entities::artist::FavoriteArtist;
use crate::media::cover_thumbs::CoverThumbs;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A solid-colour square PNG in a fresh temp dir; the dir is returned so the
/// caller can keep it alive.
fn write_test_png() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(512, 512, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/curated.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/favorites-view.slint");
const GRID: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/entity-card-grid.slint");

/// The tab count Slint declares today. Kept local so a change to
/// `Favorites.tab-count` doesn't silently rewrite what this asserts.
const TABS: usize = 3;

/// The `N` in `Favorites`'s `tab-count: N;`.
fn declared_tab_count() -> Option<usize> {
    GLOBAL
        .split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
}

/// The body of an inline `name: [ … ];` array literal in `favorites-view.slint`.
fn array_body(marker: &str) -> Option<&'static str> {
    VIEW.split_once(marker)
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
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

/// The header row is drawn from `page-w` for one frame before the first layout
/// reports the truth, and that seed has to be the row's own floor rather than a
/// plausible page width. Seeded wide, the bar believes it can afford full-width
/// tabs, draws them into a panel that can't seat them, and they spill under the
/// search bar — which is what a miniplayer → full swap reliably produces. Same
/// contract `settings-view.slint` carries; a literal reads as harmless to
/// anyone who hasn't seen it fail, so pin that it stays derived.
#[test]
fn the_page_width_seed_is_the_rows_floor() {
    let seed = VIEW
        .split_once("property <length> page-w:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value);

    assert!(
        seed.contains("compact-w"),
        "favorites-view.slint's `page-w` seed must be the header row's floor, derived from the \
         bar's own `compact-w` — not a plausible page width"
    );
}

/// The tab bodies sit inside the page's own enter transition, so theirs has to
/// stay off until the user actually switches — a horizontal slide composed with
/// the page's fade-up reads as a diagonal on every arrival from the sidebar.
/// `tab-anim-armed` starts `false` and is written by the pick handler; the page
/// is destroyed and rebuilt on every entry, so it re-disarms for free. Seed it
/// `true`, or arm it from a mount timer, and the bug is back and looks like a
/// design choice.
#[test]
fn the_sub_view_slide_is_disarmed_until_the_first_switch() {
    assert!(
        VIEW.contains("property <bool> tab-anim-armed: false;"),
        "favorites-view.slint's `tab-anim-armed` must start false — the page's own entrance is \
         the only thing that should move when it arrives"
    );

    let handler = VIEW
        .split_once("selected(i) =>")
        .and_then(|(_, rest)| rest.split_once("Favorites.tab-changed(i);"))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("root.tab-anim-armed = true;"),
        "the tab bar's `selected` handler must arm the slide — nothing else can tell a real \
         switch from the page mounting"
    );
    assert!(
        handler.contains("root.tab-enter-from ="),
        "the tab bar's `selected` handler must set the direction *before* the branch flips, the \
         same ordering `nav_transition.rs` follows for the page-level transition"
    );
}

/// A mirrored width only reaches the bar through `changed width`, and `changed`
/// doesn't fire when the first layout settles directly on the final value —
/// which is every window opened at its size. Without the mount timer the seed
/// above is never corrected, and a roomy window draws icon-only tabs until
/// something resizes it. It only looks fixed coming out of the miniplayer,
/// where the floor's answer happens to be the right one.
#[test]
fn the_page_width_mirror_has_a_mount_seed() {
    assert!(
        VIEW.contains("changed width => { self.page-w = self.width; }"),
        "favorites-view.slint must mirror its width imperatively — a live `root.width` read \
         feeding a child's size re-enters layout"
    );

    let timer = VIEW
        .split_once("Timer {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(
        timer.contains("root.page-w = root.width"),
        "favorites-view.slint's mount Timer must re-run the `page-w` mirror — `changed` never \
         fires for a window born at its final size"
    );
}

/// A `pure` callback's result is cached until a dependency is dirtied, so the
/// card's `cover` binding has to *read* `covers-generation` for the prewarm's
/// bump to re-run it. Drop the read and everything still builds and still looks
/// right on a warm tier — a freshly-entered tab just never shows a cover.
#[test]
fn the_grid_card_reads_the_covers_generation() {
    let binding = GRID
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("cover:"))
        .unwrap_or_default();

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
/// Counted by arity rather than by name, because the hero's `CoverMosaic` wires
/// a `request-cover` too and deliberately keeps the one-argument form: its tier
/// is warmed by `refresh_hero`, not by a tab.
#[test]
fn every_grid_mount_forwards_the_covers_generation() {
    let mounts = VIEW.matches("EntityCardGrid {").count();
    assert!(mounts > 0, "favorites-view.slint must still mount at least one `EntityCardGrid`");

    assert_eq!(
        VIEW.matches("covers-generation: Favorites.covers-generation;").count(),
        mounts,
        "every `EntityCardGrid` mount must forward `Favorites.covers-generation`"
    );

    let forwards = VIEW
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

/// Generation 0 means "this tier was cleared when its tab was left", and the
/// lookup must answer from the cache alone — a decode here lands on the UI
/// thread, in the frame that mounts the grid, once per visible card. It is not
/// "return nothing": an entry that survives still comes back, which is what
/// makes a re-entered warm tab paint instantly.
#[test]
fn a_cold_generation_serves_the_cache_without_decoding() -> TestResult {
    let cap = NonZeroUsize::new(4).ok_or("cap must be > 0")?;
    let thumbs = CoverThumbs::with_config(64, cap);
    let (_tmp, path) = write_test_png()?;
    let path = path.to_str().ok_or("temp path is not UTF-8")?;

    assert_eq!(
        super::grid_cover(&thumbs, path, 0).size().width,
        0,
        "a cold tier must hand back a placeholder rather than decode on the UI thread"
    );
    assert_eq!(
        super::grid_cover(&thumbs, path, 1).size().width,
        64,
        "a warmed tier must decode on miss, so rows scrolled to later still get covers"
    );
    assert_eq!(
        super::grid_cover(&thumbs, path, 0).size().width,
        64,
        "generation 0 gates the *decode*, not the lookup — a cached cover still resolves"
    );
    Ok(())
}

/// The decode outlives a fast section leave, and `release_section_state` is
/// spawned on that leave — it can easily finish first, so a prewarm that
/// ignored it would refill the tier that release just emptied and hold a
/// screenful of 448 px buffers behind a view nobody can see, until the next
/// leave. The check has to sit after the decode: before it, the leave hasn't
/// happened yet, which is the whole problem.
#[test]
fn a_prewarm_outliving_the_leave_keeps_nothing() -> TestResult {
    let (_tmp, path) = write_test_png()?;
    let path = path.to_str().ok_or("temp path is not UTF-8")?;

    let fav_ui = FavoritesUi::new(Arc::new(CoverThumbs::new()));
    fav_ui.set_active_tab(FavoritesTab::Artists);
    *fav_ui.state().fav_artists.lock() = vec![FavoriteArtist {
        id: 1,
        name: "Artist".to_owned(),
        image_path: Some(path.to_owned()),
        favorite_count: 2,
    }];

    fav_ui.set_section_active(true);
    fav_ui.prewarm_tab_covers(FavoritesTab::Artists);
    assert!(
        fav_ui.artist_cover(path, 0).size().width > 0,
        "a prewarm for a section still on screen must leave its covers in the tier"
    );

    fav_ui.artist_thumbs.clear();
    fav_ui.set_section_active(false);
    fav_ui.prewarm_tab_covers(FavoritesTab::Artists);
    assert_eq!(
        fav_ui.artist_cover(path, 0).size().width,
        0,
        "a prewarm that landed after the section leave must hand its buffers back"
    );
    Ok(())
}
