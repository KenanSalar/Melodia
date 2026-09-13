use super::{accent_border, rgb_from_abgr};

// Windows 11's default blue accent, as the registry stores it.
const ACCENT_ABGR: u32 = 0xFF_D7_78_00;
const ACCENT_RGB: u32 = 0x00_00_78_D7;

#[test]
fn the_accent_borders_windows_while_the_switch_is_on() {
    assert_eq!(accent_border(Some(1), Some(ACCENT_ABGR)), Some(ACCENT_RGB));
}

// Off is the default, and the border Windows draws then is its neutral one.
#[test]
fn the_accent_leaves_borders_alone_while_the_switch_is_off() {
    assert_eq!(accent_border(Some(0), Some(ACCENT_ABGR)), None);
}

// A value that won't read is a switch nobody turned on.
#[test]
fn a_missing_switch_leaves_borders_neutral() {
    assert_eq!(accent_border(None, Some(ACCENT_ABGR)), None);
}

// Anything but exactly on is off: the key is a DWORD and only 1 means the accent is applied.
#[test]
fn a_switch_value_other_than_one_leaves_borders_neutral() {
    assert_eq!(accent_border(Some(2), Some(ACCENT_ABGR)), None);
}

// The switch without a colour to apply has nothing to paint.
#[test]
fn the_switch_on_without_an_accent_leaves_borders_neutral() {
    assert_eq!(accent_border(Some(1), None), None);
}

// Each channel on its own, so a swap of red and blue can't pass on a grey.
#[test]
fn an_abgr_value_reads_back_with_red_and_blue_swapped_and_alpha_dropped() {
    assert_eq!(rgb_from_abgr(0xFF_00_00_11), 0x00_11_00_00);
    assert_eq!(rgb_from_abgr(0xFF_00_22_00), 0x00_00_22_00);
    assert_eq!(rgb_from_abgr(0xFF_33_00_00), 0x00_00_00_33);
}
