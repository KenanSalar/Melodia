use crate::error::AppError;
use crate::services::{
    self,
    always_on_top::AlwaysOnTopCapability,
    settings::{TitlebarButtonSide, TitlebarButtonStyle},
};
use crate::state::AppState;

pub fn get_always_on_top_capability(state: &AppState) -> Result<AlwaysOnTopCapability, AppError> {
    Ok(state.always_on_top.clone())
}

/// Apply the user's pinned choice and persist it. On Linux this drops
/// into the `KWin` / GNOME D-Bus backends via
/// `services::always_on_top::apply`; on macOS / Windows the UI callback
/// already pushed `WindowLevel::AlwaysOnTop` to winit synchronously, so
/// here we just persist. Returns `AppError::Window` when the desktop
/// has no supported method — callers use that to revert the optimistic
/// toggle they performed on the UI thread.
pub async fn set_always_on_top(state: &AppState, pinned: bool) -> Result<(), AppError> {
    services::always_on_top::apply(state, pinned).await?;
    let paths = state.paths.clone();
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
    let enable_match_unfocused = on && services::settings::is_kde_desktop();
    services::settings::mutate_settings(&state.paths, move |s| {
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
