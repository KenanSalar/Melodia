//! No colour animation restarts on a palette fade.
//!
//! Slint can't animate a global, so `ui::appearance::theme_paint` eases one Material You palette
//! into the next by writing a blend into `Theme` every frame. An `animate` on a brush that can read
//! one restarts on each write and settles after the window has, so every colour animation carries
//! an `enabled:` naming `Theme.recoloring`. Missing, it shows only during a fade, and nothing fails
//! to build. `slint-pitfalls.md` has the mechanism and the kinds that need no gate.
//!
//! A colour is told by type rather than by name: a built-in brush property, or one some file
//! declares as `brush` or `color`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, binding_value, block_body, stripped_sources};

const GATE: &str = "Theme.recoloring";

/// The element properties that are brushes, which no `.slint` file declares.
const BUILTIN_BRUSHES: [&str; 6] =
    ["background", "color", "border-color", "drop-shadow-color", "fill", "stroke"];

/// Files whose animated brushes come off a backdrop tier Rust writes once per solve, which reads no
/// palette, and how many ungated colour animations each holds. Exact, so a second one in any of them
/// is a decision to make rather than one this list already made.
const UNGATED: [(&str, usize); 3] = [
    ("components/aurora-backdrop.slint", 2),
    ("components/hero-blur-backdrop.slint", 2),
    ("components/hero/mosaic-tab-hero.slint", 1),
];

/// Loose on purpose: below it the walk has stopped finding the animations it exists to hold.
const MIN_GATED: usize = 40;

/// One `animate` statement: where it starts, the properties it names and the body it eases them
/// with.
struct Animation<'a> {
    at: usize,
    properties: &'a str,
    body: &'a str,
}

fn animations(src: &str) -> Vec<Animation<'_>> {
    src.match_indices("animate ")
        .filter(|(at, _)| src[..*at].ends_with(|c: char| c.is_whitespace() || c == '{'))
        .filter_map(|(at, keyword)| {
            let open = at + src[at..].find('{')?;
            let properties = &src[at + keyword.len()..open];
            Some(Animation { at, properties, body: block_body(src, open)? })
        })
        .collect()
}

/// Every name some file declares as a `brush` or a `color`.
fn declared_brushes(sources: &[(String, String)]) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    for (_, src) in sources {
        for marker in ["<brush> ", "<color> "] {
            for (at, _) in src.match_indices(marker) {
                names.insert(leading_name(&src[at + marker.len()..]));
            }
        }
    }
    names
}

fn leading_name(src: &str) -> &str {
    let end = src.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(src.len());
    &src[..end]
}

fn eases_a_brush(animation: &Animation<'_>, declared: &BTreeSet<&str>) -> bool {
    animation
        .properties
        .split(',')
        .map(|name| name.trim().rsplit_once('.').map_or(name.trim(), |(_, property)| property))
        .any(|name| BUILTIN_BRUSHES.contains(&name) || declared.contains(name))
}

/// The span of every `states [ … ]` block. A transition keeps its timeline through the dirt a fade
/// writes, so the animations inside need no gate.
fn state_blocks(src: &str) -> Vec<Range<usize>> {
    src.match_indices("states [")
        .filter_map(|(at, marker)| {
            let open = at + marker.len() - 1;
            closing_bracket(src, open).map(|close| open..close)
        })
        .collect()
}

fn closing_bracket(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in src[open..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn no_colour_animation_restarts_on_a_palette_fade() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let declared = declared_brushes(&sources);

    let mut gated = 0;
    let mut ungated = BTreeMap::new();
    for (path, src) in &sources {
        let transitions = state_blocks(src);
        let (with_gate, without): (Vec<_>, Vec<_>) = animations(src)
            .into_iter()
            .filter(|animation| !transitions.iter().any(|span| span.contains(&animation.at)))
            .filter(|animation| eases_a_brush(animation, &declared))
            .partition(|animation| binding_value(animation.body, "enabled:").contains(GATE));
        gated += with_gate.len();
        if !without.is_empty() {
            ungated.insert(path.as_str(), without.len());
        }
    }

    assert!(gated >= MIN_GATED, "only {gated} gated colour animations found: the walk went blind");
    assert_eq!(
        ungated,
        BTreeMap::from(UNGATED),
        "a colour animation without `enabled: … !{GATE}` restarts on every frame of a palette fade \
         and settles after the window. Gate it, or, where Rust writes the brush once per solve, \
         add its file here"
    );
}
