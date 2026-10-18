use melodia_artwork::media::image::material_you::SchemeStyle;
use melodia_core::themes::MATERIAL_YOU_ACCENT_ID;

use super::{AccentChange, accent_change};

/// One of Material 3's own accents, so the pick is one the grid can show.
const HAND_PICKED: &str = "red";

/// Tonal Spot to Vibrant turned Material You back on over an accent picked by hand, and the pick
/// was lost.
#[test]
fn a_new_style_over_a_hand_picked_accent_keeps_it() {
    let change = accent_change(SchemeStyle::TonalSpot, SchemeStyle::Vibrant, HAND_PICKED);

    assert_eq!(change, AccentChange::Keep);
}

#[test]
fn leaving_none_follows_the_artwork() {
    let change = accent_change(SchemeStyle::None, SchemeStyle::TonalSpot, HAND_PICKED);

    assert_eq!(change, AccentChange::Follow);
}

#[test]
fn a_style_change_while_following_changes_nothing() {
    let from_none =
        accent_change(SchemeStyle::None, SchemeStyle::TonalSpot, MATERIAL_YOU_ACCENT_ID);
    let between_styles =
        accent_change(SchemeStyle::TonalSpot, SchemeStyle::Vibrant, MATERIAL_YOU_ACCENT_ID);

    assert_eq!(from_none, AccentChange::Keep, "out of None");
    assert_eq!(between_styles, AccentChange::Keep, "between two styles");
}

/// A re-pick of None is the corner: the outgoing style says "on the way out of None", and there is
/// still no artwork to follow.
#[test]
fn none_hands_back_an_accent_that_was_following() {
    let from_a_style =
        accent_change(SchemeStyle::TonalSpot, SchemeStyle::None, MATERIAL_YOU_ACCENT_ID);
    let re_picked = accent_change(SchemeStyle::None, SchemeStyle::None, MATERIAL_YOU_ACCENT_ID);

    assert_eq!(from_a_style, AccentChange::Restore, "from a style");
    assert_eq!(re_picked, AccentChange::Restore, "None picked again");
}

#[test]
fn none_keeps_an_accent_picked_by_hand() {
    let from_a_style = accent_change(SchemeStyle::TonalSpot, SchemeStyle::None, HAND_PICKED);
    let re_picked = accent_change(SchemeStyle::None, SchemeStyle::None, HAND_PICKED);

    assert_eq!(from_a_style, AccentChange::Keep, "from a style");
    assert_eq!(re_picked, AccentChange::Keep, "None picked again");
}
