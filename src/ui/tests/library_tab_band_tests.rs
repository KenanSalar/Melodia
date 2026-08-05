//! Source-level pins on `melodia-ui/ui/components/hero/library-tab-band.slint`.
//!
//! The My Library page's band. Six of the pins below are the header-row fixes it
//! ports verbatim from `MosaicTabHero` — each one paid for once, invisible in
//! review, and exactly what a copy loses — so they are worded the same way as
//! their twins in `mosaic_tab_hero_tests.rs`. The remaining five are this band's
//! own, and all five are about the morph: a band that changes height, colour and
//! contents is a band with five new ways to be quietly wrong.
//!
//! They live here rather than under a host for the `tab_bar_tests.rs` reason —
//! no Rust module owns the file.

const BAND: &str =
    include_str!("../../../melodia-ui/ui/components/hero/library-tab-band.slint");

/// The file with its comment lines dropped, so prose about a fix can neither
/// satisfy a pin nor bound a region early. The `placeholder_tests.rs` helper.
fn code() -> String {
    BAND.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The value of a `name:` binding, up to its terminating `;`.
fn binding(src: &str, name: &str) -> String {
    src.split_once(name)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or(String::new(), |(value, _)| value.to_owned())
}

/// The header row is drawn from `page-w` for one frame before the first layout
/// reports the truth, and that seed has to be the row's own floor rather than a
/// plausible page width. Seeded wide, the bar believes it can afford full-width
/// tabs, draws them into a panel that can't seat them, and they spill under the
/// search bar — which is what a miniplayer → full swap reliably produces.
#[test]
fn the_page_width_seed_is_the_rows_floor() {
    let seed = binding(BAND, "property <length> page-w:");

    assert!(
        seed.contains("compact-w"),
        "the band's `page-w` seed must be the header row's floor, derived from the bar's own \
         `compact-w` — not a plausible page width"
    );
}

/// A mirrored width only reaches the bar through `changed width`, and `changed`
/// doesn't fire when the first layout settles directly on the final value —
/// which is every window opened at its size. Without the mount timer the seed
/// above is never corrected, and a roomy window draws icon-only tabs until
/// something resizes it.
#[test]
fn the_page_width_mirror_has_a_mount_seed() {
    assert!(
        BAND.contains("changed width => { self.page-w = self.width; }"),
        "the band must mirror its width imperatively — a live `root.width` read feeding a \
         child's size re-enters layout"
    );

    let timer = BAND
        .split_once("Timer {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(
        timer.contains("root.page-w = root.width"),
        "the band's mount Timer must re-run the `page-w` mirror — `changed` never fires for a \
         window born at its final size"
    );
}

/// The two ends of the header budget read published floors rather than restating
/// them, so whatever a component stops asking for is what the row stops handing
/// over. A literal on either side looks identical and silently decouples the two.
#[test]
fn the_header_budget_reserves_against_published_floors() {
    assert!(
        BAND.contains("property <length> search-w-min: search.min-w;"),
        "the band must take the input's floor off `SearchBar.min-w`"
    );
    let budget = binding(BAND, "property <length> search-w: clamp(");
    assert!(
        budget.contains("bar.compact-w"),
        "the search slot must be budgeted against the bar's own `compact-w` — the tabs are what \
         it has to leave room for, and a restated `5 * 48px` drifts the moment a tab is added"
    );
}

/// Unlike the mosaic band, this bar sits on two different surfaces: `Theme.mantle`
/// idle, the entity's solved blur with a detail open. So each of its four brushes
/// is a *pair*, and dropping either half is a bug that only shows in one state —
/// a theme token left on the hero arm washes out over a cover, and a hero tier
/// left on the idle arm paints the previous entity's colours onto a flat pane.
///
/// `active-color` is the one this exists for: it drives the selected label, its
/// FILL=1 icon and the underline from one input, so an omission is three
/// surfaces at once, and `Theme.accent` carries no contrast floor against a blur
/// (Latte's mauve lands near 1.7:1, under even the 3:1 non-text bar) where
/// `HeroBackdrop.chrome` is solved to clear it.
///
/// Asserted as "reads *some* `HeroBackdrop` tier" rather than pinning which one:
/// the tier a brush should take is a design call that may move, but reaching for
/// `Theme.*` on the hero side is a bug at any tier.
#[test]
fn every_tab_bar_brush_crosses_from_a_theme_token_to_a_backdrop_tier() {
    let code = code();

    for (prop, input) in [
        ("label-color", "label-brush"),
        ("active-color", "active-brush"),
        ("hover-fill", "hover-brush"),
        ("divider-color", "divider-brush"),
    ] {
        let pair = binding(&code, &format!("property <brush> {input}:"));
        assert!(
            !pair.is_empty(),
            "the band no longer declares `{input}` — the bar would fall back to the component's \
             `Theme.*` default on both surfaces"
        );
        assert!(
            pair.contains("HeroBackdrop."),
            "`{input}` must take a `HeroBackdrop` tier with a detail open — a theme value answers \
             a question about the *page* background, and under a hero there isn't one"
        );
        assert!(
            pair.contains("Theme."),
            "`{input}` must take a `Theme.*` token in idle — the band is a flat pane there, and a \
             solved hero tier is the previous entity's colour"
        );
        assert!(
            code.contains(&format!("{prop}: root.{input};")),
            "the band's TabBar must pass `{prop}` the `{input}` pair rather than a token directly"
        );
    }
}

/// The tab bodies sit inside the page's own enter transition, so theirs has to
/// stay off until the user actually switches — a horizontal slide composed with
/// the page's fade-up reads as a diagonal on every arrival from the sidebar.
/// `tab-anim-armed` starts `false` and is written by the pick handler; the page
/// is destroyed and rebuilt on every entry, so it re-disarms for free.
#[test]
fn the_sub_view_slide_is_disarmed_until_the_first_switch() {
    assert!(
        BAND.contains("out property <bool> tab-anim-armed: false;"),
        "the band's `tab-anim-armed` must start false — the page's own entrance is the only thing \
         that should move when it arrives"
    );

    let handler = BAND
        .split_once("selected(i) =>")
        .and_then(|(_, rest)| rest.split_once("root.tab-selected(i);"))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("root.tab-anim-armed = true;"),
        "the tab bar's `selected` handler must arm the slide — nothing else can tell a real switch \
         from the page mounting"
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
        !BAND.contains("Tooltip {"),
        "the band must not mount the tooltip itself — the host's body paints over anything \
         declared here"
    );
    for prop in ["tip-x", "tip-y", "tip-w", "tip-h", "tip-label", "tip-visible"] {
        assert!(
            BAND.contains(&format!("out property <length> {prop}:"))
                || BAND.contains(&format!("out property <string> {prop}:"))
                || BAND.contains(&format!("out property <bool> {prop}:")),
            "the band must publish `{prop}` for its host's tooltip frame"
        );
    }
    let tip_x = binding(BAND, "out property <length> tip-x:");
    assert!(
        tip_x.contains("bar.tab-w * bar.hovered-idx"),
        "`tip-x` must be derived from the hovered *index* — equal-width cells are what make that \
         possible, and a snapshotted rect goes stale the moment anything resizes the bar"
    );
}

/// **The one that costs most to get wrong.** Slint reports a component root's
/// bound dimension as both `min` and `max`, so an animated `height` here would
/// put the *window's* own minimum height on the morph: dragging the bottom edge
/// inward chases a floor that is itself still easing, and it stutters. That is
/// `tab-bar.slint`'s width bug, one axis over, and it looks identical in source.
///
/// The split buys that freedom by letting the element be drawn shorter than it
/// asked for, so the clip is not decoration either: on the shrink leg the hero
/// contents are still full height while the band is already compact, and without
/// it they paint out of the band and into the body underneath.
#[test]
fn the_band_negotiates_its_height() {
    let code = code();

    for constraint in [
        "min-height: root.compact-h;",
        "preferred-height: root.compact-h + (root.hero-h - root.compact-h) * root.hero-t;",
        "max-height: root.hero-h;",
    ] {
        assert!(
            code.contains(constraint),
            "the band's root must spell `{constraint}` — the three-way split is what keeps an \
             animated height off the window's own resize floor"
        );
    }
    assert!(
        !code.contains("\n    height:"),
        "the band's root must not bind `height` — Slint reports a bound root dimension as both \
         `min` and `max`, so the window's minimum height would ease with the morph"
    );
    assert!(
        code.contains("\n    clip: true;"),
        "the band's root must clip — the min/preferred/max split lets it be drawn shorter than it \
         asked for, and the hero contents would paint into the body below"
    );
}

/// `hero-t` is **seeded** by its binding and **owned** by the `changed` handler.
/// Both halves are load-bearing and each fails differently. An animated *binding*
/// restarts whenever a dependency is marked dirty rather than when its value
/// changes, so left bound the morph would re-base every time a detail id moved
/// under it. Dropping the seed and writing from a mount `Timer` instead is the
/// other failure: a page re-entered with a detail already open would grow into
/// the hero on every arrival, because the first evaluation no longer lands in
/// `NotAnimating`.
#[test]
fn the_morph_progress_is_seeded_by_its_binding_and_written_by_its_handler() {
    assert!(
        BAND.contains("property <float> hero-t: root.detail-open ? 1.0 : 0.0;"),
        "`hero-t` must keep its binding as the seed — without it the first evaluation animates and \
         a page entered on an open detail grows into the hero every time"
    );

    let handler = BAND
        .split_once("changed detail-open =>")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("self.hero-t ="),
        "`hero-t` must be written from `changed detail-open` — an animated binding restarts on \
         dependency dirtiness, not on a value change"
    );
}

/// The back button sits *in the tab row*, beside labels already painted in hero
/// tiers, rather than floating over a full-bleed hero the way `DetailHeader`'s
/// did. So it takes the `MetaChip` pair and not `Theme.floating-chrome-bg`: that
/// token answers a question about the theme's own surface ladder, which is not
/// what this glyph contrasts against.
#[test]
fn the_back_button_takes_both_brushes_from_the_backdrop() {
    let button = code()
        .split_once("icon: \"arrow_back\";")
        .and_then(|(_, rest)| rest.split_once("clicked =>"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!button.is_empty(), "the band no longer mounts a back button ahead of its click handler");

    for (prop, tier) in [("idle-bg", "HeroBackdrop.chip-fill"), ("idle-fg", "HeroBackdrop.chrome")] {
        assert!(
            button.contains(&format!("{prop}: {tier};")),
            "the back button must take `{prop}` from `{tier}` — it paints on the solved band, and \
             a theme token has no contrast floor there"
        );
    }
}

/// The idle pane is full-bleed and animating for the whole morph, which is
/// exactly the shape that must not use `opacity`: an element with a non-unit
/// opacity renders to an offscreen layer, so this one would cost a
/// window-width layer every frame of every drill-in. Folding the alpha into the
/// brush is one blend instead.
#[test]
fn the_idle_pane_folds_its_alpha_into_the_brush() {
    let pane = code()
        .split_once("idle-pane := Rectangle {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!pane.is_empty(), "the band no longer declares `idle-pane`");

    assert!(
        pane.contains("background: Theme.mantle.with-alpha(1.0 - root.hero-t);"),
        "the idle pane must fade by alpha inside its brush"
    );
    assert!(
        !pane.contains("opacity:"),
        "the idle pane must not use `opacity` — a full-bleed element with a non-unit opacity costs \
         an offscreen layer for the length of the morph"
    );
}

/// Data-agnostic, the `MosaicTabHero` contract: `@tr` folds msgids at codegen, so
/// a string spelled here is one the host can no longer vary per tab — and the
/// five tabs differ in every one of them. It is also what keeps the catalogue
/// surface at the host, where `ui::locale::tests` already walks it.
#[test]
fn the_band_states_no_string_of_its_own() {
    assert!(
        !code().contains("@tr("),
        "the band must take every literal as a property — the five tabs differ in all of them, and \
         a msgid folded here can't follow the mounted tab"
    );
}
