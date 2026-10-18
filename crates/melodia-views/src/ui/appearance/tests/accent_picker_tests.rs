use super::*;

/// One of Material 3's own accents, and not its default, so a fallback to the default shows.
const HAND_PICKED: &str = "red";

/// Material You follows the artwork rather than naming a colour, so no swatch may read as picked
/// beside its switch.
#[test]
fn material_you_checks_no_swatch() {
    assert_eq!(slot_index(accent_slot(&MATERIAL3, MATERIAL_YOU_ACCENT_ID)), -1);
}

/// Turning System Border Color off while the accent follows the artwork reads this grid's index
/// back through here and falls to the theme's default. A -1 read as slot 0 would hand the border
/// the first swatch instead.
#[test]
fn the_index_that_checks_no_swatch_names_no_accent() {
    let unchecked = slot_index(None);

    assert_eq!(accent_at(&MATERIAL3, usize_from(unchecked)), None);
}

/// Every launch before the first cover lands, and every theme but Material 3.
#[test]
fn material_you_paints_the_last_hand_pick_until_it_can_follow() {
    let my_active = false;

    let painted =
        effective_accent_id(&MATERIAL3, MATERIAL_YOU_ACCENT_ID, my_active, Some(HAND_PICKED));

    assert_eq!(painted, HAND_PICKED);
}

#[test]
fn material_you_with_no_hand_pick_yet_paints_the_themes_default() {
    let my_active = false;

    let painted = effective_accent_id(&MATERIAL3, MATERIAL_YOU_ACCENT_ID, my_active, None);

    assert_eq!(painted, MATERIAL3.default_accent);
}

#[test]
fn material_you_paints_itself_once_it_can_follow() {
    let my_active = true;

    let painted =
        effective_accent_id(&MATERIAL3, MATERIAL_YOU_ACCENT_ID, my_active, Some(HAND_PICKED));

    assert_eq!(painted, MATERIAL_YOU_ACCENT_ID);
}

/// A live dynamic palette repaints the surfaces and leaves a hand-picked accent alone.
#[test]
fn a_hand_pick_paints_itself_while_material_you_is_live() {
    let my_active = true;

    let painted = effective_accent_id(&MATERIAL3, HAND_PICKED, my_active, Some("pink"));

    assert_eq!(painted, HAND_PICKED);
}
