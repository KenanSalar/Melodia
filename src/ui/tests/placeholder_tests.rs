//! Source pins for the faked-placeholder inputs and the tooltip pill — the four
//! shared components that paint text nobody sized, and the one that sizes itself
//! to text.
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

const SEARCH_BAR: &str = include_str!("../../../melodia-ui/ui/components/search-bar.slint");
const LABELED_INPUT: &str = include_str!("../../../melodia-ui/ui/components/labeled-input.slint");
const RULE_VALUE_INPUT: &str =
    include_str!("../../../melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint");
const MULTILINE_INPUT: &str =
    include_str!("../../../melodia-ui/ui/components/multiline-input.slint");
const TOOLTIP: &str = include_str!("../../../melodia-ui/ui/components/tooltip.slint");

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

/// Collapses runs of whitespace so a pin reads a token sequence rather than one
/// file's indentation.
fn normalized(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The declaration between two anchors. Bounding a region on the element that
/// follows it rather than on a closing brace keeps this out of the business of
/// counting braces inside comments; comment lines are dropped first, so prose
/// about the fix can neither satisfy a pin nor end the region early.
fn code_between(src: &str, from: &str, to: &str) -> String {
    let code = src
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let Some((_, after)) = code.split_once(from) else { return String::new() };
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
    for constraint in
        ["min-width: root.min-w;", "preferred-width: input-width;", "max-width: input-width;"]
    {
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
