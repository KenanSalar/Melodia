//! Source-level pins on `crates/melodia-ui/ui/components/hero/tab-search-header.slint`.
//!
//! The tab bar and the filter box sharing one row, plus the width budget dividing it.
//! Five separately paid-for fixes live in it, every one invisible in review, which is
//! why they are pinned at all — and one copy of each, where three hosts once spelled the
//! row out for themselves. Each band keeps a single pin that it still mounts this row
//! and forwards what its host reads back.
//!
//! Here rather than under a host for the `tab_bar_tests.rs` reason: no Rust module owns
//! the file.

use crate::test_support::{binding_value as binding, strip_line_comments};

const HEADER: &str =
    include_str!("../../../../melodia-ui/ui/components/hero/tab-search-header.slint");

/// The header with its comments dropped, so prose about a fix can neither satisfy a
/// pin nor bound a region early.
fn code() -> String {
    strip_line_comments(HEADER)
}

/// **The floor is published rather than restated.**
///
/// Each host draws this row from its own width mirror, a plausible guess for the frame
/// before the first layout reports the truth — and that seed has to be the row's own
/// floor. Seeded wide, the bar believes it can afford full-width tabs, draws them into a
/// panel that can't seat them, and they spill under the search bar, which a miniplayer →
/// full swap reliably produces. Publishing it is what stopped three hosts each summing
/// the same terms.
#[test]
fn the_row_publishes_its_own_floor() {
    let floor = binding(HEADER, "out property <length> row-floor:");
    assert!(
        floor.contains("bar.compact-w") && floor.contains("search-w-max"),
        "`row-floor` must be the sum of the bar's own `compact-w` and the input's resting \
         width — a host seeds its width mirror off it rather than respelling the sum"
    );
}

/// The two ends of the header budget read published floors rather than restating them, so
/// whatever a component stops asking for is what the row stops handing over. A literal on
/// either side looks identical and silently decouples the two.
#[test]
fn the_header_budget_reserves_against_published_floors() {
    assert!(
        HEADER.contains("property <length> search-w-min: search.min-w;"),
        "the row must take the input's floor off `SearchBar.min-w`"
    );
    let budget = binding(HEADER, "out property <length> search-w: clamp(");
    assert!(
        budget.contains("bar.compact-w"),
        "the search slot must be budgeted against the bar's own `compact-w` — the tabs are what \
         it has to leave room for, and a restated `5 * 48px` drifts the moment a tab is added"
    );
}

/// The leading slot's gap is folded into its width, which is what lets the row carry no
/// spacing of its own and the whole inset ease to zero. The bar-to-input clearance then
/// has to survive as the spacer's own floor, or the budget above reserves two `pad-md`s
/// the layout never hands out.
#[test]
fn the_row_hands_out_no_spacing_of_its_own() {
    let code = code();
    assert!(
        code.contains("spacing: 0px;"),
        "the row must hand out no spacing, or a host's leading slot can't ease its width to zero"
    );
    assert!(
        code.contains("min-width: 2 * Theme.pad-md;"),
        "the bar-to-input clearance must survive as the spacer's own floor — it is the two \
         `pad-md`s the `search-w` budget reserves"
    );
    assert!(
        binding(&code, "property <length> content-w:").contains("root.lead-w"),
        "the leading slot must come out of the bar's budget, or a host that grows one draws its \
         tabs into a row that no longer has room for them"
    );
}

/// The sub-view slide starts disarmed and the direction comes off the bar's own
/// `previous-index`.
///
/// A tab body sits inside its page's own enter transition, so a horizontal slide composed
/// with the page's fade-up reads as a diagonal on every arrival from the sidebar —
/// `tab-anim-armed` starts `false` and only a real pick arms it, and a page destroyed and
/// rebuilt on entry re-disarms for free.
#[test]
fn the_sub_view_slide_is_disarmed_until_the_first_switch() {
    assert!(
        HEADER.contains("out property <bool> tab-anim-armed: false;"),
        "`tab-anim-armed` must start false — a page's own entrance is the only thing that should \
         move when it arrives"
    );

    let handler = HEADER
        .split_once("selected(i) =>")
        .and_then(|(_, rest)| rest.split_once("root.tab-selected(i);"))
        .map_or("", |(body, _)| body);
    assert!(
        handler.contains("root.tab-anim-armed = true;"),
        "the bar's `selected` handler must arm the slide — nothing else can tell a real switch \
         from the page mounting"
    );
    // Down to the operand: the direction has to come off `previous-index`, `tab-idx` and
    // everything bound to it already reading the tab just picked. A local mirror here
    // would compare `i` against `i` and enter from the left every time.
    assert!(
        handler.contains("root.tab-enter-from = i > bar.previous-index"),
        "the `selected` handler must set the direction from `bar.previous-index`, and *before* it \
         hands the pick out — the same ordering `nav_transition.rs` follows for the page-level \
         transition"
    );
}

/// The compact-mode tooltip is anchored by the *host*, after its scroll body, because
/// Slint paints in declaration order and anything this row owns is covered by the content
/// below it. So the row publishes the rect instead — derived from the hovered index rather
/// than snapshotted, which is what keeps a tab that moves under a parked pointer (Ctrl+B,
/// F11) anchored to its tooltip.
///
/// The other half — that the row draws no pill of its own — is
/// `placeholder_tests::no_shared_band_draws_its_own_tooltip`, which walks
/// `components/hero/`. Publishing an anchor and drawing nothing are two rules: this one
/// is the row's alone, that one every band's.
#[test]
fn the_row_publishes_every_tooltip_anchor_its_host_needs() {
    for prop in [
        "tip-x",
        "tip-y",
        "tip-w",
        "tip-h",
        "tip-label",
        "tip-visible",
    ] {
        assert!(
            HEADER.contains(&format!("out property <length> {prop}:"))
                || HEADER.contains(&format!("out property <string> {prop}:"))
                || HEADER.contains(&format!("out property <bool> {prop}:")),
            "the row must publish `{prop}` for its host's tooltip frame"
        );
    }
    let tip_x = binding(HEADER, "out property <length> tip-x:");
    assert!(
        tip_x.contains("bar.tab-w * bar.hovered-idx"),
        "`tip-x` must be derived from the hovered *index* — equal-width cells are what make that \
         possible, and a snapshotted rect goes stale the moment anything resizes the bar"
    );
    // **Both anchors owe the bar's own offset inside the row**, and only `x` can lose it
    // silently: a host adds its own `header ↔ root` delta and cannot reach past that
    // into where the bar sits. With a leading slot that offset is the whole `lead-w`, so
    // the pill lands a back button's width left of the tab it names — and only while a
    // detail is open, the mounts without one putting the bar at `x == 0`.
    for (prop, axis) in [("tip-x", "x"), ("tip-y", "y")] {
        assert!(
            binding(HEADER, &format!("out property <length> {prop}:"))
                .contains(&format!("bar.absolute-position.{axis} - root.absolute-position.{axis}")),
            "`{prop}` must offset by the bar's own position in the row — a host can only add its \
             own delta, so anything between the row's edge and the bar is this file's to carry"
        );
    }
}

/// Every brush the row paints with is a defaulted input, which is what lets one component
/// sit on a flat Settings page and on an entity's solved blur.
///
/// Hardcode any of them and the mount that isn't Settings washes out — the failure the
/// `HeroBackdrop` rule exists for, and one that looks correct in whichever file you happen
/// to be reading.
#[test]
fn every_painted_brush_is_an_input() {
    for (prop, default) in [
        ("label-color", "Theme.text"),
        ("active-color", "Theme.accent"),
        ("hover-fill", "Theme.surface0"),
        ("divider-color", "Theme.surface1"),
    ] {
        assert!(
            HEADER.contains(&format!("in property <brush> {prop}: {default};")),
            "`{prop}` must be an input defaulted to `{default}` — the hero mounts override it, \
             and a literal here is invisible until one of them is on screen"
        );
        assert!(
            HEADER.contains(&format!("{prop}: root.{prop};")),
            "the bar must read `{prop}` off the input rather than the token"
        );
    }
}

/// The three hosts, and that each still mounts the row rather than growing its own.
///
/// A page that re-inlines a `TabBar` and a `SearchBar` passes every pin above — they only
/// ever look at this file — so the count of mounts is the half that has to be checked from
/// outside.
#[test]
fn every_tabbed_page_mounts_the_shared_row() {
    const HOSTS: [(&str, &str); 3] = [
        (
            "library-tab-band.slint",
            include_str!("../../../../melodia-ui/ui/components/hero/library-tab-band.slint"),
        ),
        (
            "mosaic-tab-hero.slint",
            include_str!("../../../../melodia-ui/ui/components/hero/mosaic-tab-hero.slint"),
        ),
        (
            "settings-view.slint",
            include_str!("../../../../melodia-ui/ui/views/settings-view.slint"),
        ),
    ];

    for (name, src) in HOSTS {
        assert!(
            src.contains("header := TabSearchHeader {"),
            "{name} must mount the shared header row"
        );
        for grown in ["TabBar {", "SearchBar {"] {
            assert!(
                !src.contains(grown),
                "{name} must not mount its own `{grown}` — that is the row it stopped spelling out"
            );
        }
        assert!(
            binding(src, "property <length> page-w:").contains("header.row-floor"),
            "{name}'s width seed must read the row's published floor rather than re-summing it"
        );
    }
}
