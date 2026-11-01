//! A pick runs on `clicked`, never on a pointer event.
//!
//! An area that unmounts mid-press hands its release to whatever is under the pointer as a fresh
//! event, so a `pointer-event` acting on `Up` answers a release for a press it never had: a hover
//! control rewriting its row, or a notification card dropped mid-drag, picks the row beneath it.
//! `clicked` fires only for the item's own press, and carries no modifiers, so a box takes them
//! off that press.

use std::collections::BTreeSet;

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, block_after, stripped_sources};

/// The boxes that pick, each from its `clicked` with the modifiers its press saw. An equality, so
/// a renamed pick can't leave the walk below reading nothing.
const PICKERS: [&str; 3] = [
    "components/grid/entity-card.slint",
    "components/track-list/track-list-row.slint",
    "views/queue-panel/queue-row.slint",
];

/// Loose on purpose: below it the walk has stopped finding the handlers it reads.
const MIN_POINTER_EVENTS: usize = 10;

/// The body of every handler `head` opens in `src`, a longer name ending in it
/// (`double-clicked`) left out.
fn handlers<'a>(src: &'a str, head: &str) -> Vec<&'a str> {
    src.match_indices(head)
        .filter(|(at, _)| !src[..*at].ends_with(|c: char| c == '-' || c.is_ascii_alphanumeric()))
        .map(|(at, _)| block_after(&src[at..], head))
        .collect()
}

#[test]
fn no_pointer_event_handler_picks() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);

    let pointer_events: Vec<(&str, &str)> = sources
        .iter()
        .flat_map(|(path, src)| {
            handlers(src, "pointer-event(").into_iter().map(move |body| (path.as_str(), body))
        })
        .collect();
    let picking_on_clicked: BTreeSet<&str> = sources
        .iter()
        .filter(|(_, src)| {
            handlers(src, "clicked =>").iter().any(|body| body.contains("pick(root.press-"))
        })
        .map(|(path, _)| path.as_str())
        .collect();
    let picking_on_a_pointer_event: BTreeSet<&str> = pointer_events
        .iter()
        .filter(|(_, body)| body.contains("pick("))
        .map(|(path, _)| *path)
        .collect();

    assert!(
        pointer_events.len() >= MIN_POINTER_EVENTS,
        "only {} `pointer-event` handlers found: the walk went blind",
        pointer_events.len()
    );
    assert_eq!(
        picking_on_clicked,
        BTreeSet::from(PICKERS),
        "the boxes picking from `clicked` with their press's modifiers changed: a renamed pick \
         leaves the check below reading nothing, and a new box belongs in the list"
    );
    assert!(
        picking_on_a_pointer_event.is_empty(),
        "{picking_on_a_pointer_event:?} pick from a `pointer-event`, which a release handed on \
         from an area that unmounted mid-press reaches. Pick on `clicked`, taking the modifiers \
         off the press"
    );
}
