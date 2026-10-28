//! The window's settings on disk. A setter here only commits: the live effect is its UI
//! callback's, applied before the write, except where the doc says it is restart-gated or, for
//! the pin, applied here.

use std::sync::Arc;

use crate::services::{
    self,
    settings::{BorderTint, TitlebarButtonSide, TitlebarButtonStyle, WindowBorder},
};
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_platform::services::platform::always_on_top::AlwaysOnTopMethod;
use melodia_platform::services::platform::desktop;

/// Apply the pin, through the D-Bus backends on Linux (elsewhere the UI callback has set winit's
/// level), then persist it. A failed apply persists nothing, and the caller reverts its optimistic
/// toggle on any error.
pub async fn set_always_on_top(state: &AppState, pinned: bool) -> Result<(), AppError> {
    apply_then_persist(&state.paths, state.always_on_top.method, pinned).await
}

/// Narrowed to what it reaches, so the ordering can be driven against a desktop that supports
/// nothing.
async fn apply_then_persist(
    paths: &Arc<Paths>,
    method: AlwaysOnTopMethod,
    pinned: bool,
) -> Result<(), AppError> {
    melodia_platform::services::platform::always_on_top::apply(method, &paths.data_dir, pinned)
        .await?;
    record_always_on_top(paths, pinned).await
}

/// Persist a pin the window manager changed on its own, `KWin`'s keep-above titlebar button among
/// them, so a launch restores the last choice wherever it was made. There is nothing to apply: the
/// window already carries it.
pub async fn record_always_on_top(paths: &Arc<Paths>, pinned: bool) -> Result<(), AppError> {
    let paths = Arc::clone(paths);
    tokio::task::spawn_blocking(move || {
        services::settings::mutate_settings(&paths, |s| {
            s.window.always_on_top = pinned;
        })
    })
    .await
    .map_err(|e| AppError::io("persist always_on_top join", e))?
}

/// Restart-gated: the respawn that follows hydrates it before first show.
///
/// On KDE, turning the native titlebar on also turns on `match_unfocused_to_system_bg` in the same
/// write, the tint mirroring KDE's decoration fade and meaning nothing without it. Turning it off
/// leaves the flag alone: the Slint bindings already gate the tint on the native titlebar, and the
/// value survives for next time.
pub fn set_use_native_titlebar(paths: &Paths, on: bool) -> Result<(), AppError> {
    write_use_native_titlebar(paths, on, desktop::is_kde_desktop())
}

/// Takes the desktop probe rather than reading it, so a test steers the second write without the
/// environment.
fn write_use_native_titlebar(paths: &Paths, on: bool, is_kde: bool) -> Result<(), AppError> {
    let enable_match_unfocused = on && is_kde;
    services::settings::mutate_settings(paths, move |s| {
        s.window.use_native_titlebar = on;
        if enable_match_unfocused {
            s.layout.match_unfocused_to_system_bg = true;
        }
    })
}

pub fn set_close_to_tray(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.tray.close_to_tray = on;
    })
}

/// Restart-gated: boot skips `ui::shell::tray_bridge::install` while it is off, so the tray's D-Bus
/// connection and thread never run.
pub fn set_tray_enabled(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.tray.tray_enabled = on;
    })
}

/// Restart-gated: `boot::ui_setup::chrome::apply_backdrop_style` reads it before the first artwork
/// tier exists, deciding whether any blur is built.
pub fn set_aurora_backdrop(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.backdrop.aurora_backdrop = on;
    })
}

/// **Not restart-gated**, unlike [`set_aurora_backdrop`]: that one decides which stack every colour
/// tier is solved for, where this only mounts or unmounts the stack it chose.
pub fn set_mini_backdrop(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.backdrop.mini_backdrop = on;
    })
}

pub fn set_titlebar_button_style(
    paths: &Paths,
    style: TitlebarButtonStyle,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.titlebar_button_style = style;
    })
}

pub fn set_mini_player_button_style(
    paths: &Paths,
    style: TitlebarButtonStyle,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.mini_player_button_style = style;
    })
}

pub fn set_titlebar_button_side(paths: &Paths, side: TitlebarButtonSide) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.titlebar_button_side = side;
    })
}

pub fn set_window_border(paths: &Paths, border: WindowBorder) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.window_border = border;
    })
}

/// `WINDOW_BORDER_SYSTEM_COLOR`, or an accent id of the active theme.
pub fn set_window_border_color(paths: &Paths, color_id: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.window_border_color = color_id;
    })
}

/// What the System colour is mixed from under Material You.
pub fn set_window_border_tint(paths: &Paths, tint: BorderTint) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.window.window_border_tint = tint;
    })
}

#[cfg(test)]
#[path = "tests/window_tests.rs"]
mod tests;
