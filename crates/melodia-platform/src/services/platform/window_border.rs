//! The colour Windows draws its own window borders in, for the outline a frameless window draws
//! where that border would be.
//!
//! Windows paints a border in the accent colour only while "Show accent color on title bars and
//! window borders" is on, and a neutral one otherwise. Both halves of that answer are in the DWM
//! registry key: `ColorPrevalence` is the switch, and `AccentColor` the colour it applies.
//!
//! **Fails open**: a key or value that won't read answers `None`, the neutral border, which is what
//! Windows draws whenever the accent isn't applied.

use super::registry::read_user_dword;

const DWM_KEY: &str = "Software\\Microsoft\\Windows\\DWM";

/// Returns the accent colour Windows borders its windows in, as `0x00RRGGBB`, or `None` when the
/// user has the accent off borders and they stay neutral.
pub fn accent_border_rgb() -> Option<u32> {
    accent_border(
        read_user_dword(DWM_KEY, "ColorPrevalence"),
        read_user_dword(DWM_KEY, "AccentColor"),
    )
}

/// The decision, apart from the registry: the accent applies only while the switch is exactly on.
fn accent_border(prevalence: Option<u32>, accent_abgr: Option<u32>) -> Option<u32> {
    if prevalence != Some(1) {
        return None;
    }
    accent_abgr.map(rgb_from_abgr)
}

/// `AccentColor` is stored `0xAABBGGRR`. The alpha is always opaque there and the outline is drawn
/// opaque, so it is dropped.
fn rgb_from_abgr(abgr: u32) -> u32 {
    ((abgr & 0x00_00_00_FF) << 16) | (abgr & 0x00_00_FF_00) | ((abgr & 0x00_FF_00_00) >> 16)
}

#[cfg(test)]
#[path = "tests/window_border_tests.rs"]
mod tests;
