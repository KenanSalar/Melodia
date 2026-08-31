//! Source pins for `EntityCard`'s overlay slot — the half of it no reviewer can see.
//!
//! A control a host puts in that slot is drawn inside the card's artwork square but *written*
//! inside the host's own `EntityCard { … }` block, and Slint resolves names where they are written.
//! So `parent` there is the card, not the square: the four station controls compiled to
//! `card-height - …` and landed 74 px below the tile, over the text, with a source that reads
//! exactly like the one that was right.

use crate::test_support::{MIN_SLINT_SOURCES, UI_DIR, strip_line_comments, stripped_sources};

const ENTITY_CARD: &str = include_str!("../../../melodia-ui/ui/components/grid/entity-card.slint");

/// What a host writes to open the slot, and how the walk below finds the hosts rather than naming
/// them — a third one is covered the day it is written.
const OPENS_THE_SLOT: &str = "show-overlay-actions: true;";

/// Every file that puts controls in the slot, with the card's own declaration left out.
fn overlay_hosts() -> Vec<(String, String)> {
    let hosts: Vec<(String, String)> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(_, src)| src.contains(OPENS_THE_SLOT))
        .collect();

    assert!(
        !hosts.is_empty(),
        "nothing mounts `{OPENS_THE_SLOT}` any more, so the pins below measure an empty set"
    );
    hosts
}

/// The mistake is invisible in the source and only the generated tree tells the two apart, so the
/// rule is the blunt one: a host that opens the slot names no `parent` at all. Both of today's
/// frame the square through `EntityCard`'s published `tile-size`, which is what the card exports
/// it for.
#[test]
fn no_overlay_host_positions_against_parent() {
    let against_parent: Vec<String> = overlay_hosts()
        .into_iter()
        .filter(|(_, src)| src.contains("parent."))
        .map(|(path, _)| path)
        .collect();

    assert!(
        against_parent.is_empty(),
        "{against_parent:?} put controls in `EntityCard`'s overlay slot and position against \
         `parent` — which resolves to the card, not to the artwork square the slot is drawn in. \
         Frame them off the card's `tile-size` instead"
    );
}

/// A host reading the slot's hover back is the other half of the deal: the card's `touch` goes
/// false under a higher-z button, and only the host that named those buttons can say so.
#[test]
fn every_overlay_host_answers_for_its_own_hover() {
    let silent: Vec<String> = overlay_hosts()
        .into_iter()
        .filter(|(_, src)| !src.contains("overlay-hovered:"))
        .map(|(path, _)| path)
        .collect();

    assert!(
        silent.is_empty(),
        "{silent:?} put controls in the overlay slot without passing `overlay-hovered` — the \
         card's fill and cover zoom drop out while the pointer is on one of them"
    );
}

/// The card carried the Playlists CRUD trio once, and `show-overlay-actions` gated it and the slot
/// together — so a station card raising the flag for its own five controls drew three more it had
/// no callbacks for, on the same corners. The trio is the host's now, and the card draws no
/// control of its own.
#[test]
fn the_card_draws_none_of_its_hosts_controls() {
    assert!(
        !strip_line_comments(ENTITY_CARD).contains("IconButton"),
        "`entity-card.slint` draws a button again — the overlay is a slot, and a control it \
         mounts itself is one every host raising `show-overlay-actions` gets whether or not it \
         has a callback for it"
    );
}
