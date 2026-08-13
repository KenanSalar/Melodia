//! Source pins for the faked-placeholder inputs and the tooltip pill — the four
//! shared components that paint text nobody sized, and the one that sizes itself
//! to text, plus the two hosts that reach for that one rather than drawing a pill
//! of their own.
//!
//! Slint has no `placeholder-text` on a raw `TextInput`, so all four inputs fake
//! one: a sibling `Text` gated on the field being empty. In a non-layout parent
//! that Text takes its own *implicit* width — the untruncated string — and paints
//! out of its field, which is what French did to the search bar. `overflow: elide`
//! is only half the cure: elide lowers a Text's layout *minimum* to one ellipsis
//! and leaves `preferred` at the full string, and the implicit size is the larger
//! of the two. Both lines are load-bearing, and deleting either leaves a file that
//! builds and looks right in English.
//!
//! `multiline-input.slint` is the fourth, and it is pinned to the *opposite*
//! shape: bounded to its scroller and wrapping, which is what a multi-line box
//! wants. It never had the bug, so it is the one nothing would have caught being
//! "fixed" into it.
//!
//! These live here rather than beside one component because the defect is one
//! copy-pasted block in three files with no Rust owner between them — the
//! reason `src/ui/tab_bar.rs` holds `tab-bar.slint`'s invariants.
//!
//! **The last two pins are corpus walks rather than checks on a named file**, which is
//! why this module is listed with the tree walks in `src/ui/mod.rs` as well as with the
//! component contracts. `no_page_or_shell_mounts_a_bare_tooltip` asks every page and
//! every shell container whether it hand-rolled a tooltip frame instead of mounting
//! `TooltipFrame`; `no_shared_band_draws_its_own_tooltip` asks every shared band whether
//! it drew a pill where it should have published an anchor. Both replaced a written
//! inventory that had already missed a site, and both carry a floor over their own
//! subtree — a walk whose filter stops matching reports a clean tree, not a broken walk.

// Every file here documents its own invariants at length, which is exactly the text a
// `contains` would otherwise match — so each pin reads the source stripped of comments and
// with its indentation collapsed to single spaces.
use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, normalize_ws as normalized, strip_line_comments as code,
    stripped_sources,
};

/// The two trees, relative to [`UI_DIR`], where a hand-rolled tooltip frame is the defect
/// rather than the default: the pages, and the containers that surround every page.
///
/// **`layout/` is here because `views/` alone would have missed the next Browse.** A
/// top-layer frame belongs wherever something paints over its host, and three of those
/// containers qualify — `sidebar.slint`, `now-playing-bar.slint`, `shortcut-scope.slint`
/// — with `now-playing-bar` in particular being a band full of controls. None mounts one
/// today, which is exactly when to widen: the site that gets this wrong is the one nobody
/// has written yet.
///
/// `components/` stays out — an in-tree mount is the *default* there, and `IconButton`,
/// `PillButton`, the traffic lights, the swatch dots and the two volume readouts all
/// annotate a host they sit inside. [`BAND_DIR`] is the one subtree where it isn't.
/// `app-window.slint` is neither tree, and holds the one documented exception.
const FRAME_DIRS: [&str; 2] = ["views/", "layout/"];

/// The shared banners, relative to [`UI_DIR`] — the one corner of `components/` where an
/// in-tree tooltip is a defect rather than the default.
///
/// A band is mounted *into* a page and its host's scroll body is declared after it, so
/// anything it draws is painted over. That is why all three publish `tip-x` … `tip-visible`
/// and let the page hang the frame: `LibraryTabBand`, `MosaicTabHero`, and the
/// `TabSearchHeader` row all three of them mount. `MosaicHeroTile` is here too and needs no
/// argument of its own — it is a tile inside a band, one level further from the frame.
///
/// **The boundary is the directory, not the concept**, and the difference is reachable:
/// `components/hero-blur-backdrop.slint` and `components/hero-chip-strip.slint` are hero
/// parts that sit at the `components/` root, so the "one level further from the frame"
/// argument covers them exactly as it covers `MosaicHeroTile` — and neither walk does.
/// Nothing is wrong today, because neither is a band and `MetaChip` is decorative with no
/// `TouchArea` at all. A chip that grows a hover affordance wants moving under `hero/`
/// rather than a third walk.
const BAND_DIR: &str = "components/hero/";

const SEARCH_BAR: &str = include_str!("../../../melodia-ui/ui/components/search-bar.slint");
const LABELED_INPUT: &str = include_str!("../../../melodia-ui/ui/components/labeled-input.slint");
const RULE_VALUE_INPUT: &str =
    include_str!("../../../melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint");
const MULTILINE_INPUT: &str =
    include_str!("../../../melodia-ui/ui/components/multiline-input.slint");
const TOOLTIP: &str = include_str!("../../../melodia-ui/ui/components/tooltip.slint");

/// The two hosts that read a value out of a live drag, and the only ones in the
/// tree that ever hand-rolled a pill of their own.
const VOLUME_READOUTS: [(&str, &str); 2] = [
    (
        "volume-popup.slint",
        include_str!("../../../melodia-ui/ui/components/now-playing/volume-popup.slint"),
    ),
    (
        "volume-popup-horizontal.slint",
        include_str!("../../../melodia-ui/ui/components/now-playing/volume-popup-horizontal.slint"),
    ),
];

/// Every host that budgets its header row and drives `input-width` outright: the
/// Settings page, the banner both mosaic pages wear, and My Library's morphing
/// band. The Search view is deliberately absent — its `input-width` is a literal,
/// so it reserves nothing and has no floor to read.
/// The one row that budgets against the bar's floor. It was three — Settings and the
/// two hero bands each spelled the same clamp — and they are one component now, which
/// is the point of pinning it here rather than at each mount.
const BUDGETING_HOST: (&str, &str) = (
    "tab-search-header.slint",
    include_str!("../../../melodia-ui/ui/components/hero/tab-search-header.slint"),
);

/// The declaration between two anchors. Bounding a region on the element that
/// follows it rather than on a closing brace keeps this out of the business of
/// counting braces inside comments.
fn code_between(src: &str, from: &str, to: &str) -> String {
    let code = code(src);
    let Some((_, after)) = code.split_once(from) else {
        return String::new();
    };
    normalized(after.split_once(to).map_or(after, |(block, _)| block))
}

/// The placeholder's own declaration, taken as everything between it and the
/// `TextInput` that follows it in all three files.
fn placeholder_block(src: &str) -> String {
    code_between(src, "text: root.placeholder;", "TextInput")
}

#[test]
fn every_faked_placeholder_is_bounded_and_elides() {
    for (name, src) in [
        ("search-bar.slint", SEARCH_BAR),
        ("labeled-input.slint", LABELED_INPUT),
        ("smart-playlist-editor-body.slint", RULE_VALUE_INPUT),
    ] {
        let block = placeholder_block(src);
        assert!(!block.is_empty(), "{name} no longer declares `text: root.placeholder;`");
        assert!(
            block.contains("width: 100%;"),
            "{name}'s placeholder has no width — it will take the untruncated \
             string's and paint out of its field"
        );
        assert!(
            block.contains("overflow: elide;"),
            "{name}'s placeholder is bounded but not elided — a long translation \
             gets cut mid-glyph with no ellipsis"
        );
    }
}

/// The fourth placeholder takes the opposite shape on purpose: a multi-line box
/// wants its hint bounded to the scroller and wrapped, not elided against one
/// line. It is the one exempted by `.claude/rules/slint-pitfalls.md`, and a rule
/// is not a build failure — without this the exemption is the easiest thing in
/// the tree to "fix" into the bug the other three just had.
#[test]
fn the_multiline_placeholder_is_bounded_and_wraps() {
    let block = code_between(MULTILINE_INPUT, "x: sv.x;", "Rectangle {");
    assert!(
        block.contains(r#"text: root.text == "" ? root.placeholder : "";"#),
        "multiline-input.slint no longer paints a placeholder at the scroll origin"
    );
    assert!(
        block.contains("width: sv.width;"),
        "multiline-input.slint's placeholder is no longer bounded to the scroller"
    );
    assert!(
        block.contains("wrap: word-wrap;"),
        "multiline-input.slint's placeholder no longer wraps — a multi-line box \
         that elides its hint has taken the single-line fix by mistake"
    );
}

/// The bar sizes its slot to the placeholder it was handed, and a bound `width`
/// on a component root is reported as both `min` and `max` — so leaving one here
/// would have put the window's own resize floor on the running locale. The split
/// is the same one `tab-bar.slint` carries for the same reason.
#[test]
fn the_search_bar_negotiates_its_width() {
    let src = normalized(SEARCH_BAR);
    for constraint in [
        "min-width: root.min-w;",
        "preferred-width: input-width;",
        "max-width: input-width;",
    ] {
        assert!(src.contains(constraint), "search-bar.slint's root is missing `{constraint}`");
    }
    // Leading space so this doesn't read the tail of `max-width: input-width;`.
    assert!(
        !src.contains(" width: input-width;"),
        "search-bar.slint's root binds `width` again — that is reported as both \
         min and max, and drags the window's resize floor with the locale"
    );
}

/// The floor is published, and the row that budgets around it takes it from the bar
/// rather than restating it — the `TabBar.compact-w` contract. Whatever the row stops
/// handing over has to be what the bar stops asking for; a restated literal looks
/// identical and silently decouples the two.
#[test]
fn the_search_bar_publishes_the_floor_its_hosts_budget_against() {
    assert!(
        normalized(SEARCH_BAR).contains("out property <length> min-w: 140px;"),
        "search-bar.slint no longer publishes its floor"
    );
    let (name, src) = BUDGETING_HOST;
    assert!(
        normalized(src).contains("property <length> search-w-min: search.min-w;"),
        "{name} restates the search bar's floor instead of reading `min-w` off it"
    );
}

/// The slot's natural width is measured off the Text the bar draws, so the
/// shape measured and the shape drawn can't drift, and the chrome it adds is
/// spelled from the tokens the layout below uses rather than restated.
#[test]
fn the_search_bar_measures_its_own_placeholder() {
    let src = normalized(SEARCH_BAR);
    assert!(
        src.contains("natural-width: placeholder-text.preferred-width + root.chrome-overhead;"),
        "search-bar.slint no longer measures its own placeholder"
    );
    assert!(
        src.contains("chrome-overhead: Theme.pad-sm + root.icon-size + 2 * Theme.pad-xs;"),
        "search-bar.slint's chrome overhead is no longer derived from the layout's own tokens"
    );
    assert!(
        src.contains("icon-size: root.icon-size;"),
        "search-bar.slint's search glyph no longer takes the size `chrome-overhead` budgets for it"
    );
    assert!(
        src.contains("input-width: clamp(root.natural-width,"),
        "search-bar.slint's default width no longer follows its placeholder"
    );
}

/// The pill is absolutely positioned against its host, so nothing downstream
/// stops it leaving the window — the cap is what does, and the wrap is what
/// makes the cap render a label instead of clipping one.
#[test]
fn the_tooltip_pill_is_capped() {
    let src = normalized(TOOLTIP);
    assert!(
        src.contains("width: min(tip-text.preferred-width + 16px, root.max-pill-width);"),
        "tooltip.slint's pill is unbounded again"
    );
    assert!(
        src.contains("width: root.width - 16px; wrap: word-wrap;"),
        "tooltip.slint's label is no longer bounded and wrapped, so the cap would clip it"
    );
}

/// All four sides, and the `x` arm that makes the fourth one mean anything.
///
/// A variant with no arm falls through to the centred branch, so the pill lands
/// *on* the host instead of beside it — for the volume readout, directly over the
/// slider it is reading out. The mount still compiles and still fades in, which is
/// why the arm is worth pinning separately from the variant.
#[test]
fn the_tooltip_clears_its_host_on_all_four_sides() {
    let src = normalized(TOOLTIP);
    assert!(
        src.contains("export enum TooltipSide { above, below, left, right }"),
        "tooltip.slint no longer declares all four sides"
    );
    assert!(
        src.contains("root.side == TooltipSide.left ? (-self.width - root.gap)"),
        "TooltipSide.left has no `x` arm of its own, so it falls through to the centred one"
    );
}

/// Both volume popups read their percentage out through the shared pill.
///
/// Each used to hand-roll one — a fixed 42×22 `Theme.surface2` rectangle, no
/// border, a bigger label and a bouncier fade — diverging from every other tooltip
/// in the app, and from the *same string* the trigger disc a few lines above
/// already renders through `Tooltip` on the keyboard-HUD path. Two readouts per
/// file is the whole count: the disc's and the slider's.
#[test]
fn the_volume_readouts_are_the_shared_tooltip() {
    for (name, src) in VOLUME_READOUTS {
        let src = normalized(&code(src));
        assert!(src.contains("Tooltip {"), "{name} no longer mounts the shared Tooltip");
        assert!(
            src.contains("force-shown:"),
            "{name}'s readout must bypass the reveal delay — it has to be up on the frame \
             the drag starts, not half a second later"
        );
        assert_eq!(
            src.matches("round(Player.vm.volume) + \"%\"").count(),
            2,
            "{name} should read the percentage out exactly twice — once on the disc's \
             tooltip, once on the slider's"
        );
    }
}

/// **A page or a shell container reaches the pill through `TooltipFrame`, never past it.**
///
/// A top-layer tooltip — one whose host sits somewhere Slint paints over afterwards — is
/// a frame tracking the host's rect with the pill inside it, and six of those had been
/// spelled out identically. `components/tooltip-frame.slint` is that shape now, and this
/// walks [`FRAME_DIRS`] rather than naming the five hosts for the reason
/// `ui::file_dialog::tests` walks `src/`: the site that gets it wrong is the one nobody
/// has written yet.
///
/// It is not a hypothetical. `browse-view.slint`'s view-toggle frame — the same six
/// lines, its own comment saying "same shape as `my-library-view.slint`'s `header-tip`" —
/// sat outside the rule's written inventory for as long as that inventory existed, and
/// was the one site the extraction missed. A pin over a list is precisely the pin a
/// missing entry walks past, which is also why the walk covers `layout/` and not only the
/// pages: see [`FRAME_DIRS`] for which trees are in and why `components/` is not.
///
/// **The name says "shell" rather than "view" because `layout/` is neither.** Those four
/// files are the chrome *around* every page — the rail, the transport bar, the shortcut
/// scope, the resize ring — and a walk whose name promised only views is one a later
/// reader would narrow back to `views/` on the grounds that it had overreached.
#[test]
fn no_page_or_shell_mounts_a_bare_tooltip() {
    let mut pages = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !FRAME_DIRS.iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        pages += 1;
        if src.contains("Tooltip {") {
            offenders.push(path);
        }
    }

    // A floor over the *subset*, which `MIN_SLINT_SOURCES` can't stand in for: it bounds
    // the whole tree, so renaming `views/` — or dropping an entry from `FRAME_DIRS` —
    // leaves this filter matching nothing while the walk still sees every file it asked
    // for. An empty offender list is then indistinguishable from a clean tree. Loose on
    // purpose: 54 files today, and a floor tight enough to matter would trip on an
    // ordinary page deletion.
    assert!(pages >= 30, "only {pages} files under {FRAME_DIRS:?} — the walk is broken");
    assert!(
        offenders.is_empty(),
        "nothing under `views/` or `layout/` may mount `Tooltip` directly — a top-layer \
         tooltip is `components/tooltip-frame.slint`'s `TooltipFrame`, which owns the \
         `host-width` wiring and leaves the host only its two `absolute-position` deltas. \
         If a mount genuinely needs what the component doesn't do — a live-width `x`, a \
         `held` latch, a `gap` — it belongs beside `app-window.slint`'s `sidebar-tip` with \
         the reason written down, not inline in a page or a shell container:\n{}",
        offenders.join("\n")
    );
}

/// **A shared band publishes its tooltip rect and draws no pill of its own.**
///
/// The mirror of the walk above, one level in. A band is mounted *into* a page, and the
/// page declares its scroll body afterwards — so a pill the band draws is painted over by
/// the very content the frame exists to clear, while the page's own frame draws the real
/// one at the top layer. A band that grows a pill beside its `tip-*` forwards compiles,
/// keeps every alias resolving, and paints two tooltips of which one is invisible.
///
/// **One walk rather than an assertion per band**, which is what this replaced: the rule
/// was stated three times — once in `library_tab_band_tests`, once in
/// `mosaic_tab_hero_tests`, once inside `tab_search_header_tests`' anchor pin — over three
/// of the four files in [`BAND_DIR`]. Three copies of one rule is the shape this tree
/// keeps folding, and a per-band test is a list: it cannot cover the band nobody has
/// written, which is the same argument [`FRAME_DIRS`] makes one directory out.
///
/// The three per-band tests each said *which* frame occludes them — the sheet's, or each
/// page's. That detail is the bands' own and stays in their files, on the forwarding pins
/// that assert the anchors reach a host at all; what is shared is only "draws none".
#[test]
fn no_shared_band_draws_its_own_tooltip() {
    let mut bands = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !path.starts_with(BAND_DIR) {
            continue;
        }
        bands += 1;
        if src.contains("Tooltip {") {
            offenders.push(path);
        }
    }

    // The subset floor its sibling above now carries too, and for the same reason: a
    // renamed `components/hero/` would sail past the tree-wide one while this filter
    // quietly matched nothing. Exact rather than loose — the directory is the four files
    // [`BAND_DIR`] names, and a fifth band is precisely what this walk exists to reach.
    assert!(bands >= 4, "only {bands} files under {BAND_DIR} — the walk is broken");
    assert!(
        offenders.is_empty(),
        "a shared band must publish `tip-x` … `tip-visible` and let its host hang the \
         frame, never mount `Tooltip` itself — the host declares its scroll body after the \
         band, so a pill here is painted over *and* duplicated by the frame the host \
         already draws:\n{}",
        offenders.join("\n")
    );
}
