use std::sync::Arc;

use crate::services::{
    self,
    settings::{TitlebarButtonSide, TitlebarButtonStyle},
};
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_platform::services::platform::always_on_top::AlwaysOnTopMethod;

/// Apply the user's pinned choice and persist it. On Linux this drops
/// into the `KWin` / GNOME D-Bus backends via
/// `melodia_platform::services::platform::always_on_top::apply`; on macOS / Windows the UI callback
/// already pushed `WindowLevel::AlwaysOnTop` to winit synchronously, so
/// here we just persist. Returns `AppError::Window` when the desktop
/// has no supported method — callers use that to revert the optimistic
/// toggle they performed on the UI thread.
pub async fn set_always_on_top(state: &AppState, pinned: bool) -> Result<(), AppError> {
    apply_then_persist(&state.paths, state.always_on_top.method, pinned).await
}

/// [`set_always_on_top`]'s body, narrowed to the paths and the method it reaches so the ordering
/// can be driven against a desktop that supports nothing.
async fn apply_then_persist(
    paths: &Arc<Paths>,
    method: AlwaysOnTopMethod,
    pinned: bool,
) -> Result<(), AppError> {
    melodia_platform::services::platform::always_on_top::apply(method, &paths.data_dir, pinned)
        .await?;
    let paths = Arc::clone(paths);
    tokio::task::spawn_blocking(move || {
        services::settings::mutate_settings(&paths, |s| {
            s.window.always_on_top = pinned;
        })
    })
    .await
    .map_err(|e| AppError::Settings(format!("set_always_on_top join: {e}")))?
}

/// Persist the user's titlebar choice. Slint reads `Window.no-frame` once
/// at first show, so this only commits the new value to disk — the
/// caller (`window_chrome::on_restart_app`) respawns the binary so the
/// next process picks up the new setting at construction time.
///
/// On KDE, enabling the native titlebar also turns on
/// `match_unfocused_to_system_bg` in the same write: the unfocused-tint
/// feature mirrors KDE's window-decoration fade and is only meaningful
/// under the native titlebar, so it ships on by default the moment that
/// mode becomes active. Both fields land in one `mutate_settings` call
/// so the respawned process reads a consistent pair. Disabling the
/// native titlebar leaves the flag untouched — the runtime gate lives
/// in the Slint binding sites (`sidebar.slint` / `now-playing-bar.slint`
/// both check `Settings.match-unfocused-bg && Theme.use-native-titlebar
/// && !Theme.window-focused`), so the tint is suppressed automatically
/// in custom-titlebar mode while the persisted value survives for the
/// next time the native titlebar is enabled.
pub fn set_use_native_titlebar(state: &AppState, on: bool) -> Result<(), AppError> {
    write_use_native_titlebar(&state.paths, on, services::settings::is_kde_desktop())
}

/// [`set_use_native_titlebar`]'s body, with the desktop probe passed in rather than read: the
/// second write is the whole decision, and a test that steered it through `XDG_CURRENT_DESKTOP`
/// would be testing the environment as well.
fn write_use_native_titlebar(paths: &Paths, on: bool, is_kde: bool) -> Result<(), AppError> {
    let enable_match_unfocused = on && is_kde;
    services::settings::mutate_settings(paths, move |s| {
        s.window.use_native_titlebar = on;
        if enable_match_unfocused {
            s.layout.match_unfocused_to_system_bg = true;
        }
    })
}

/// Persist the user toggle for "Close to tray". When `true` (and a tray icon
/// is actually active) closing the window hides it to the system tray
/// instead of quitting. The runtime effect — `window_chrome`'s close
/// handlers consulting the value — is applied synchronously by the UI
/// callback through `ui::shell::tray_bridge::set_close_to_tray` *before* this
/// async disk write, so the new behaviour takes effect immediately.
pub fn set_close_to_tray(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.tray.close_to_tray = on;
    })
}

/// Persist the user toggle for "System Tray Icon". When `false` (the
/// default) `main.rs` skips `ui::shell::tray_bridge::install` at startup, so the
/// tray subsystem — D-Bus connection, service thread, action tasks — never
/// runs. The toggle is restart-gated through the `restart-tray` `Dialog`
/// flow, so this write commits just before the process respawns and the new
/// value takes effect on the next launch.
pub fn set_tray_enabled(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.tray.tray_enabled = on;
    })
}

/// Persist the user toggle for "Aurora Backdrop". When `false` (the default) the
/// artwork-derived surfaces blur the cover behind them, and the two artwork tiers build a
/// blurred half per decode; when `true` they wash the cover's own colours over `Theme.base`
/// and no blur is built at all. Restart-gated through the `restart-backdrop` `Dialog` flow —
/// `boot::ui_setup::apply_backdrop_style` is what reads it, before the first tier exists.
pub fn set_aurora_backdrop(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.backdrop.aurora_backdrop = on;
    })
}

/// Persist the user's pick for the custom titlebar's decoration button
/// style (Standard vs macOS traffic lights). The runtime effect lives in
/// `Theme.titlebar-button-style`, mirrored synchronously by the UI
/// callback before this fires, so the visual swap happens immediately;
/// here we only commit the new value to disk.
pub fn set_titlebar_button_style(
    state: &AppState,
    style: TitlebarButtonStyle,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.window.titlebar_button_style = style;
    })
}

/// Persist the user's pick for the custom titlebar's decoration button
/// side (Right vs Left). Same async-only shape as
/// `set_titlebar_button_style` — `Theme.titlebar-button-side` is mirrored
/// synchronously by the UI callback so the buttons reposition before this
/// disk write commits.
pub fn set_titlebar_button_side(
    state: &AppState,
    side: TitlebarButtonSide,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.window.titlebar_button_side = side;
    })
}

#[cfg(test)]
#[path = "tests/window_tests.rs"]
mod tests;
