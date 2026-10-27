//! The native titlebar's DWM styling on Windows: a dark or light caption through
//! `DWMWA_USE_IMMERSIVE_DARK_MODE`, painted in the app's mantle through `DWMWA_CAPTION_COLOR`,
//! which also overrides the "Show accent color on title bars" preference.
//!
//! **It takes an `HWND`, not a window**, so this file names no Slint type;
//! `ui::window_chrome::win32_hwnd` is the one place that hop is written. **Fails open**: a
//! pre-22000 build rejects the caption colour while the dark flag still works from Windows 10
//! 20H1, so errors are logged and dropped.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};

/// Windows 10 1903–1909 put the immersive-dark attribute at 19 rather than 20, so 20 is tried
/// first and this second.
const DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1: u32 = 19;

/// `caption_rgb` is `0x00RRGGBB`, converted to the BGR `COLORREF` Windows takes. Pass
/// `Theme.mantle`: the dark flag is derived from the same value, so one colour drives both.
pub fn apply(hwnd: *mut c_void, caption_rgb: u32) {
    set_immersive_dark(hwnd, is_dark_from_rgb(caption_rgb));
    set_caption_color(hwnd, caption_rgb);
}

/// The coefficients and threshold of `themes::on_accent_hex`, **a deliberate third copy** so this
/// module owes nothing to the palette code calling into it. `<=` because both siblings split on
/// `lum > 0.5` for light, and a Material You mantle can land on the threshold.
/// `services::platform::dwm_titlebar::tests` pins the pair.
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
    // A Win32 `BOOL` is four bytes; the literal dodges the `cast_possible_truncation` a `size_of`
    // would trip.
    let value: i32 = i32::from(is_dark);
    let pv: *const c_void = std::ptr::from_ref::<i32>(&value).cast::<c_void>();
    let size: u32 = 4;

    // SAFETY: `pv` points at a stack-local `i32` and `size` matches its size. `hwnd` is live for
    // the call, every path reaching it through `ui::window_chrome::win32_hwnd` off the live winit
    // window. The API does not retain the pointer.
    let hr = unsafe {
        DwmSetWindowAttribute(hwnd as HWND, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, pv, size)
    };
    if hr >= 0 {
        return;
    }

    // SAFETY: the first call's contract; only the attribute id changes.
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
    // COLORREF is 0x00BBGGRR.
    let colorref: u32 =
        ((rgb & 0x00_FF_00_00) >> 16) | (rgb & 0x00_00_FF_00) | ((rgb & 0x00_00_00_FF) << 16);
    let pv: *const c_void = std::ptr::from_ref::<u32>(&colorref).cast::<c_void>();
    let size: u32 = 4;

    // SAFETY: `pv` targets a stack-local `u32`, `size` matches, and `hwnd` is live for the call by
    // `set_immersive_dark`'s contract.
    let hr = unsafe { DwmSetWindowAttribute(hwnd as HWND, DWMWA_CAPTION_COLOR as u32, pv, size) };
    if hr < 0 {
        // Pre-22000; the dark flag already took, and what is lost overrides a preference Windows
        // 10 doesn't have.
        log::debug!("DWMWA_CAPTION_COLOR not supported (hr=0x{hr:08x})");
    }
}

#[cfg(test)]
#[path = "tests/dwm_titlebar_tests.rs"]
mod tests;
