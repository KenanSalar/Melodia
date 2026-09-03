//! XDG portal subscription + UI-thread consumer that repaints when the
//! persisted variant is `"system"`. On non-Linux platforms the OS doesn't
//! surface live appearance changes through this code path, so the watcher
//! is a no-op and the cache stays at its `unknown()` initial value.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::AppWindow;
use crate::state::{AppState, Signal};
use crate::themes::SystemColorState;

#[cfg(target_os = "linux")]
use slint::ComponentHandle;
#[cfg(target_os = "linux")]
use tokio::sync::watch;

#[cfg(target_os = "linux")]
use crate::library;
#[cfg(target_os = "linux")]
use crate::services;
#[cfg(target_os = "linux")]
use crate::themes;

#[cfg(target_os = "linux")]
use super::apply_and_seed;

/// On Linux: spawn the portal watcher and the UI-thread consumer that
/// repaints when the persisted variant is `"system"`. On other platforms
/// the OS doesn't surface live appearance changes through this code path,
/// so the cache stays at its `unknown()` initial value.
#[cfg(target_os = "linux")]
pub(super) fn spawn_os_state_watcher(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    initial: SystemColorState,
    kick: Signal,
) {
    let (tx, mut rx) = watch::channel(initial);
    services::platform::system_theme::spawn_color_watcher(tx);

    let weak = ui.as_weak();
    let s = state.clone();
    if let Err(e) = slint::spawn_local(async_compat::Compat::new(async move {
        while rx.changed().await.is_ok() {
            let new_state = rx.borrow_and_update().clone();
            // Preserve the dynamic Material You palette across OS theme
            // flips — only the OS-owned fields move. The coordinator
            // will overwrite `material_you` after it regenerates for the
            // new `is_dark`, but until then the previously generated
            // palette stays painted.
            {
                let mut guard = os_state.write();
                guard.theme.clone_from(&new_state.theme);
                guard.kde_palette.clone_from(&new_state.kde_palette);
            }

            // Kick the Material You coordinator so it re-evaluates
            // `is_dark` (matters when the user's variant is "system" and
            // the OS just flipped dark/light) and regenerates the
            // palette.
            kick.bump();

            let Some(ui) = weak.upgrade() else { return };
            // Only repaint when the *persisted* variant is "system" —
            // static variants don't care about OS changes. Re-reading
            // `settings.json` here is fine: it's a few KB and only fires
            // on a desktop event (user toggling Plasma's colour scheme).
            let settings = match library::settings::get_settings(&s) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("system theme repaint: read settings: {e}");
                    continue;
                }
            };
            if settings.theme_variant == themes::SYSTEM_VARIANT_ID {
                let snapshot = os_state.read().clone();
                let last_static = settings
                    .theme_preferences
                    .get(&settings.theme_id)
                    .and_then(|p| p.last_static_accent.clone());
                apply_and_seed(
                    &ui,
                    &settings.theme_id,
                    &settings.theme_variant,
                    &settings.accent_color,
                    &settings.dynamic_color_style,
                    last_static.as_deref(),
                    &snapshot,
                );
            }
        }
    })) {
        log::warn!("system theme subscriber spawn_local: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn spawn_os_state_watcher(
    _ui: &AppWindow,
    _state: &AppState,
    _os_state: Arc<RwLock<SystemColorState>>,
    _initial: SystemColorState,
    _kick: Signal,
) {
}
