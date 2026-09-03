//! Window-chrome settings wires that don't fit the theme / variant /
//! accent triad: match-unfocused background tint (KDE-only),
//! corner-radius chip group, and the overflow-menu button toggles. Each
//! is independent of the others and of Material You — they share this
//! file purely as "the smaller appearance-section toggles".

use slint::ComponentHandle;

use crate::{AppWindow, Settings, Theme};
use melodia_app::library;
use melodia_app::services;
use melodia_app::services::settings::{TitlebarButtonSide, TitlebarButtonStyle};
use melodia_app::state::AppState;

/// Wire the KDE-only "Match Unfocused Window Background" toggle. The two-way binding
/// has already flipped the property, and the sidebar and bar bindings read it directly,
/// so no Rust mirror is needed — only the persist.
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

/// Wire the Window Corner Radius row: clamp, apply synchronously so the shell and inner
/// panel repaint at once, then persist on the blocking pool. No shadow — nothing else
/// touches `settings.corner_radius` — and no coordinator kick, `Theme.shell-radius`
/// being written here rather than by a task re-reading the file.
pub(super) fn wire_corner_radius_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_corner_radius_changed(move |px_i32| {
        let Some(ui) = weak.upgrade() else { return };
        // Clamp, then snap to the nearest chip preset, so the painted radius and the
        // persisted value can't diverge from the chip group's set. Defensive — the chip
        // group only emits valid presets — against a later path forwarding arbitrary px.
        let clamped = u32::try_from(px_i32).unwrap_or(0).min(services::settings::MAX_CORNER_RADIUS);
        let radius = library::settings::snap_to_preset(clamped);
        // Slint length properties codegen as `f32` logical pixels.
        #[allow(
            clippy::cast_precision_loss,
            reason = "snapped to {0,6,8,10,15}: exact f32 representation"
        )]
        ui.global::<Theme>().set_shell_radius(radius as f32);
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::settings::set_corner_radius(&s_clone, radius) {
                log::warn!("persist corner_radius: {e}");
            }
        });
    });
}

/// Wire the "Close to Tray" toggle: mirror the value into the process-global atomic
/// `window_chrome`'s close handlers read — synchronously, so the very next close honours
/// it — then persist. No coordinator kick; nothing re-reads the file for this field.
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

/// Wire the Decoration Button Style chip group: write the matching
/// `Theme.titlebar-button-style` token synchronously so `custom-titlebar.slint` reflows
/// at once, then persist. No coordinator kick — the titlebar reads `Theme` directly.
pub(super) fn wire_titlebar_button_style_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_titlebar_button_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let style = match idx {
            1 => TitlebarButtonStyle::Macos,
            _ => TitlebarButtonStyle::Standard,
        };
        ui.global::<Theme>().set_titlebar_button_style(idx_for(style));
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_titlebar_button_style(&s_clone, style) {
                log::warn!("persist titlebar_button_style: {e}");
            }
        });
    });
}

/// Wire the Decoration Button Side chip group, in `wire_titlebar_button_style_changed`'s
/// shape.
pub(super) fn wire_titlebar_button_side_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_titlebar_button_side_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let side = match idx {
            1 => TitlebarButtonSide::Left,
            _ => TitlebarButtonSide::Right,
        };
        ui.global::<Theme>().set_titlebar_button_side(idx_for_side(side));
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_titlebar_button_side(&s_clone, side) {
                log::warn!("persist titlebar_button_side: {e}");
            }
        });
    });
}

/// Slint stores the titlebar style as an int, so the enum-to-int mapping lives here and
/// the install and wire paths agree on it.
pub(super) fn idx_for(style: TitlebarButtonStyle) -> i32 {
    match style {
        TitlebarButtonStyle::Standard => 0,
        TitlebarButtonStyle::Macos => 1,
    }
}

/// [`idx_for`] for the side enum.
pub(super) fn idx_for_side(side: TitlebarButtonSide) -> i32 {
    match side {
        TitlebarButtonSide::Right => 0,
        TitlebarButtonSide::Left => 1,
    }
}

/// Wire the Overflow Menu Buttons row. `OverflowCheckCell` has already flipped the
/// matching `Settings.overflow-<id>` bool synchronously, so the now-playing bar has
/// repainted and only the persist is left. No shadow, no coordinator kick.
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
