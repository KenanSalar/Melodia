//! Source-level pins on `melodia-ui/ui/components/hero/library-tab-band.slint`.
//!
//! The My Library page's band. Six of the pins below are the header-row fixes it
//! ports verbatim from `MosaicTabHero` — each one paid for once, invisible in
//! review, and exactly what a copy loses — so they are worded the same way as
//! their twins in `mosaic_tab_hero_tests.rs`. The rest are this band's own: five
//! about the morph, because a band that changes height, colour and contents is a
//! band with five new ways to be quietly wrong, and three about the **seam** with
//! its mount sheet, because a band nobody drives passes all eight of the others.
//!
//! They live here rather than under a host for the `tab_bar_tests.rs` reason —
//! no Rust module owns the file.

const BAND: &str =
    include_str!("../../../melodia-ui/ui/components/hero/library-tab-band.slint");
/// The band's only mount. Three of the pins below are about the seam rather than
/// the component, and a band nobody feeds passes every check on its own source.
const SHEET: &str = include_str!("../../../melodia-ui/ui/views/my-library-view.slint");

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

    // Anchored on the *anonymous* `Timer`, since the band has a second one now — the
    // named `collapse :=` that defers the hero teardown.
    let timer = BAND
        .split_once("\n    Timer {")
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

/// Unlike the mosaic band, this bar sits on two different surfaces: `Theme.base`
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

/// **The column that mounts `HeroChipStrip` is gated on `detail-open`, and moving
/// it onto the morph is a panic rather than a glitch.**
///
/// A dropped Slint repeater instance keeps its memory alive for weak refs, so a
/// `ChangeTracker` sitting inside it stays registered — and
/// `ChangeTracker::evaluate` upgrades its weak handle with an `unwrap`. Whether
/// that ever fires depends on what the tracker *watches*: `Tooltip`'s
/// `changed hovered` reads a property driven from inside its own branch, so it can
/// never go dirty once the branch is gone, which is why the back slot survives on
/// `hero-t`. `MetaChipStrip`'s `changed watched-w` reads a **layout** property, and
/// the surviving parent re-dirties it when it re-flows without the child. Gate that
/// column on `hero-t` and every back out of a detail panics on the frame the morph
/// lands. See `.claude/rules/slint-pitfalls.md`.
#[test]
fn the_chip_bearing_column_is_gated_on_the_detail_and_not_on_the_morph() {
    let code = code();
    let lines: Vec<&str> = code.lines().collect();
    let mount = lines.iter().position(|line| line.contains("HeroChipStrip {"));
    assert!(mount.is_some(), "the band no longer mounts `HeroChipStrip`");
    let mount = mount.unwrap_or_default();

    // The *enclosing* branch, not the nearest `if` — the strip has `if`-gated siblings
    // (the subtitle) at its own depth, and those say nothing about its lifetime.
    let indent = |line: &str| line.len() - line.trim_start().len();
    let gate = lines[..mount]
        .iter()
        .rev()
        .find(|line| line.trim_start().starts_with("if root.") && indent(line) < indent(lines[mount]))
        .map_or(String::new(), |line| line.trim().to_owned());
    assert!(
        gate.starts_with("if root.detail-open:"),
        "the column mounting `HeroChipStrip` must be gated on `detail-open`, which only a Rust \
         write moves — found `{gate}`. On an animated predicate the branch is dropped from inside \
         `run_change_handlers`, and the strip's `changed watched-w` tracker then unwraps a dead \
         weak handle."
    );

    assert_eq!(
        code.matches("if root.hero-t > 0:").count(),
        2,
        "the back slot and the artwork tile are the two branches that may ride the morph — \
         neither carries a tracker the parent can re-dirty after the drop"
    );
}

/// The hero's teardown rides the *end* of the collapse. Every fact the band paints
/// is a ternary over the detail id at the mount sheet, so releasing the cover, the
/// blur pair, the shared backdrop tiers and the chip row when that id clears left
/// the band spending the whole morph collapsing a fallback glyph over a reset
/// gradient — an exit *from* a placeholder. The `Dialog.closed()` shape, for the
/// same reason it has it.
#[test]
fn the_collapse_defers_the_hero_teardown() {
    let timer = BAND
        .split_once("collapse := Timer {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(!timer.is_empty(), "the band no longer declares its `collapse` timer");

    assert!(
        timer.contains("interval: Theme.dur-spatial;"),
        "the collapse timer must run the morph's own duration — a shorter one tears the hero down \
         mid-fade, a longer one holds its buffers past the point anyone can see them"
    );
    assert!(
        timer.contains("root.hero-collapsed();"),
        "the collapse timer is what fires `hero-collapsed` — nothing else knows the morph is done"
    );
    assert!(
        timer.contains("running: false;"),
        "the collapse timer must start idle: a page mounted with no detail never transitioned into \
         one and has nothing to hand back"
    );

    let handler = BAND
        .split_once("changed detail-open =>")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("collapse.running = !self.detail-open;"),
        "the same edge that drives the morph must arm *and* cancel the timer, so a re-drill inside \
         the window can't have the previous hero's teardown land on the new one"
    );

    assert!(
        SHEET.contains("hero-collapsed => { MyLibrary.hero-collapsed(); }"),
        "the sheet must forward `hero-collapsed` — unforwarded, the band collapses correctly and \
         the hero it was painting is never released"
    );
}

/// The back slot's trailing gap is **its own width**, not the row's `spacing`. As
/// layout spacing the row loses a whole `pad-md` on the frame the slot unmounts,
/// and on the way out that frame is the last one — after the height and every
/// brush have settled — so it reads as the tab bar jumping left. Opening, the same
/// step lands on frame one under 400 ms of movement, which is why it only ever
/// showed one way.
#[test]
fn the_back_slot_carries_its_own_gap() {
    let code = code();

    assert!(
        binding(&code, "property <length> back-slot-w:").contains("Theme.pad-md"),
        "the back slot must fold its trailing gap into its own eased width"
    );
    assert!(
        !code.contains("back-slot-gap"),
        "the stepped `back-slot-gap` must be gone — it *is* the jump, and a width that folds the \
         gap in makes the whole inset continuous"
    );
    assert!(
        code.contains("spacing: 0px;"),
        "the header row must hand out no spacing of its own, or the slot's width can't reach zero"
    );
    assert!(
        code.contains("min-width: 2 * Theme.pad-md;"),
        "the bar-to-input clearance must survive as the spacer's own floor — it is the two \
         `pad-md`s the `search-w` budget reserves"
    );
}

/// The count line rides the band's lower edge, which is the meta row's floor in
/// both states — so it follows the animated height with no anchor to interpolate,
/// exactly as the pill row beside it does. Centred in the meta row instead, it was
/// laid out against that row's own floor — the 140 px artwork tile — so the
/// collapse put it below the shrinking band and clipped it away, then snapped it
/// back the frame the tile unmounted.
#[test]
fn the_count_line_rides_the_bands_lower_edge() {
    let code = code();
    let floating = code
        .split_once("alignment: space-between;")
        .map_or(String::new(), |(_, rest)| rest.to_owned());
    assert!(
        !floating.is_empty(),
        "the floating slot must keep `space-between`: `@children` is whatever the host hands over, \
         and a stretchy spacer only pushes it right if nothing inside it stretches"
    );

    assert!(
        floating.contains("root.count-text"),
        "the count line must sit in the floating slot, beside the pills it already shares a line with"
    );
    assert_eq!(
        code.matches("root.count-text").count(),
        1,
        "the count must be drawn once — a second reader is a second anchor to keep in step"
    );
    assert!(
        floating.contains("opacity: 1.0 - root.hero-t"),
        "the count must fade in with the collapse rather than appear on the frame the hero unmounts"
    );
}

/// The back button sits *in the tab row*, beside labels already painted in hero
/// tiers, rather than floating over a full-bleed hero the way `DetailHeader`'s
/// did. So it takes the `MetaChip` pair and not `Theme.floating-chrome-bg`: that
/// token answers a question about the theme's own surface ladder, which is not
/// what this glyph contrasts against.
///
/// `hover-bg` is the half that fails silently, and the reason this covers three
/// brushes rather than two: `IconButton` defaults it to an opaque
/// `Theme.surface0`, so an omission builds, reads correctly at rest, and paints
/// the theme's grey over the entity's blur the moment the pointer lands.
/// `AccentDiscButton` carries the same override for the same reason.
#[test]
fn the_back_button_takes_every_brush_from_the_backdrop() {
    let button = code()
        .split_once("icon: \"arrow_back\";")
        .and_then(|(_, rest)| rest.split_once("clicked =>"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!button.is_empty(), "the band no longer mounts a back button ahead of its click handler");

    for (prop, tier) in [
        ("idle-bg", "HeroBackdrop.chip-fill"),
        ("hover-bg", "HeroBackdrop.disc-hover"),
        ("idle-fg", "HeroBackdrop.chrome"),
    ] {
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
        pane.contains("background: Theme.base.with-alpha(1.0 - root.hero-t);"),
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

/// The morph runs off a derivation, never a literal.
///
/// It shipped as `detail-open: false` for one phase on purpose, so the hero half
/// could compile and be pinned while the four detail views still drew their own
/// `DetailHeader` — and the page would have worn two banners the moment it went
/// true. Pinned now for the opposite reason: a literal `false` here silently
/// retires the whole hero half, and everything else in this file goes on passing
/// because the band's own source is still correct.
///
/// Which of the four ids is open decides the tab, not the other way round, so the
/// derivation has to name all four. Naming three leaves one detail opening under
/// an idle band, which reads as that one page failing rather than as a missing
/// clause.
#[test]
fn the_morph_is_driven_by_the_sheets_own_derivation() {
    let sheet: String = SHEET.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(
        sheet.contains("detail-open: root.detail-open;"),
        "my-library-view.slint must hand the band its own derived `detail-open` — a literal is \
         what held the hero half unreachable while the detail views still drew their own header"
    );
    let derived = sheet
        .split_once("property <bool> detail-open:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value);
    for open in ["album-open", "artist-open", "genre-open", "playlist-open"] {
        assert!(
            derived.contains(open),
            "the sheet's `detail-open` must name `{open}` — a detail left out of it opens under \
             an idle band, with its own body mounted below"
        );
    }
}

/// The hero tile shows a cover only when the open detail actually has one.
///
/// Slint has no empty-`image` literal, so the sheet's `cover` ternary has to bind *some*
/// global's cover on the Genre arm, whose tile is a name-hashed gradient with no image
/// behind it. `ArtworkImage` gates on `cover.width` alone, and more than one detail is
/// open at a time as a matter of routine — `seed_detail_from_settings` restores one per
/// view whichever tab boot resumes — so ungated, a genre hero paints whichever sibling
/// detail happened to be restored. It looks like a decode landing in the wrong view.
#[test]
fn the_hero_tile_suppresses_a_cover_the_open_detail_does_not_own() {
    let tile = code()
        .split_once("ArtworkImage {")
        .and_then(|(_, rest)| rest.split_once("tile-size:"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(
        !tile.is_empty(),
        "the band must mount an `ArtworkImage` ahead of its `tile-size` binding"
    );
    assert!(
        tile.contains("has-cover: root.artwork-path != \"\";"),
        "the band's tile must gate its cover on `artwork-path` — the sheet cannot withhold \
         `cover` on the arm that owns none, so this is where it says so"
    );
}

/// The band draws the back arrow; the page owns what it means.
///
/// `MyLibrary.back()` routes to the mounted tab's own `close-detail`, so every
/// teardown that button already triggers stays where it is. Unhandled, the arrow
/// is drawn, hovers, and does nothing — which is exactly what it did for the two
/// phases the band was mounted with the hero half switched off.
#[test]
fn the_back_arrow_routes_to_the_pages_own_close() {
    assert!(
        BAND.contains("clicked => { root.back-clicked(); }"),
        "the band's back button must emit `back-clicked` — it states no route of its own"
    );
    let sheet: String = SHEET.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        sheet.contains("back-clicked => { MyLibrary.back(); }"),
        "my-library-view.slint must route `back-clicked` to `MyLibrary.back()`, which is what \
         reaches the mounted tab's `close-detail` and the teardown behind it"
    );
}

/// Every hero fact the band declares is one the sheet feeds.
///
/// The band is data-agnostic — four detail globals with nothing between them — so
/// each fact arrives as an `in property`, and a new one added here and not bound
/// there simply sits at its default: an artwork tile that never fills, a badge
/// that never shows. Nothing fails, and only the hero it belongs to looks wrong.
#[test]
fn every_hero_fact_the_band_declares_is_fed_by_the_sheet() {
    const FACTS: [&str; 13] = [
        "title",
        "subtitle",
        "title-badge",
        "artwork-path",
        "cover",
        "circular-artwork",
        "fallback-icon",
        "tile-bg",
        "tile-icon-color",
        "blur-a",
        "blur-b",
        "use-a",
        "has-blur",
    ];

    let sheet: String = SHEET.split_whitespace().collect::<Vec<_>>().join(" ");
    let mount = sheet
        .split_once("band := LibraryTabBand {")
        .and_then(|(_, rest)| rest.split_once("MyLibraryTabPills"))
        .map_or("", |(body, _)| body);
    assert!(
        !mount.is_empty(),
        "my-library-view.slint no longer mounts the band ahead of its pill row"
    );

    for fact in FACTS {
        assert!(
            BAND.contains(&format!("{fact}:")) || BAND.contains(&format!("{fact};")),
            "library-tab-band.slint no longer declares `{fact}`; drop it from this list too"
        );
        assert!(
            mount.contains(&format!("{fact}:")),
            "my-library-view.slint must bind the band's `{fact}` — left unbound it sits at the \
             component's default, which is wrong for every detail and fails nothing"
        );
    }
}
