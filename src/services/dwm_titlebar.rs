//! Windows-only DWM titlebar styling for native-titlebar mode.
//!
//! Calls `DwmSetWindowAttribute` to:
//! - Flip the focused caption between light and dark via `DWMWA_USE_IMMERSIVE_DARK_MODE`, so dark
//!   themes get a File-Explorer-style caption instead of the default light Win32 one.
//! - Paint the caption in the app's mantle via `DWMWA_CAPTION_COLOR`, suppressing Windows'
//!   system-wide "Show accent color on title bars" override.
//!
//! **It takes an `HWND`, not a window.** Both callers already sit above this module and can do
//! the `WinitWindowAccessor` hop themselves, and keeping it on their side is what leaves this file
//! naming no Slint type at all; `ui::window_chrome::win32_hwnd` is the one place that hop is
//! written. Applied after window-show from `main`, and again at the end of every
//! `ui::appearance::theme_apply::write_palette`, so theme / variant / accent changes update the
//! caption live.
//!
//! **Fails open**: every `DwmSetWindowAttribute` error is logged and dropped. Pre-Win11 22000
//! builds reject `DWMWA_CAPTION_COLOR` with `E_INVALIDARG` while the immersive-dark flag still
//! works on Win10 20H1+, so the dark caption survives on older releases.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};

/// Pre-20H1 builds (Windows 10 1903–1909) expose the immersive-dark attribute at index 19 instead
/// of 20. Try 20 first and fall back — one of the two succeeds on every build that knows the
/// attribute at all.
const DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1: u32 = 19;

/// Push DWM titlebar attributes onto a shown window's `HWND`.
///
/// `caption_rgb` is `0x00_RR_GG_BB` (Slint / CSS byte order), converted to the BGR `COLORREF`
/// Windows expects. Pass `Theme.mantle` so the caption matches the chrome below it exactly; the
/// dark/light flag is derived from that same value's relative luminance, so one value drives both.
///
/// The caller owns getting the handle, and owns the pre-show case where there isn't one yet.
pub fn apply(hwnd: *mut c_void, caption_rgb: u32) {
    set_immersive_dark(hwnd, is_dark_from_rgb(caption_rgb));
    set_caption_color(hwnd, caption_rgb);
}

/// Relative-luminance dark/light check on a packed `0x00RRGGBB` colour. Same coefficients and
/// threshold as `themes::on_accent_hex` — **a deliberate third copy**, so this module owes
/// nothing to the palette code that calls *into* it.
///
/// `<=`, not `<`: both siblings split on `lum > 0.5` for *light*, so anything but the exact
/// complement disagrees with them on the colours that land on the threshold. Those are reachable —
/// a Material You mantle comes off album art. `services::dwm_titlebar::tests` pins the pair.
fn is_dark_from_rgb(rgb: u32) -> bool {
    let r = f64::from((rgb >> 16) & 0xff) / 255.0;
    let g = f64::from((rgb >> 8) & 0xff) / 255.0;
    let b = f64::from(rgb & 0xff) / 255.0;
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    lum <= 0.5
}

#[allow(
    unsafe_code,
    reason = "FFI to DwmSetWindowAttribute. Both pointer parameters target stack-local primitives whose sizes match the `cbAttribute` argument; the API takes a `const void*` and never retains the pointer past the call."
)]
fn set_immersive_dark(hwnd: *mut c_void, is_dark: bool) {
    // `BOOL` in the Windows SDK is a 4-byte signed integer, and `cbAttribute` takes a `u32` — the
    // literal dodges the `cast_possible_truncation` lint `size_of::<i32>() as u32` would trigger.
    let value: i32 = i32::from(is_dark);
    let pv: *const c_void = std::ptr::from_ref::<i32>(&value).cast::<c_void>();
    let size: u32 = 4;

    // SAFETY: `pv` points at a stack-local `i32` and `size` matches its actual size. `hwnd` is the
    // caller's to keep live for the call — every path in reaches it through
    // `ui::window_chrome::win32_hwnd`, which reads it off the live winit window. The API does not
    // retain the pointer.
    let hr = unsafe {
        DwmSetWindowAttribute(hwnd as HWND, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, pv, size)
    };
    if hr >= 0 {
        return;
    }

    // SAFETY: same contract as the first call — only the attribute id changes.
    let hr_legacy = unsafe {
        DwmSetWindowAttribute(hwnd as HWND, DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1, pv, size)
    };
    if hr_legacy < 0 {
        log::warn!(
            "DWMWA_USE_IMMERSIVE_DARK_MODE failed (hr=0x{hr:08x}, legacy_hr=0x{hr_legacy:08x})"
        );
    }
}

#[allow(
    unsafe_code,
    reason = "FFI to DwmSetWindowAttribute. The `pvAttribute` pointer targets a stack-local COLORREF (u32) whose size matches `cbAttribute`."
)]
fn set_caption_color(hwnd: *mut c_void, rgb: u32) {
    // COLORREF is 0x00_BB_GG_RR; the caller hands us 0x00_RR_GG_BB.
    let colorref: u32 =
        ((rgb & 0x00_FF_00_00) >> 16) | (rgb & 0x00_00_FF_00) | ((rgb & 0x00_00_00_FF) << 16);
    let pv: *const c_void = std::ptr::from_ref::<u32>(&colorref).cast::<c_void>();
    // COLORREF is a `u32`; the literal dodges the `cast_possible_truncation` lint.
    let size: u32 = 4;

    // SAFETY: `pv` targets a stack-local `u32`, `size` matches, and `hwnd` is live for the
    // call by the same contract as `set_immersive_dark`'s.
    let hr = unsafe { DwmSetWindowAttribute(hwnd as HWND, DWMWA_CAPTION_COLOR as u32, pv, size) };
    if hr < 0 {
        // Pre-Win11 22000 builds reject this attribute. The immersive-dark call already handled
        // the dark/light variant, and what is lost is the override of a Win10-absent preference.
        log::debug!("DWMWA_CAPTION_COLOR not supported (hr=0x{hr:08x})");
    }
}

#[cfg(test)]
#[path = "tests/dwm_titlebar_tests.rs"]
mod tests;
