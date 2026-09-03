//! Source pins for `EntityCard`'s overlay slot, the half of it no reviewer can see.
//!
//! A control a host puts in that slot is drawn inside the card's artwork square but *written*
//! inside the host's own `EntityCard { … }` block, and Slint resolves names where they are written.
//! So `parent` there is the card, not the square: the four station controls compiled to
//! `card-height - …` and landed `card-height - tile-size` below where they read, over the text
//! block, with a source that reads exactly like the one that was right.

use crate::test_support::{
    MIN_SLINT_SOURCES, UI_DIR, blocks_named, strip_line_comments, stripped_sources,
};

const ENTITY_CARD: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/entity-card.slint");

/// What a host writes to open the slot, and how the walk below finds the hosts rather than naming
/// them — a third one is covered the day it is written.
const OPENS_THE_SLOT: &str = "show-overlay-actions: true;";

/// The component the pins below read. [`blocks_named`] takes the name followed by `{`, so the
/// card's own `EntityCard inherits Rectangle {` declaration is not a mount.
const CARD: &str = "EntityCard";

/// Every `EntityCard { … }` block in the tree that opens the overlay slot, paired with its file.
///
/// **The mount's own braces rather than the whole file**, since a host is free to spell `parent`
/// anywhere else in it: `x: parent.width - self.width` is the canonical `OverlayScrollbar` mount,
/// so a file-wide ban would fail the first overlay host that also carries a scrollbar and the
/// repair would be to weaken the pin.
fn overlay_mounts() -> Vec<(String, String)> {
    let mut mounts: Vec<(String, String)> = Vec::new();

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !src.contains(OPENS_THE_SLOT) {
            continue;
        }
        let found_before = mounts.len();
        for body in blocks_named(&src, CARD) {
            if body.contains(OPENS_THE_SLOT) {
                mounts.push((path.clone(), body.to_owned()));
            }
        }
        assert!(
            mounts.len() > found_before,
            "{path} raises `{OPENS_THE_SLOT}` but no `{CARD}` block in it holds that \
             line — the walk reads the mount's own braces, so a mount spelled any other way is \
             one the pins below never see"
        );
    }

    assert!(
        !mounts.is_empty(),
        "nothing mounts `{OPENS_THE_SLOT}` any more, so the pins below measure an empty set"
    );
    mounts
}

/// The offending files, named once each however many mounts they carry.
fn files_of(mounts: Vec<(String, String)>) -> Vec<String> {
    let mut paths: Vec<String> = mounts.into_iter().map(|(path, _)| path).collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The mistake is invisible in the source and only the generated tree tells the two apart, so the
/// rule is the blunt one: nothing inside the mount names `parent` at all. Both of today's frame
/// the square through `EntityCard`'s published `tile-size`, which is what the card exports it for.
#[test]
fn no_overlay_host_positions_against_parent() {
    let against_parent = files_of(
        overlay_mounts().into_iter().filter(|(_, body)| body.contains("parent.")).collect(),
    );

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
    let silent = files_of(
        overlay_mounts()
            .into_iter()
            .filter(|(_, body)| !body.contains("overlay-hovered:"))
            .collect(),
    );

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
