//! Source pins for `EntityCard`'s overlay slot, the half of it no reviewer can see.
//!
//! A control a host puts in that slot is drawn inside the card's artwork square but *written*
//! inside the host's own `EntityCard { … }` block, and Slint resolves names where they are written.
//! So `parent` there is the card, not the square: the four station controls compiled to
//! `card-height - …` and landed `card-height - tile-size` below where they read, over the text
//! block, with a source that reads exactly like the one that was right.

use melodia_testkit::strip_line_comments;

const ENTITY_CARD: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/entity-card.slint");

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
