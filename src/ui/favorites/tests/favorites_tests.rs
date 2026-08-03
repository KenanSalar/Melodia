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
const MOST_PLAYED_TAB: &str =
    include_str!("../../../../melodia-ui/ui/views/favorites/most-played-tab.slint");
const ARTISTS_TAB: &str =
    include_str!("../../../../melodia-ui/ui/views/favorites/artists-tab.slint");
const GRID: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/entity-card-grid.slint");
const SONGS: &str = include_str!("../songs.rs");
const SONGS_TAB: &str = include_str!("../../../../melodia-ui/ui/views/favorites/songs-tab.slint");
const SUBVIEWS: &str = include_str!("../../callbacks/favorites/subviews.rs");

/// `Favorites.tracks` feeds one element in the whole tree, under the Songs
/// tab's `if` — so off that tab, every prepared row the Songs path builds
/// reaches nothing and every row it leaves in the model is pinned behind a view
/// nobody can see.
///
/// Four things hold that, and each fails differently. `build_filtered_tracks`'
/// bail is what skips the cost; drop it and the tab gate becomes decorative,
/// with the whole favourites list still prepared per keystroke on the UI thread.
/// `write_filtered_tracks`' is what survives a pick landing mid-post; drop it
/// and the retention comes back on exactly the race the first one can't see. The
/// tab-leave **clear** is what empties what the last visit left; drop it and the
/// rows simply stay. And the tab pick writing through the **non-hopping**
/// `apply_filtered_tracks_now` is what stops that clear becoming visible: the
/// entering tab mounts on the next frame, and a posted write that loses the race
/// paints a `TrackList` of headers over nothing.
///
/// None of the four shows on screen except the last, and that one only for a
/// frame — which is why they are pinned rather than trusted.
#[test]
fn the_songs_model_is_written_only_while_its_tab_is_mounted() {
    assert!(
        SONGS_TAB.contains("rows: Favorites.tracks;"),
        "songs-tab.slint is the sole reader of `Favorites.tracks` — if that moved, this gate needs re-deriving"
    );

    let build = SONGS
        .split_once("fn build_filtered_tracks")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    assert!(
        build.contains("if fav_ui.active_tab() != FavoritesTab::Songs {"),
        "`build_filtered_tracks` must bail before the prepare walk when Songs isn't mounted"
    );

    let write = SONGS
        .split_once("fn write_filtered_tracks")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    assert!(
        write.contains("|| fav_ui.active_tab() != FavoritesTab::Songs"),
        "`write_filtered_tracks` must re-check the tab — a pick can land while the post is in flight"
    );

    assert!(
        SUBVIEWS.contains(r#"clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "favorites: leave songs tab")"#),
        "leaving the Songs tab must empty `Favorites.tracks`, the way `write_filtered_grids` empties the grid it isn't mounting"
    );
    assert!(
        SUBVIEWS.contains("apply_filtered_tracks_now(&ui, &fu)"),
        "the tab pick must write without hopping the event loop, or the clear above shows as a bare list"
    );
}

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
    // Pinned down to the operand: the direction has to come off the bar's own
    // `previous-index`, since `tab-idx` and everything bound to it already read
    // the tab just picked. A local mirror reintroduced here would compare `i`
    // against `i` and enter from the left every time.
    assert!(
        handler.contains("root.tab-enter-from = i > bar.previous-index"),
        "the tab bar's `selected` handler must set the direction from `bar.previous-index`, and \
         *before* the branch flips — the same ordering `nav_transition.rs` follows for the \
         page-level transition"
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

/// `TabBar`'s four brushes all default to `Theme.*` tokens, which is right for
/// Settings and wrong on a banner — and a mount that omits one still builds and
/// still looks correct in Settings, so nothing else catches it.
///
/// `active-color` is the one this exists for. It was left at the default long
/// after the other three moved, and it drives the selected label, its FILL=1
/// icon *and* the underline from one input, so the omission is three surfaces
/// at once. Two things make it wrong rather than merely inconsistent: the band
/// takes its hue from the mosaic now, and a theme accent has no contrast floor
/// against it — Latte's mauve lands near 1.7:1 on the pinned band, under even
/// the 3:1 non-text bar, where `HeroBackdrop.chrome` is solved to clear it.
///
/// Asserted as "reads *some* `HeroBackdrop` tier" rather than pinning which
/// one: the tier a brush should take is a design call that may move, but
/// reaching for `Theme.*` here is a bug at any tier.
#[test]
fn the_hero_tab_bar_takes_every_brush_from_the_backdrop() {
    let mount = VIEW
        .split_once("bar := TabBar {")
        .and_then(|(_, rest)| rest.split_once("selected(i) =>"))
        .map_or("", |(body, _)| body);
    assert!(
        !mount.is_empty(),
        "favorites-view.slint no longer mounts `bar := TabBar` ahead of its `selected` handler"
    );

    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            mount.contains(&format!("{prop}: HeroBackdrop.")),
            "the Favorites hero's TabBar must pass `{prop}` a `HeroBackdrop` tier — omitting it \
             falls back to the component's `Theme.*` default, which is a theme value on a band \
             that is no longer theme-seeded"
        );
    }
}

/// Every field a sort pill can ask for has to be one the comparator handles.
///
/// The token is a bare string on both sides — `request-artist-sort("name")` in
/// the Slint, a `match` arm in `grids::sort::sort_artists` — so a typo or a rename
/// on either side compiles, and the pill just quietly sorts by the default arm
/// while painting its arrow as though it had worked. Nothing pins this for the
/// Albums / Artists / Genres rows, which is exactly why it's worth pinning here.
///
/// Also asserts the pills carry `reserve-sort-slot`, without which there is no
/// arrow slot and the active field is indicated by colour alone.
#[test]
fn every_sort_pill_asks_for_a_field_the_comparator_knows() {
    // The arms of `grids::sort::sort_artists`, restated. A field dropped there and
    // left here fails the round-trip below; one added here and not there falls
    // through to the default and this test keeps passing — which is fine, since
    // the default is a defined order, not a no-op.
    const FIELDS: [&str; 2] = ["name", "favorite_count"];

    let asked: Vec<&str> = VIEW
        .match_indices("Favorites.request-artist-sort(\"")
        .filter_map(|(i, m)| VIEW[i + m.len()..].split_once('"').map(|(field, _)| field))
        .collect();

    assert_eq!(
        asked.len(),
        FIELDS.len(),
        "favorites-view.slint must mount one sort pill per field the Artists tab sorts on"
    );
    for field in asked {
        assert!(
            FIELDS.contains(&field),
            "`request-artist-sort(\"{field}\")` names a field `sort_artists` has no arm for"
        );
    }

    // Counted against the pills rather than asserted once: a row where only the
    // first pill reserves the slot has its labels jump sideways as the active
    // field moves.
    assert_eq!(
        VIEW.matches("sort-direction: Favorites.artist-sort-field ==").count(),
        FIELDS.len(),
        "every Artists-tab sort pill must bind `sort-direction` to the active field"
    );
    assert_eq!(
        VIEW.matches("reserve-sort-slot: true;").count(),
        FIELDS.len(),
        "every Artists-tab sort pill must reserve the arrow slot"
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
        super::covers::grid_cover(&thumbs, path, 0).size().width,
        0,
        "a cold tier must hand back a placeholder rather than decode on the UI thread"
    );
    assert_eq!(
        super::covers::grid_cover(&thumbs, path, 1).size().width,
        64,
        "a warmed tier must decode on miss, so rows scrolled to later still get covers"
    );
    assert_eq!(
        super::covers::grid_cover(&thumbs, path, 0).size().width,
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
