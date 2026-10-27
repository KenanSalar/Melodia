//! The window rows outside the theme, variant and accent: the unfocused tint, the corner radius,
//! the caption buttons, close to tray and the overflow menu's buttons, each independent of the
//! others and of Material You.

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::services;
use melodia_app::services::settings::{TitlebarButtonSide, TitlebarButtonStyle};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, MiniPlayer, Settings, Theme};

/// The binding has already flipped the property the sidebar and the bar read, so only the persist
/// is left.
pub(super) fn wire_match_unfocused_bg_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_match_unfocused_bg_changed(move |on| {
        s.persist_blocking("persist match_unfocused_to_system_bg", move |paths| {
            library::settings::set_match_unfocused_to_system_bg(paths, on)
        });
    });
}

/// `px` clamped, then snapped to the nearest chip preset, so the painted radius and the
/// persisted value can't diverge from the chip group's set. The boot seed and the row share it.
pub(super) fn corner_radius_preset(px: u32) -> u32 {
    library::settings::snap_to_preset(px.min(services::settings::MAX_CORNER_RADIUS))
}

/// Applied synchronously so the shell and the panel repaint at once, then persisted. No shadow,
/// nothing else touching `corner_radius`, and no kick, nothing re-reading the file for it.
pub(super) fn wire_corner_radius_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_corner_radius_changed(move |px_i32| {
        let Some(ui) = weak.upgrade() else { return };
        // The chips emit presets only; this holds for a caller that doesn't.
        let radius = corner_radius_preset(u32::try_from(px_i32).unwrap_or(0));
        #[allow(
            clippy::cast_precision_loss,
            reason = "snapped to {0,6,8,10,15}: exact f32 representation"
        )]
        ui.global::<Theme>().set_shell_radius(radius as f32);
        s.persist_blocking("persist corner_radius", move |paths| {
            library::settings::set_corner_radius(paths, radius)
        });
    });
}

/// Mirrored into the atomic `window_chrome`'s close handlers read, synchronously so the next close
/// honours it, then persisted.
pub(super) fn wire_close_to_tray_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_close_to_tray_changed(move |on| {
        crate::ui::shell::tray_bridge::set_close_to_tray(on);
        s.persist_blocking("persist close_to_tray", move |paths| {
            library::window::set_close_to_tray(paths, on)
        });
    });
}

/// Writes `Theme.titlebar-button-style` synchronously so the titlebar reflows at once, then
/// persists.
pub(super) fn wire_titlebar_button_style_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_titlebar_button_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let style = style_for(idx);
        ui.global::<Theme>().set_titlebar_button_style(idx_for(style));
        s.persist_blocking("persist titlebar_button_style", move |paths| {
            library::window::set_titlebar_button_style(paths, style)
        });
    });
}

/// [`wire_titlebar_button_style_changed`]'s shape, over `MiniPlayer.button-style`.
pub(super) fn wire_mini_player_button_style_changed(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_mini_player_button_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let style = style_for(idx);
        ui.global::<MiniPlayer>().set_button_style(idx_for(style));
        s.persist_blocking("persist mini_player_button_style", move |paths| {
            library::window::set_mini_player_button_style(paths, style)
        });
    });
}

/// [`wire_titlebar_button_style_changed`]'s shape, over the side.
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
        s.persist_blocking("persist titlebar_button_side", move |paths| {
            library::window::set_titlebar_button_side(paths, side)
        });
    });
}

/// The chip index back to a style, the inverse of [`idx_for`], shared by the titlebar's row and the
/// miniplayer's so the two can't read one chip two ways.
fn style_for(idx: i32) -> TitlebarButtonStyle {
    match idx {
        1 => TitlebarButtonStyle::Macos,
        2 => TitlebarButtonStyle::Kde,
        _ => TitlebarButtonStyle::Standard,
    }
}

/// The style as the chip index every Slint property carrying it holds, shared by the seed and the
/// rows.
pub(super) fn idx_for(style: TitlebarButtonStyle) -> i32 {
    match style {
        TitlebarButtonStyle::Standard => 0,
        TitlebarButtonStyle::Macos => 1,
        TitlebarButtonStyle::Kde => 2,
    }
}

/// [`idx_for`] for the side enum.
pub(super) fn idx_for_side(side: TitlebarButtonSide) -> i32 {
    match side {
        TitlebarButtonSide::Right => 0,
        TitlebarButtonSide::Left => 1,
    }
}

/// `OverflowCheckCell` has already flipped `Settings.overflow-<id>` and the bar repainted, so only
/// the persist is left.
pub(super) fn wire_overflow_buttons_changed(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_overflow_buttons_changed(move |id, on| {
        let id_str = id.to_string();
        s.persist_blocking("persist overflow_buttons", move |paths| {
            library::settings::set_overflow_button(paths, id_str, on)
        });
    });
}

#[cfg(test)]
#[path = "tests/window_settings_tests.rs"]
mod tests;
