//! Wiring for the Appearance section's Theme / Variant / Accent rows.
//!
//! Reads the persisted `theme_id` / `theme_variant` / `accent_color` from
//! `services::settings`, populates the Slint `Settings` global, and applies
//! the resolved palette via `themes::apply()`. Three callbacks
//! (`theme-changed` / `variant-changed` / `accent-changed`) update the
//! global, repaint the Theme tokens, and persist the new selection on the
//! tokio runtime so the JSON write doesn't block the UI thread.

mod accent_picker;
mod install;
mod material_you_sync;
mod repaint;
mod system_watcher;
mod theme_picker;
mod window_settings;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tokio::sync::watch;

use crate::library;
#[cfg(target_os = "linux")]
use crate::services;
use crate::state::AppState;
use crate::themes::{self, SystemColorState, ThemeDef};
use crate::{AppWindow, Settings};

pub use install::install;
pub use repaint::{apply_and_seed, repaint_from_settings};

/// Handles returned by [`fn@install`] so `main.rs` can wire the Material You
/// coordinator (`tasks::material_you`) without `appearance` having to
/// reach across into the player view-model channel itself.
pub struct AppearanceHandles {
    /// Shared OS-state cache. The coordinator writes the dynamic Material
    /// You palette into `material_you` and triggers a repaint by sending
    /// a fresh snapshot through [`Self::repaint_tx`].
    pub os_state: Arc<parking_lot::RwLock<SystemColorState>>,
    /// Counter-style watch channel — every appearance callback (theme /
    /// variant / accent / colour-style click) increments the counter so
    /// the coordinator wakes and re-evaluates against the latest settings
    /// snapshot.
    pub kick_tx: watch::Sender<u64>,
    /// Snapshot-style repaint channel — the Material You coordinator
    /// publishes the fresh [`SystemColorState`] after each palette
    /// generation; a UI-thread subscriber spawned inside [`fn@install`]
    /// consumes the latest value and calls [`repaint_from_settings`].
    pub repaint_tx: watch::Sender<SystemColorState>,
}

/// Synchronous in-memory shadow of `settings.accent_color`. Updated by
/// every appearance callback **before** it spawns the async disk write,
/// and read by sibling callbacks that need to know the persisted accent
/// without going through `library::settings::get_settings`.
pub(super) type PersistedAccent = Arc<parking_lot::Mutex<String>>;

/// Read the OS appearance state once at startup. Linux: XDG portal +
/// `kdeglobals`. Other platforms: an `unknown()` placeholder so `apply()`'s
/// system-variant branch defaults to dark.
pub(super) fn read_initial_system_state() -> SystemColorState {
    #[cfg(target_os = "linux")]
    {
        SystemColorState {
            theme: services::system_theme::get_system_theme_blocking(),
            kde_palette: services::system_theme::get_kde_colors(),
            material_you: None,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        SystemColorState::unknown()
    }
}

pub(super) fn seed_theme_names(ui: &AppWindow) {
    let names: Vec<SharedString> =
        themes::registry().iter().map(|t| SharedString::from(t.name)).collect();
    ui.global::<Settings>().set_theme_names(ModelRc::from(Rc::new(VecModel::from(names))));
}

/// Look up `theme_preferences[theme_id].last_static_accent` in the
/// current settings.json. Returns `None` on read failure or when no
/// static accent has been recorded yet — callers fall back to the
/// theme's `default_accent`.
pub(super) fn read_last_static_accent(state: &AppState, theme_id: &str) -> Option<String> {
    library::settings::get_settings(state)
        .ok()
        .and_then(|s| s.theme_preferences.get(theme_id).and_then(|p| p.last_static_accent.clone()))
}

/// Persist the user's pick on tokio's blocking pool — `set_appearance`
/// is sync `std::fs` I/O and must not block the Slint event loop. Any
/// write failure (disk full, permissions) surfaces as a `log::warn!`
/// instead of being silently dropped.
pub(super) fn persist(state: &AppState, theme_id: &str, variant_id: &str, accent_id: &str) {
    let s = state.clone();
    let theme_id = theme_id.to_owned();
    let variant_id = variant_id.to_owned();
    let accent_id = accent_id.to_owned();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_appearance(&s, theme_id, variant_id, accent_id) {
            log::warn!("persist appearance: {e}");
        }
    });
}

/// Persist appearance + kick the Material You coordinator afterwards.
/// The coordinator reads `settings.json` on wakeup; firing the kick
/// before the disk write commits was racy — the coordinator could
/// observe the *previous* theme / variant and "regenerate" against
/// stale state. With the kick inside the same `spawn_blocking` task as
/// the write, the coordinator always reads fresh settings on wake.
pub(super) fn persist_and_kick(
    state: &AppState,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    kick_tx: &watch::Sender<u64>,
) {
    let s = state.clone();
    let theme_id = theme_id.to_owned();
    let variant_id = variant_id.to_owned();
    let accent_id = accent_id.to_owned();
    let kick = kick_tx.clone();
    state.runtime.spawn_blocking(move || {
        match library::settings::set_appearance(&s, theme_id, variant_id, accent_id) {
            Ok(()) => kick.send_modify(|n| *n = n.wrapping_add(1)),
            Err(e) => log::warn!(
                "persist appearance: {e}; suppressing Material You kick (disk write failed, \
                 coordinator would observe stale settings)"
            ),
        }
    });
}

pub(super) fn registry_get(idx: i32) -> Option<&'static ThemeDef> {
    themes::registry().get(usize_from(idx)).copied()
}

pub(super) fn resolved_variant_id(theme: &ThemeDef, idx: usize) -> &'static str {
    theme.variants.get(idx).map_or(theme.default_variant, |v| v.id)
}

#[allow(
    clippy::cast_sign_loss,
    reason = "Slint chip indices are i32 ≥ 0; negative slips fall through to default ids via .get(idx).map_or"
)]
pub(super) fn usize_from(idx: i32) -> usize {
    if idx < 0 { usize::MAX } else { idx as usize }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "Theme/variant/accent counts are tiny (≤ 14); never overflows i32"
)]
pub(super) fn apply_and_seed_to_i32(idx: usize) -> i32 {
    idx as i32
}
