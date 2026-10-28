//! The colour Windows borders its windows in, for the outline a frameless window draws there.
//!
//! The accent only while "Show accent color on title bars and window borders" is on, both halves
//! in the DWM registry key: `ColorPrevalence` the switch, `AccentColor` the colour. **Fails open**
//! to `None`, the neutral border Windows draws without the accent.

use super::registry::read_user_dword;

const DWM_KEY: &str = "Software\\Microsoft\\Windows\\DWM";

/// As `0x00RRGGBB`, or `None` with the accent off borders.
pub fn accent_border_rgb() -> Option<u32> {
    accent_border(
        read_user_dword(DWM_KEY, "ColorPrevalence"),
        read_user_dword(DWM_KEY, "AccentColor"),
    )
}

/// The decision apart from the registry: the accent applies only while the switch is exactly on.
fn accent_border(prevalence: Option<u32>, accent_abgr: Option<u32>) -> Option<u32> {
    if prevalence != Some(1) {
        return None;
    }
    accent_abgr.map(rgb_from_abgr)
}

/// `AccentColor` is stored `0xAABBGGRR`; the alpha is always opaque there, and dropped.
fn rgb_from_abgr(abgr: u32) -> u32 {
    ((abgr & 0x00_00_00_FF) << 16) | (abgr & 0x00_00_FF_00) | ((abgr & 0x00_FF_00_00) >> 16)
}

#[cfg(test)]
#[path = "tests/window_border_tests.rs"]
mod tests;
