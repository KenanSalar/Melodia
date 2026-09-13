//! The colour Windows draws its own window borders in, for the outline a frameless window draws
//! where that border would be.
//!
//! Windows paints a border in the accent colour only while "Show accent color on title bars and
//! window borders" is on, and a neutral one otherwise. Both halves of that answer are in the DWM
//! registry key: `ColorPrevalence` is the switch, and `AccentColor` the colour it applies.
//!
//! **Fails open**: a key or value that won't read answers `None`, the neutral border, which is what
//! Windows draws whenever the accent isn't applied.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::core::{PCWSTR, w};

/// Returns the accent colour Windows borders its windows in, as `0x00RRGGBB`, or `None` when the
/// user has the accent off borders and they stay neutral.
pub fn accent_border_rgb() -> Option<u32> {
    accent_border(read_dwm_dword(w!("ColorPrevalence")), read_dwm_dword(w!("AccentColor")))
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

#[allow(
    unsafe_code,
    reason = "FFI to RegGetValueW. The data pointer targets a stack-local u32 whose size is the one handed in `pcbData`, RRF_RT_REG_DWORD refuses any value that isn't a 4-byte DWORD, and both name pointers are NUL-terminated literals."
)]
fn read_dwm_dword(name: PCWSTR) -> Option<u32> {
    let mut value: u32 = 0;
    // A DWORD is four bytes; the literal dodges the `cast_possible_truncation` a `size_of` would.
    let mut size: u32 = 4;

    // SAFETY: the sub-key and `name` are NUL-terminated `w!` literals that live for the call.
    // `value` is a stack-local `u32` and `size` names its exact size, and `RRF_RT_REG_DWORD` makes
    // the call fail rather than write a value of any other type or length. The type out-parameter
    // may be null, and the API retains neither pointer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\DWM"),
            name,
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut::<u32>(&mut value).cast(),
            std::ptr::from_mut::<u32>(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

#[cfg(test)]
#[path = "tests/window_border_tests.rs"]
mod tests;
