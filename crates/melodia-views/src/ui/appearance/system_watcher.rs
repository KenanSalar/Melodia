//! Keeps the OS appearance cache current and repaints a theme on its System variant when the OS
//! flips between light and dark. Linux hears it from the XDG portal; Windows re-reads its default
//! app mode whenever `WindowChrome.recheck-system-theme` fires. Elsewhere nothing reports a change,
//! and the cache keeps its startup reading.

use std::sync::Arc;

use parking_lot::RwLock;

use melodia_app::state::{AppState, Signal};
use melodia_core::themes::SystemColorState;
use melodia_ui::AppWindow;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use slint::ComponentHandle;
#[cfg(target_os = "linux")]
use tokio::sync::watch;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use melodia_app::library;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use melodia_core::themes;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use melodia_platform::services::platform;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::apply_and_seed;

/// Spawn the portal watcher and the UI-thread consumer that applies each reading it sends.
#[cfg(target_os = "linux")]
pub(super) fn watch_os_state(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    initial: SystemColorState,
    kick: Signal,
) {
    let (tx, mut rx) = watch::channel(initial);
    platform::system_theme::spawn_color_watcher(tx);

    let weak = ui.as_weak();
    let s = state.clone();
    if let Err(e) = slint::spawn_local(async_compat::Compat::new(async move {
        while rx.changed().await.is_ok() {
            let reading = rx.borrow_and_update().clone();
            let Some(ui) = weak.upgrade() else { return };
            apply_reading(&ui, &s, &os_state, &reading, &kick);
        }
    })) {
        log::warn!("system theme subscriber spawn_local: {e}");
    }
}

/// Answer `WindowChrome.recheck-system-theme` with a fresh reading of the default app mode. The
/// recheck fires on every focus gain, so a reading matching the cache is dropped before it costs a
/// settings read and a Material You wake.
#[cfg(target_os = "windows")]
pub(super) fn watch_os_state(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    _initial: SystemColorState,
    kick: Signal,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<melodia_ui::WindowChrome>().on_recheck_system_theme(move || {
        let theme = platform::app_mode::system_theme();
        if os_state.read().theme == theme {
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        let reading = SystemColorState { theme: theme.to_owned(), material_you: None };
        apply_reading(&ui, &s, &os_state, &reading, &kick);
    });
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(super) fn watch_os_state(
    _ui: &AppWindow,
    _state: &AppState,
    _os_state: Arc<RwLock<SystemColorState>>,
    _initial: SystemColorState,
    _kick: Signal,
) {
}

/// Fold an OS reading into the shared cache, wake the Material You coordinator, and repaint when
/// the persisted variant is the one following the OS.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn apply_reading(
    ui: &AppWindow,
    state: &AppState,
    os_state: &RwLock<SystemColorState>,
    reading: &SystemColorState,
    kick: &Signal,
) {
    // Only the OS-owned fields move. A generated Material You palette stays painted until the
    // coordinator, woken below, regenerates it for the new brightness.
    {
        let mut cache = os_state.write();
        cache.theme.clone_from(&reading.theme);
        #[cfg(target_os = "linux")]
        cache.kde_palette.clone_from(&reading.kde_palette);
    }
    kick.bump();

    // Read from disk rather than shadowed: this runs on a desktop event, not per frame.
    let settings = match library::settings::get_settings(state) {
        Ok(settings) => settings,
        Err(e) => {
            log::warn!("system theme repaint: read settings: {e}");
            return;
        }
    };
    if settings.theme_variant != themes::SYSTEM_VARIANT_ID {
        return;
    }

    let snapshot = os_state.read().clone();
    let last_static = settings
        .theme_preferences
        .get(&settings.theme_id)
        .and_then(|p| p.last_static_accent.clone());
    apply_and_seed(
        ui,
        &settings.theme_id,
        &settings.theme_variant,
        &settings.accent_color,
        &settings.dynamic_color_style,
        last_static.as_deref(),
        &snapshot,
    );
}
