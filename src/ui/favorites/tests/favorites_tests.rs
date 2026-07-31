const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/curated.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/favorites-view.slint");

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
