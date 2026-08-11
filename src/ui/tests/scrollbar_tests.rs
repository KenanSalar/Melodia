//! Source pin for the scrollbar convention: nothing in the Slint tree paints
//! Slint's stock scrollbar.
//!
//! `CLAUDE.md` states the rule — both policies on every `ScrollView` / `ListView`
//! set to `always-off`, with a sibling `OverlayScrollbar` pinned by absolute
//! coordinates and round-tripped through `viewport-{x,y}` — because std-widgets'
//! bar paints inside padded containers and can't be reskinned. Nothing enforced
//! it, and three scrollers had drifted: the Artist Detail albums strip on
//! `as-needed`, the smart-playlist rule list with neither policy spelled, and the
//! playlist mosaic picker with only one. Each reads as a different app the moment
//! its content overflows, and each looks fine in the file it lives in.
//!
//! Two checks, because the two failures are different shapes. A wrong *value* is
//! the one you can grep for. An *omitted* policy is how two of the three got
//! here, and only the per-block walk sees it — a flat search would let a scroller
//! borrow the opt-out of the `TrackList` nested inside it, which is exactly the
//! arrangement every composite view has.

use crate::test_support::{MIN_SLINT_SOURCES, UI_DIR, normalize_ws as normalized, stripped_sources};

/// The elements that own a scrollbar-policy pair. `Flickable` is deliberately
/// absent: it has no scrollbars to turn off.
const SCROLLERS: [&str; 2] = ["ScrollView", "ListView"];
const AXES: [&str; 2] = ["horizontal", "vertical"];
const OPT_OUT: &str = "always-off";

/// Everything under this directory is mounted inside the dialog card, so its
/// scrollbars sit on `surface0` rather than on the page. `multiline-input.slint`
/// is named beside it because it is only ever mounted there — the Lyrics tab and
/// the playlist description — despite living a directory up.
const DIALOG_DIR: &str = "components/dialog/";
const DIALOG_STRAYS: [&str; 1] = ["multiline-input.slint"];
const CARD_TRACK: &str = "track-color: Theme.scrollbar-track-on-card;";

/// The two components that own a `TrackList`'s bar pair — the plain page and the
/// nested-under-another-scroller case. They are the only files allowed to bind a list's
/// vertical metrics onto an `OverlayScrollbar`.
const SCROLLBAR_COMPONENTS: [&str; 2] =
    ["components/track-list-scrollbars.slint", "components/composite-scrollbars.slint"];

/// The band a `TrackList`'s vertical bar has to clear, and the height of what's left —
/// the two metrics **only** a `TrackList` publishes, since only it has a column header
/// above its scrolling region.
///
/// Deliberately not `v-viewport-height` / `v-visible-height`, which every entity card
/// grid publishes too: those mount a *single* vertical bar over a body with no header and
/// no horizontal axis, which is a different shape and correctly not this component's.
/// Either name inside an `OverlayScrollbar` block is a hand-rolled pair; both appear in
/// the six converted hosts as hand-overs *outside* one, which is what the block walk
/// separates.
const TRACK_LIST_V_METRICS: [&str; 2] = ["body-y", "body-height"];

/// The whole tree, comment-stripped, paired with the file it came from.
fn sources() -> Vec<(String, String)> {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
}

/// The first index at or after `from` holding a non-whitespace byte.
fn next_non_ws(bytes: &[u8], from: usize) -> Option<usize> {
    (from..bytes.len()).find(|i| !bytes[*i].is_ascii_whitespace())
}

/// The body of the block whose `{` sits at `open`, braces excluded.
///
/// Quote-aware so a brace inside a string literal can't unbalance the count —
/// `"\{album.year}"` interpolation is balanced anyway, but nothing guarantees the
/// next one is. The caller strips comments first, which is the other half.
fn block_body(src: &str, open: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open + 1..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Every scroller declared in `src`, as `(element, body)`.
///
/// A declaration is the element name followed by `{`, which is what separates
/// `sv := ScrollView {` from the `import { ScrollView } from …` line above it.
fn scroller_blocks(src: &str) -> Vec<(&'static str, &str)> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    for name in SCROLLERS {
        let mut from = 0;
        while let Some(at) = src[from..].find(name).map(|rel| rel + from) {
            from = at + name.len();
            // A longer identifier ending in the same word isn't a declaration.
            if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                continue;
            }
            let Some(open) = next_non_ws(bytes, from) else { continue };
            if bytes[open] != b'{' {
                continue;
            }
            if let Some(body) = block_body(src, open) {
                out.push((name, body));
            }
        }
    }
    out
}

/// The `<axis>-scrollbar-policy` values `body` sets at its **own** nesting depth,
/// as `(axis, value)`.
///
/// Depth is the whole point: the composite views nest a `TrackList`'s scrollers
/// inside their own, so a scroller with no policy of its own would otherwise pass
/// on its child's.
fn own_policies(body: &str) -> Vec<(&str, &str)> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => depth = depth.saturating_sub(1),
            _ if depth == 0 && !in_string => {
                for axis in AXES {
                    let key = format!("{axis}-scrollbar-policy:");
                    if bytes[i..].starts_with(key.as_bytes())
                        && let Some(end) = body[i..].find(';').map(|rel| rel + i)
                    {
                        out.push((axis, body[i + key.len()..end].trim()));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The check that catches an omission — the shape two of the three drifted into.
#[test]
fn every_scroller_opts_out_of_the_stock_scrollbar() {
    let sources = sources();
    let mut blocks = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        for (element, body) in scroller_blocks(src) {
            blocks += 1;
            let set = own_policies(body);
            for axis in AXES {
                if !set.iter().any(|(a, v)| *a == axis && *v == OPT_OUT) {
                    offenders.push(format!("{path}: {element} does not set {axis}-scrollbar-policy: {OPT_OUT}"));
                }
            }
        }
    }

    assert!(blocks >= 20, "only {blocks} scrollers found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "every ScrollView/ListView must turn both stock scrollbars off and mount an \
         OverlayScrollbar instead (CLAUDE.md, Slint Conventions):\n{}",
        offenders.join("\n")
    );
}

/// A bar on a dialog card takes a track colour of its own.
///
/// The default is `surface0` at half alpha, which over a `surface0` card
/// composites to exactly `surface0` — zero contrast, so the thumb floats with
/// nothing saying how far the list runs. It is the failure mode that *looks* like
/// a missing feature rather than a wrong colour, and a new dialog scrollbar
/// inherits it by writing nothing at all, which is why it is worth a pin.
#[test]
fn every_dialog_scrollbar_takes_the_track_that_reads_on_a_card() {
    let sources = sources();
    let mut bars = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        let in_dialog = path.contains(DIALOG_DIR)
            || DIALOG_STRAYS.iter().any(|stray| path.ends_with(stray));
        if !in_dialog {
            continue;
        }
        let mut from = 0;
        while let Some(at) = src[from..].find("OverlayScrollbar").map(|rel| rel + from) {
            from = at + "OverlayScrollbar".len();
            let Some(open) = next_non_ws(src.as_bytes(), from) else { continue };
            if src.as_bytes()[open] != b'{' {
                continue;
            }
            let Some(body) = block_body(src, open) else { continue };
            bars += 1;
            if !normalized(body).contains(CARD_TRACK) {
                offenders.push(format!("{path}: OverlayScrollbar does not set {CARD_TRACK}"));
            }
        }
    }

    assert!(bars >= 5, "only {bars} dialog scrollbars found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "an OverlayScrollbar on a dialog card composites the default track into the \
         card exactly, leaving only the thumb visible:\n{}",
        offenders.join("\n")
    );
}

/// The check that catches a wrong value, wherever it is spelled — including on an
/// element this walk doesn't know about yet.
#[test]
fn no_scrollbar_policy_asks_for_the_stock_bar() {
    let sources = sources();
    let mut policies = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        let mut from = 0;
        while let Some(at) = src[from..].find("scrollbar-policy:").map(|rel| rel + from) {
            let value_at = at + "scrollbar-policy:".len();
            from = value_at;
            let Some(end) = src[value_at..].find(';').map(|rel| rel + value_at) else { continue };
            policies += 1;
            let value = src[value_at..end].trim();
            if value != OPT_OUT {
                offenders.push(format!("{path}: scrollbar-policy: {value}"));
            }
        }
    }

    assert!(policies >= 40, "only {policies} policies found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "the only scrollbar in the tree is OverlayScrollbar, so every policy reads \
         `{OPT_OUT}` (CLAUDE.md, Slint Conventions):\n{}",
        offenders.join("\n")
    );
}

/// **A `TrackList`'s bars come from a component; nobody hand-rolls the pair again.**
///
/// Six pages had — the three Songs tabs and the Album / Genre / Playlist detail bodies —
/// and they had already drifted into two spellings of the vertical anchor:
/// `tl.absolute-position.y - root.absolute-position.y + tl.body-y` in the details, a bare
/// `tl.y + tl.body-y` in the tabs. Both are right at their own nesting depth, and the
/// short one stops being right the moment a wrapper appears above it — silently, since
/// nothing about it reads wrong.
///
/// The `10px` thickness is the other half and the reason a snippet wouldn't do: it is a
/// *pair*, the horizontal bar's width subtracting exactly the vertical bar's so the two
/// don't overlap in the corner. Spelled per site, that coupling is invisible.
///
/// **Anchored on the header-band metrics, inside a bar's own block.** The six hosts still
/// spell `body-y:` and `body-height:` — as property hand-overs to `TrackListScrollbars`,
/// *outside* any `OverlayScrollbar { … }` — so the block walk is what tells a hand-over
/// from a mount. Two neighbouring shapes stay out for the same reason and are worth
/// naming, since widening the needle would sweep both in: every entity card grid mounts a
/// lone vertical bar over a body with no header, and `views/search/songs-section.slint`
/// mounts a lone *horizontal* one because the whole search view scrolls.
#[test]
fn no_page_hand_rolls_a_track_lists_scrollbars() {
    let sources = sources();
    let mut bars = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        if SCROLLBAR_COMPONENTS.iter().any(|owner| path.ends_with(owner)) {
            continue;
        }
        let mut from = 0;
        while let Some(at) = src[from..].find("OverlayScrollbar").map(|rel| rel + from) {
            from = at + "OverlayScrollbar".len();
            let Some(open) = next_non_ws(src.as_bytes(), from) else { continue };
            if src.as_bytes()[open] != b'{' {
                continue;
            }
            let Some(body) = block_body(src, open) else { continue };
            bars += 1;
            if let Some(metric) = TRACK_LIST_V_METRICS.iter().find(|m| body.contains(**m)) {
                offenders.push(format!("{path}: OverlayScrollbar binds {metric}"));
            }
        }
    }

    assert!(bars >= 10, "only {bars} non-component scrollbars found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "a plain `TrackList` page mounts `components/track-list-scrollbars.slint`'s \
         `TrackListScrollbars` as the last child of its root at `x: 0; y: 0; width: 100%; \
         height: 100%` — never a hand-rolled pair, which drifts on the anchor and hides \
         the two bars' shared thickness. A list nested *under* another scroller wants \
         `CompositeScrollbars` instead:\n{}",
        offenders.join("\n")
    );
}
