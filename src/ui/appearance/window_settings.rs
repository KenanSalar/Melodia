//! Window-chrome settings wires that don't fit the theme / variant /
//! accent triad: match-unfocused background tint (KDE-only),
//! corner-radius chip group, and the overflow-menu button toggles. Each
//! is independent of the others and of Material You — they share this
//! file purely as "the smaller appearance-section toggles".

use slint::ComponentHandle;

use crate::library;
use crate::services;
use crate::services::settings::{TitlebarButtonSide, TitlebarButtonStyle};
use crate::state::AppState;
use crate::{AppWindow, Settings, Theme};

/// Wire the "Match Unfocused Window Background" toggle (KDE-only).
/// The two-way Slint binding has already flipped `Settings.match-unfocused-bg`
/// before this fires; the sidebar / now-playing-bar bindings consult
/// `(Settings.match-unfocused-bg && !Theme.window-focused)` directly, so
/// no Rust-side state mirror is needed. Persist the new value
/// asynchronously through `mutate_settings`.
pub(super) fn wire_match_unfocused_bg_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_match_unfocused_bg_changed(move |on| {
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::settings::set_match_unfocused_to_system_bg(&s_clone, on) {
                log::warn!("persist match_unfocused_to_system_bg: {e}");
            }
        });
    });
}

/// Wire the Window Corner Radius row's `corner-radius-changed` callback.
/// Mirrors `wire_match_unfocused_bg_changed`'s shape: clamp the value,
/// apply the runtime effect synchronously (so the rounded outer shell +
/// inner panel repaint immediately), then spawn the disk write on the
/// tokio blocking pool. No synchronous shadow needed — no other callback
/// reads or writes `settings.corner_radius`. No coordinator kick needed —
/// the only consumer (`Theme.shell-radius`) is updated synchronously here,
/// not by a downstream task re-reading `settings.json`.
pub(super) fn wire_corner_radius_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_corner_radius_changed(move |px_i32| {
        let Some(ui) = weak.upgrade() else { return };
        // Clamp first, then snap to the nearest chip preset so the
        // painted radius and the persisted value can never diverge from
        // the chip-group set {0, 6, 8, 10, 15}. The chip group only
        // emits valid presets in practice — the snap is defensive
        // against future code paths that might forward arbitrary px
        // values through this callback.
        let clamped = u32::try_from(px_i32)
            .unwrap_or(0)
            .min(services::settings::MAX_CORNER_RADIUS);
        let radius = library::settings::snap_to_preset(clamped);
        // Apply: drive `Theme.shell-radius` synchronously. Slint length
        // properties codegen as `f32` in logical pixels.
        #[allow(
            clippy::cast_precision_loss,
            reason = "snapped to {0,6,8,10,15}: exact f32 representation"
        )]
        ui.global::<Theme>().set_shell_radius(radius as f32);
        // Persist on a blocking worker — `mutate_settings` does a
        // synchronous file rewrite that we don't want on the UI thread.
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::settings::set_corner_radius(&s_clone, radius) {
                log::warn!("persist corner_radius: {e}");
            }
        });
    });
}

/// Wire the "Close to Tray" toggle. The two-way Slint binding has already
/// flipped `Settings.close-to-tray` before this fires; we mirror the value
/// into the process-global atomic that `window_chrome`'s close handlers
/// read — synchronously, so the very next window-close honours it — then
/// persist on the blocking pool. No coordinator kick: nothing downstream
/// re-reads `settings.json` for this field.
pub(super) fn wire_close_to_tray_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_close_to_tray_changed(move |on| {
        crate::ui::shell::tray_bridge::set_close_to_tray(on);
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_close_to_tray(&s_clone, on) {
                log::warn!("persist close_to_tray: {e}");
            }
        });
    });
}

/// Wire the Overflow Menu Buttons row's `overflow-buttons-changed`
/// callback. Each click in the Appearance section fires
/// `(id, overflow-on)`; the matching `Settings.overflow-<id>` bool has
/// already been flipped synchronously inside the `OverflowCheckCell`
/// before this fires, so the now-playing bar repaints immediately.
/// We just persist the new state through `library::settings`. No
/// shadow needed (no sibling callback reads `overflow_buttons` before
/// the disk write commits); no coordinator kick needed (no downstream
/// task re-reads `settings.json` for this field).
/// Wire the Decoration Button Style chip group (Standard / macOS).
/// Mirrors `wire_corner_radius_changed`'s shape: apply the runtime
/// effect synchronously by writing the matching `Theme.titlebar-button-style`
/// token (so `custom-titlebar.slint` reflows immediately), then spawn the
/// disk write on the tokio blocking pool. No coordinator kick — the
/// titlebar reads from `Theme` directly, no downstream task re-reads
/// `settings.json`.
pub(super) fn wire_titlebar_button_style_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_titlebar_button_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let style = match idx {
            1 => TitlebarButtonStyle::Macos,
            _ => TitlebarButtonStyle::Standard,
        };
        ui.global::<Theme>()
            .set_titlebar_button_style(idx_for(style));
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_titlebar_button_style(&s_clone, style) {
                log::warn!("persist titlebar_button_style: {e}");
            }
        });
    });
}

/// Wire the Decoration Button Side chip group (Right / Left). Same shape
/// as `wire_titlebar_button_style_changed`.
pub(super) fn wire_titlebar_button_side_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_titlebar_button_side_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let side = match idx {
            1 => TitlebarButtonSide::Left,
            _ => TitlebarButtonSide::Right,
        };
        ui.global::<Theme>()
            .set_titlebar_button_side(idx_for_side(side));
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_titlebar_button_side(&s_clone, side) {
                log::warn!("persist titlebar_button_side: {e}");
            }
        });
    });
}

/// Slint stores titlebar style as an int — keep the enum-to-int mapping
/// here so the install / wire paths agree. 0 = Standard, 1 = macOS.
pub(super) fn idx_for(style: TitlebarButtonStyle) -> i32 {
    match style {
        TitlebarButtonStyle::Standard => 0,
        TitlebarButtonStyle::Macos => 1,
    }
}

/// Same as `idx_for` but for the side enum. 0 = Right, 1 = Left.
pub(super) fn idx_for_side(side: TitlebarButtonSide) -> i32 {
    match side {
        TitlebarButtonSide::Right => 0,
        TitlebarButtonSide::Left => 1,
    }
}

pub(super) fn wire_overflow_buttons_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_overflow_buttons_changed(move |id, on| {
        let s_clone = s.clone();
        let id_str = id.to_string();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::settings::set_overflow_button(&s_clone, id_str, on) {
                log::warn!("persist overflow_buttons: {e}");
            }
        });
    });
}
