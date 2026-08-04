//! Source-level pins on `melodia-ui/ui/components/hero/mosaic-tab-hero.slint`.
//!
//! The banner Favorites and Recently Played share. Each thing below is a fix
//! that was paid for once and is invisible in review — which is the whole reason
//! the band is one file rather than two — so they live beside it here rather
//! than under either host, the `tab_bar_tests.rs` arrangement for the same
//! reason.

const HERO: &str = include_str!("../../../melodia-ui/ui/components/hero/mosaic-tab-hero.slint");

/// The header row is drawn from `page-w` for one frame before the first layout
/// reports the truth, and that seed has to be the row's own floor rather than a
/// plausible page width. Seeded wide, the bar believes it can afford full-width
/// tabs, draws them into a panel that can't seat them, and they spill under the
/// search bar — which is what a miniplayer → full swap reliably produces. Same
/// contract `settings-view.slint` carries; a literal reads as harmless to
/// anyone who hasn't seen it fail, so pin that it stays derived.
#[test]
fn the_page_width_seed_is_the_rows_floor() {
    let seed = HERO
        .split_once("property <length> page-w:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value);

    assert!(
        seed.contains("compact-w"),
        "the hero band's `page-w` seed must be the header row's floor, derived from the bar's own \
         `compact-w` — not a plausible page width"
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

/// The two ends of the header budget read published floors rather than restating
/// them, so whatever a component stops asking for is what the row stops handing
/// over. A literal on either side looks identical and silently decouples the two.
#[test]
fn the_header_budget_reserves_against_published_floors() {
    assert!(
        HERO.contains("property <length> search-w-min: search.min-w;"),
        "the hero band must take the input's floor off `SearchBar.min-w`"
    );
    let budget = HERO
        .split_once("property <length> search-w: clamp(")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value);
    assert!(
        budget.contains("bar.compact-w"),
        "the search slot must be budgeted against the bar's own `compact-w` — the tabs are what \
         it has to leave room for, and a restated `5 * 48px` drifts the moment a tab is added"
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
    let mount = HERO
        .split_once("bar := TabBar {")
        .and_then(|(_, rest)| rest.split_once("selected(i) =>"))
        .map_or("", |(body, _)| body);
    assert!(
        !mount.is_empty(),
        "the hero band no longer mounts `bar := TabBar` ahead of its `selected` handler"
    );

    for prop in ["label-color", "active-color", "hover-fill", "divider-color"] {
        assert!(
            mount.contains(&format!("{prop}: HeroBackdrop.")),
            "the hero band's TabBar must pass `{prop}` a `HeroBackdrop` tier — omitting it falls \
             back to the component's `Theme.*` default, which is a theme value on a band that is \
             no longer theme-seeded"
        );
    }
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
        HERO.contains("out property <bool> tab-anim-armed: false;"),
        "the hero band's `tab-anim-armed` must start false — the page's own entrance is the only \
         thing that should move when it arrives"
    );

    let handler = HERO
        .split_once("selected(i) =>")
        .and_then(|(_, rest)| rest.split_once("root.tab-selected(i);"))
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
         *before* it hands the pick out — the same ordering `nav_transition.rs` follows for the \
         page-level transition"
    );
}

/// The compact-mode tooltip is anchored by the *host*, after its scroll body,
/// because Slint paints in declaration order and anything this band owns is
/// covered by the content below it. So the band publishes the rect instead —
/// derived from the hovered index rather than snapshotted, which is what keeps a
/// tab that moves under a parked pointer (Ctrl+B, F11) anchored to its tooltip.
#[test]
fn the_band_publishes_its_tooltip_anchor_rather_than_drawing_one() {
    assert!(
        !HERO.contains("Tooltip {"),
        "the hero band must not mount the tooltip itself — the host's scroll body paints over \
         anything declared here"
    );
    for prop in ["tip-x", "tip-y", "tip-w", "tip-h", "tip-label", "tip-visible"] {
        assert!(
            HERO.contains(&format!("out property <length> {prop}:"))
                || HERO.contains(&format!("out property <string> {prop}:"))
                || HERO.contains(&format!("out property <bool> {prop}:")),
            "the hero band must publish `{prop}` for its host's tooltip frame"
        );
    }
    let tip_x = HERO
        .split_once("out property <length> tip-x:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value);
    assert!(
        tip_x.contains("bar.tab-w * bar.hovered-idx"),
        "`tip-x` must be derived from the hovered *index* — equal-width cells are what make that \
         possible, and a snapshotted rect goes stale the moment anything resizes the bar"
    );
}
