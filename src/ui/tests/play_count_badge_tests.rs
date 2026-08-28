//! Source pins for the play-count badge and its four mounts.
//!
//! The badge is one leaf drawn by two unrelated cards, and every way it goes wrong is a way that
//! reviews clean: a host re-rolling the pill looks like ordinary markup, a mount opting every card
//! into it looks like a simplification, and the corner it shares on a station card only collides on
//! a row almost nobody has.

use crate::test_support::strip_line_comments;

const BADGE: &str = include_str!("../../../melodia-ui/ui/components/grid/play-count-badge.slint");
const ENTITY_CARD: &str = include_str!("../../../melodia-ui/ui/components/grid/entity-card.slint");
const STATION_CARD: &str = include_str!("../../../melodia-ui/ui/views/radio/station-card.slint");
const KEPT_TAB: &str = include_str!("../../../melodia-ui/ui/views/radio/kept-stations-tab.slint");

/// Every file that draws a card carrying the badge, named for the failure message.
const CARDS: [(&str, &str); 2] = [
    ("entity-card.slint", ENTITY_CARD),
    ("station-card.slint", STATION_CARD),
];

/// The pill's own markup, as only the leaf may spell it. `border-color` rather than the fill,
/// which is a colour other overlays on the same tile legitimately share.
const PILL_MARKUP: &str = "border-color: Theme.surface2";

#[test]
fn both_cards_mount_the_shared_badge_rather_than_drawing_their_own() {
    for (name, source) in CARDS {
        let code = strip_line_comments(source);
        assert!(
            code.contains("PlayCountBadge {"),
            "`{name}` must mount `PlayCountBadge` — a card that draws the pill itself is the \
             second copy the leaf exists to prevent"
        );
        assert!(
            !code.contains(PILL_MARKUP),
            "`{name}` spells the pill's own chrome — the badge is `grid/play-count-badge.slint` \
             and a host may only mount it"
        );
    }
}

/// The `> 0` suppression lives in the leaf, so a host passes a count and asks no question about
/// it. Re-adding the ternary is harmless on the day it is written and is how the rule ends up
/// stated in four places, one of which will eventually disagree.
#[test]
fn the_leaf_owns_the_hide_at_zero_rule() {
    assert!(
        strip_line_comments(BADGE).contains("visible: root.count > 0"),
        "`play-count-badge.slint` must hide itself at a count of zero — without it every host \
         owes the test, and a host that forgets paints an empty pill on an unplayed row"
    );
}

/// Radio's Recently Played is the one tab that asks for the badge, and it asks off the same
/// predicate the star mark rides. Bound to `true`, every Favorites card carries a count under a
/// list that ranks by name.
#[test]
fn only_recently_played_turns_the_station_badge_on() {
    let code = strip_line_comments(KEPT_TAB);
    assert!(
        code.contains("show-play-count: root.is-recent;"),
        "`kept-stations-tab.slint` must gate `show-play-count` on `is-recent` — the file is \
         mounted for both local tabs and an ungated badge lands on Favorites too"
    );
}

/// The count now has the tile's top-left to itself: segmented stations play, so the warning badge
/// that used to share the corner is gone along with the gate behind it.
#[test]
fn nothing_else_claims_the_count_badge_corner() {
    let code = strip_line_comments(STATION_CARD);
    assert!(
        !code.contains("unplayable"),
        "`station-card.slint` still carries the unplayable badge, which no longer has a gate \
         behind it and would paint over the count"
    );
}
