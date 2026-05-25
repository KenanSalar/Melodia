//! Windows-only DWM titlebar styling for native-titlebar mode.
//!
//! Calls `DwmSetWindowAttribute` to:
//! - Flip the focused caption between light and dark via
//!   `DWMWA_USE_IMMERSIVE_DARK_MODE` so dark themes get a File-Explorer-style
//!   dark caption instead of the default light Win32 caption.
//! - Paint the caption in the app's mantle colour via `DWMWA_CAPTION_COLOR`,
//!   suppressing Windows' system-wide "Show accent color on title bars"
//!   override that otherwise repaints the caption with the user's accent
//!   when the window is focused.
//!
//! The same call site fires twice: once after window-show in
//! [`crate::main`] (alongside the SMTC attach), and once at the end of every
//! [`crate::themes::apply`] so theme / variant / accent / Material-You
//! changes update the caption live without a restart.
//!
//! Fails open: every `DwmSetWindowAttribute` error is logged and dropped.
//! Pre-Win11 22000 builds don't support `DWMWA_CAPTION_COLOR` (returns
//! `E_INVALIDARG`) but the immersive-dark flag still works on Win10 20H1+,
//! which keeps the dark caption working on the older releases.

use std::ffi::c_void;

use slint::ComponentHandle;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};

use crate::AppWindow;

/// Pre-20H1 builds (Windows 10 1903 – 1909) expose the immersive-dark
/// attribute at index 19 instead of 20. We attempt 20 first and fall back
/// to 19 if the API returns an error; one of the two succeeds on every
/// build that knows about the attribute at all.
const DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1: u32 = 19;

/// Push DWM titlebar attributes for the given Slint window.
///
/// - `caption_rgb`: `0x00_RR_GG_BB` (Slint / CSS byte order). Converted to
///   the BGR `COLORREF` Windows expects. Pass `Theme.mantle` so the
///   caption matches the painted chrome below it exactly. The dark /
///   light flag is derived from the same value's relative luminance —
///   dark themes have a dark mantle (mocha base sits at ~0.05 luminance),
///   light themes a light mantle (~0.95), so one value drives both.
///
/// No-op when the underlying winit window isn't `Win32` (e.g. the
/// pre-show boot path before `app.show()` runs). The HWND is fetched per
/// call rather than cached because Slint's `with_winit_window` accessor
/// only borrows for the closure's duration.
pub fn apply(app: &AppWindow, caption_rgb: u32) {
    let Some(hwnd) = win32_hwnd(app) else { return };
    set_immersive_dark(hwnd, is_dark_from_rgb(caption_rgb));
    set_caption_color(hwnd, caption_rgb);
}

/// Read `Theme.mantle` back from the Slint global and apply. Used at
/// startup (`main.rs`) once the window is shown — the boot-time
/// `themes::apply` already ran but its DWM hook no-op'd because the
/// HWND didn't exist yet. After this single re-apply, every subsequent
/// `themes::apply` (theme switch, accent change, Material You) drives
/// the DWM call directly from `write_palette`.
pub fn reapply_from_theme(app: &AppWindow) {
    use crate::Theme;
    let color = app.global::<Theme>().get_mantle().color();
    let rgb = (u32::from(color.red()) << 16)
        | (u32::from(color.green()) << 8)
        | u32::from(color.blue());
    apply(app, rgb);
}

/// Relative-luminance dark/light check on a packed `0x00RRGGBB` colour.
/// Same coefficients + threshold as `themes::apply::on_accent_hex`,
/// duplicated here so the windows-only module has no cross-module
/// dependency on the cross-platform palette code.
fn is_dark_from_rgb(rgb: u32) -> bool {
    let r = f64::from((rgb >> 16) & 0xff) / 255.0;
    let g = f64::from((rgb >> 8) & 0xff) / 255.0;
    let b = f64::from(rgb & 0xff) / 255.0;
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    lum < 0.5
}

/// Extract the native Win32 `HWND` from a shown Slint window. `None` until
/// the window has been shown for the first time; `None` if the active
/// winit backend isn't Win32 (impossible on Windows builds, but the API
/// surface stays honest).
fn win32_hwnd(app: &AppWindow) -> Option<*mut c_void> {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    app.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut c_void),
            _ => None,
        })
        .flatten()
}

#[allow(
    unsafe_code,
    reason = "FFI to DwmSetWindowAttribute. Both pointer parameters target stack-local primitives whose sizes match the `cbAttribute` argument; the API takes a `const void*` and never retains the pointer past the call."
)]
fn set_immersive_dark(hwnd: *mut c_void, is_dark: bool) {
    // `BOOL` in the Windows SDK is a 4-byte signed integer. The
    // `cbAttribute` argument takes a `u32` — literal 4 dodges the
    // `cast_possible_truncation` lint that `size_of::<i32>() as u32`
    // would trigger.
    let value: i32 = i32::from(is_dark);
    let pv: *const c_void = std::ptr::from_ref::<i32>(&value).cast::<c_void>();
    let size: u32 = 4;

    // SAFETY: `pv` points at a stack-local `i32` (4 bytes) and `size`
    // matches its actual size. The HWND comes from `win32_hwnd()` which
    // pulled it from the live winit window — it's valid for the lifetime
    // of this call.
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            pv,
            size,
        )
    };
    if hr >= 0 {
        return;
    }

    // SAFETY: same contract as the first call — only the attribute id
    // changes.
    let hr_legacy = unsafe {
        DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_USE_IMMERSIVE_DARK_MODE_PRE_20H1,
            pv,
            size,
        )
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
    let colorref: u32 = ((rgb & 0x00_FF_00_00) >> 16)
        | (rgb & 0x00_00_FF_00)
        | ((rgb & 0x00_00_00_FF) << 16);
    let pv: *const c_void = std::ptr::from_ref::<u32>(&colorref).cast::<c_void>();
    // COLORREF is u32 — 4 bytes. Literal dodges the
    // `cast_possible_truncation` lint.
    let size: u32 = 4;

    // SAFETY: `pv` targets a stack-local `u32` and `size` matches.
    let hr = unsafe {
        DwmSetWindowAttribute(hwnd as HWND, DWMWA_CAPTION_COLOR as u32, pv, size)
    };
    if hr < 0 {
        // Pre-Win11 22000 builds reject this attribute. The
        // immersive-dark call already handled the dark/light variant —
        // the only thing we lose is the override of Windows' system-wide
        // "accent on title bars" preference, which doesn't exist on
        // Win10 anyway.
        log::debug!("DWMWA_CAPTION_COLOR not supported (hr=0x{hr:08x})");
    }
}
