//! Source pin for the scrollbar convention: nothing in the Slint tree paints
//! Slint's stock scrollbar.
//!
//! Also for what a scroller does with a *drag*, which lands here rather than beside itself
//! because it is answered on the same two elements by the same block walk.
//!
//! `CLAUDE.md` states the rule — both policies `always-off` on every scroller, with a
//! sibling `OverlayScrollbar` pinned by absolute coordinates — because std-widgets' bar
//! paints inside padded containers and can't be reskinned. A drifted scroller reads as a
//! different app the moment its content overflows, and looks fine in the file it lives in.
//!
//! Two checks, because the two failures are different shapes. A wrong *value* is the one
//! you can grep for. An *omitted* policy is what only the per-block walk sees: a flat
//! search would let a scroller borrow the opt-out of the `TrackList` nested inside it,
//! which is exactly the arrangement every composite view has.

use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, block_body, normalize_ws as normalized, stripped_sources,
};

/// The elements that own a scrollbar-policy pair. `Flickable` is deliberately
/// absent: it has no scrollbars to turn off.
const SCROLLERS: [&str; 2] = ["ScrollView", "ListView"];
const AXES: [&str; 2] = ["horizontal", "vertical"];
const OPT_OUT: &str = "always-off";

/// Everything under this directory mounts inside the dialog card, so its scrollbars sit
/// on `surface0` rather than on the page. `multiline-input.slint` is named beside it
/// because it is only ever mounted there, despite living a directory up.
const DIALOG_DIR: &str = "components/dialog/";
const DIALOG_STRAYS: [&str; 1] = ["multiline-input.slint"];
const CARD_TRACK: &str = "track-color: Theme.scrollbar-track-on-card;";

/// The card grids, every one of them click-to-open. Scoped to the directory rather than
/// listed, so a seventh grid is held on the day it lands.
const GRID_DIR: &str = "components/grid/";

/// The click-to-act lists outside [`GRID_DIR`], which no directory scope reaches.
///
/// Both track lists are deliberately absent. `track-list.slint` keeps the default because a
/// row click is a selection rather than a navigation, and `draggable-track-list.slint` binds
/// the property to `!root.reorder-enabled`, a drag there being the reorder itself.
const CARD_SHAPED_LISTS: [&str; 4] = [
    "views/radio/station-grid.slint",
    "views/browse-view.slint",
    "views/radio/facet-chip.slint",
    "components/now-playing/up-next-list.slint",
];

const DRAG_PAN: &str = "mouse-drag-pan-enabled:";
const DRAG_PAN_OPT_OUT: &str = "false";

/// The two components that own a `TrackList`'s bar pair — the plain page and the
/// nested-under-another-scroller case. They are the only files allowed to bind a list's
/// vertical metrics onto an `OverlayScrollbar`.
const SCROLLBAR_COMPONENTS: [&str; 2] = [
    "components/track-list-scrollbars.slint",
    "components/composite-scrollbars.slint",
];

/// The band a `TrackList`'s vertical bar has to clear and the height of what's left —
/// the two metrics **only** a `TrackList` publishes, only it having a column header
/// above its scrolling region.
///
/// Deliberately not `v-viewport-height` / `v-visible-height`, which every entity card
/// grid publishes too: those mount a *single* bar over a body with no header and no
/// horizontal axis. Either name inside an `OverlayScrollbar` block is a hand-rolled
/// pair; both appear in the converted hosts as hand-overs *outside* one, which is what
/// the block walk separates.
const TRACK_LIST_V_METRICS: [&str; 2] = ["body-y", "body-height"];

/// The two bar pairs a `TrackList` page can mount, and the lane the list reserves under
/// itself for either one's horizontal half.
const BAR_PAIRS: [&str; 2] = ["TrackListScrollbars", "CompositeScrollbars"];
const LANE_OPT_IN: &str = "reserve-scrollbar-lane: true;";

/// The whole tree, comment-stripped, paired with the file it came from.
fn sources() -> Vec<(String, String)> {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
}

/// The first index at or after `from` holding a non-whitespace byte.
fn next_non_ws(bytes: &[u8], from: usize) -> Option<usize> {
    (from..bytes.len()).find(|i| !bytes[*i].is_ascii_whitespace())
}

/// Whether `src` mounts `component` — the name followed by `{`, which is what separates a
/// mount from the `import { … }` line and from the component's own `inherits` declaration.
fn mounts(src: &str, component: &str) -> bool {
    let mut from = 0;
    while let Some(at) = src[from..].find(component).map(|rel| rel + from) {
        from = at + component.len();
        if let Some(open) = next_non_ws(src.as_bytes(), from)
            && src.as_bytes()[open] == b'{'
        {
            return true;
        }
    }
    false
}

/// Every scroller declared in `src`, as `(element, body)`. A declaration is the element
/// name followed by `{`, which is what separates `sv := ScrollView {` from the
/// `import { ScrollView } from …` line above it.
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
            let Some(open) = next_non_ws(bytes, from) else {
                continue;
            };
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

/// The values `body` binds `key` to at its **own** nesting depth. Depth is the whole
/// point: the composite views nest a `TrackList`'s scrollers inside their own, so a
/// scroller that binds nothing would otherwise pass on its child's answer.
fn own_property<'a>(body: &'a str, key: &str) -> Vec<&'a str> {
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
                if bytes[i..].starts_with(key.as_bytes())
                    && let Some(end) = body[i..].find(';').map(|rel| rel + i)
                {
                    out.push(body[i + key.len()..end].trim());
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The `<axis>-scrollbar-policy` values `body` sets at its own depth.
fn own_policies(body: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for axis in AXES {
        let key = format!("{axis}-scrollbar-policy:");
        out.extend(own_property(body, &key).into_iter().map(|value| (axis, value)));
    }
    out
}

/// Whether `body` turns drag-to-pan off at its own depth.
fn opts_out_of_drag_pan(body: &str) -> bool {
    own_property(body, DRAG_PAN).contains(&DRAG_PAN_OPT_OUT)
}

/// The check that catches an omission, which is the shape most drift takes.
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
                    offenders.push(format!(
                        "{path}: {element} does not set {axis}-scrollbar-policy: {OPT_OUT}"
                    ));
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

/// A card click and a drag-pan are one gesture, and the pan wins by default: past 8 px of
/// travel inside 500 ms the flickable takes the grab and every item under it is sent a
/// cancel, so a press that drifts that far never lands as a click. That reads as "sometimes
/// it doesn't open", it only happens on a grid long enough to flick, and a new grid inherits
/// it by binding nothing at all. `views/radio/station-grid.slint` carries the argument.
#[test]
fn every_grid_scroller_opts_out_of_drag_to_pan() {
    let sources = sources();
    let mut blocks = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in sources.iter().filter(|(path, _)| path.contains(GRID_DIR)) {
        for (element, body) in scroller_blocks(src) {
            blocks += 1;
            if !opts_out_of_drag_pan(body) {
                offenders.push(format!("{path}: {element} does not set {DRAG_PAN} false"));
            }
        }
    }

    assert!(blocks >= 7, "only {blocks} grid scrollers found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "a card grid's delegates are click-to-open, so drag-to-pan costs a click it \
         intercepts and buys a gesture nothing here wants:\n{}",
        offenders.join("\n")
    );
}

/// [`every_grid_scroller_opts_out_of_drag_to_pan`] for the click-to-act lists a directory
/// scope can't reach. Per file rather than per block, which is the granularity
/// [`CARD_SHAPED_LISTS`] is written at: Browse mounts the page scroller its folder list sits
/// inside, and that one wraps a `TrackList` that keeps the default.
#[test]
fn every_card_shaped_list_opts_out_of_drag_to_pan() {
    let sources = sources();
    let mut offenders = Vec::new();

    for name in CARD_SHAPED_LISTS {
        let Some((path, src)) = sources.iter().find(|(path, _)| path.as_str() == name) else {
            offenders.push(format!("{name}: named here but not in the tree"));
            continue;
        };
        if !scroller_blocks(src).into_iter().any(|(_, body)| opts_out_of_drag_pan(body)) {
            offenders.push(format!("{path}: no scroller sets {DRAG_PAN} false"));
        }
    }

    assert!(
        offenders.is_empty(),
        "these lists act on a click the way a card does, so they answer the same way:\n{}",
        offenders.join("\n")
    );
}

/// A bar on a dialog card takes a track colour of its own. The default is `surface0` at
/// half alpha, which over a `surface0` card composites to exactly `surface0` — zero
/// contrast, so the thumb floats with nothing saying how far the list runs. It looks
/// like a missing feature rather than a wrong colour, and a new dialog scrollbar
/// inherits it by writing nothing at all.
#[test]
fn every_dialog_scrollbar_takes_the_track_that_reads_on_a_card() {
    let sources = sources();
    let mut bars = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        let in_dialog =
            path.contains(DIALOG_DIR) || DIALOG_STRAYS.iter().any(|stray| path.ends_with(stray));
        if !in_dialog {
            continue;
        }
        let mut from = 0;
        while let Some(at) = src[from..].find("OverlayScrollbar").map(|rel| rel + from) {
            from = at + "OverlayScrollbar".len();
            let Some(open) = next_non_ws(src.as_bytes(), from) else {
                continue;
            };
            if src.as_bytes()[open] != b'{' {
                continue;
            }
            let Some(body) = block_body(src, open) else {
                continue;
            };
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
            let Some(end) = src[value_at..].find(';').map(|rel| rel + value_at) else {
                continue;
            };
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
            let Some(open) = next_non_ws(src.as_bytes(), from) else {
                continue;
            };
            if src.as_bytes()[open] != b'{' {
                continue;
            }
            let Some(body) = block_body(src, open) else {
                continue;
            };
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

/// **A page that mounts a bar pair reserves the horizontal bar's lane.**
///
/// Both pairs put that bar on the list's own bottom edge, by different routes.
/// `TrackListScrollbars` pins it to the page's bottom, which on a plain page *is* where
/// the list ends. `CompositeScrollbars` takes the list's `x` and `width` and pins the bar
/// to the view's bottom — which reads as somewhere else entirely, and is the same place
/// the moment the list hits its `below-sv.visible-height` cap, i.e. exactly when there are
/// enough rows to care. Either way it paints across the last row, and across Playlist
/// Detail's after-the-last-row drop indicator, which pins to that edge.
///
/// `reserve-scrollbar-lane` clears it, and the two halves sit in different files with
/// nothing connecting them: an eighth host reads correct and regresses silently.
///
/// `views/search/songs-section.slint` is the one list under a horizontal bar that must not
/// opt in — its bar is a real layout sibling already holding a slot. It mounts neither
/// pair, so it falls outside the walk without being named.
#[test]
fn every_track_list_under_an_overlay_bar_reserves_its_lane() {
    let sources = sources();
    let mut hosts = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &sources {
        if SCROLLBAR_COMPONENTS.iter().any(|owner| path.ends_with(owner)) {
            continue;
        }
        let Some(pair) = BAR_PAIRS.iter().find(|pair| mounts(src, pair)) else {
            continue;
        };
        hosts += 1;
        if !normalized(src).contains(LANE_OPT_IN) {
            offenders.push(format!("{path}: mounts {pair}, no {LANE_OPT_IN}"));
        }
    }

    assert!(hosts >= 8, "only {hosts} bar-pair hosts found — the walk is broken");
    assert!(
        offenders.is_empty(),
        "a page mounting either bar pair sets `reserve-scrollbar-lane: true` on its list, \
         so the horizontal bar gets a lane of its own instead of painting over the last \
         row. A composite host owes it in its height cap's content-fit arm too, that arm \
         being a hand-sum of what the list holds:\n{}",
        offenders.join("\n")
    );
}
