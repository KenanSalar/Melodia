//! The four Settings cards facing the window, and the palette they paint: resolved by
//! [`theme_apply`], written by [`theme_paint`], and reached only through [`apply_palette`].

mod accent_picker;
mod install;
mod material_you_sync;
mod repaint;
pub mod rgb;
mod system_watcher;
pub mod theme_apply;
pub mod theme_paint;
mod theme_picker;
pub mod window_border;
mod window_settings;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tokio::sync::watch;

use melodia_app::library;
use melodia_app::state::{AppState, Signal};
use melodia_core::error::describe;
use melodia_core::themes::{self, SystemColorState, ThemeDef};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use melodia_platform::services::platform;
use melodia_ui::{AppWindow, Settings};

pub use install::install;
pub use repaint::{apply_and_seed, repaint_from_settings};

/// What boot hands the Material You coordinator (`tasks::material_you`), so this module never
/// reaches the player's view-model channel.
pub struct AppearanceHandles {
    /// The OS state, whose `material_you` the coordinator writes before sending the snapshot
    /// through [`Self::repaint_tx`].
    pub os_state: Arc<parking_lot::RwLock<SystemColorState>>,
    /// Bumped by every appearance pick, so the coordinator re-evaluates against the settings just
    /// written.
    pub kick: Signal,
    /// Where the coordinator publishes each generated palette's state, for the UI-thread
    /// subscriber [`fn@install`] spawns to repaint from.
    pub repaint_tx: watch::Sender<SystemColorState>,
}

/// The synchronous shadow of `settings.accent_color`, written by every appearance callback before
/// it spawns the write and read by its siblings in place of the file.
pub(super) type PersistedAccent = Arc<parking_lot::Mutex<String>>;

/// Read the OS appearance state once at startup, ahead of the first frame, so a System variant
/// never paints the wrong brightness first. Linux: XDG portal + `kdeglobals`. Windows: the default
/// app mode. Elsewhere: an `unknown()` placeholder, which a System variant paints as dark.
pub(super) fn read_initial_system_state() -> SystemColorState {
    #[cfg(target_os = "linux")]
    {
        SystemColorState {
            theme: platform::system_theme::get_system_theme_blocking(),
            kde_palette: platform::system_theme::get_kde_colors(),
            material_you: None,
        }
    }
    #[cfg(target_os = "windows")]
    {
        SystemColorState { theme: platform::color_mode::app_theme().to_owned(), material_you: None }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        SystemColorState::unknown()
    }
}

pub(super) fn seed_theme_registry(ui: &AppWindow) {
    let names: Vec<SharedString> =
        themes::registry().iter().map(|t| SharedString::from(t.name)).collect();
    let slot = |theme: &ThemeDef| apply_and_seed_to_i32(themes::theme_index(theme.id));
    let g = ui.global::<Settings>();
    g.set_theme_names(ModelRc::from(Rc::new(VecModel::from(names))));
    g.set_catppuccin_idx(slot(&themes::catppuccin::CATPPUCCIN));
    g.set_material3_idx(slot(&themes::material3::MATERIAL3));
}

/// The theme's recorded `last_static_accent`, `None` on a failed read or before one is recorded;
/// callers fall back to the theme's `default_accent`.
pub(super) fn read_last_static_accent(state: &AppState, theme_id: &str) -> Option<String> {
    library::settings::get_settings(&state.paths)
        .ok()
        .and_then(|s| s.last_static_accent(theme_id).map(str::to_owned))
}

/// Writes the palette, then re-solves the two artwork-derived tiers and the window border against
/// it.
///
/// **The only place `theme_apply::apply` may be called from.** Both tiers are snapshots taken when
/// a hero or a track landed, so a palette change reaches neither on its own, and the border's
/// swatches and System colour come off the palette too.
pub(super) fn apply_palette(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    system: &SystemColorState,
) {
    theme_apply::apply(ui, theme_id, variant_id, accent_id, system);
    crate::ui::hero_backdrop::republish_for_palette(ui);
    crate::ui::now_playing::republish_for_palette(ui);
    window_border::republish_for_palette(ui, theme_id, variant_id, system);
}

/// Persist the user's pick without waking the Material You coordinator; [`persist_and_kick`] is
/// the one that does.
pub(super) fn persist(state: &AppState, theme_id: &str, variant_id: &str, accent_id: &str) {
    let theme_id = theme_id.to_owned();
    let variant_id = variant_id.to_owned();
    let accent_id = accent_id.to_owned();
    state.persist_blocking("persist appearance", move |paths| {
        library::settings::set_appearance(paths, theme_id, variant_id, accent_id)
    });
}

/// Persists and then kicks the Material You coordinator, in one blocking task: the coordinator
/// reads `settings.json` on waking, and a kick ahead of the write would regenerate against the
/// previous pick.
pub(super) fn persist_and_kick(
    state: &AppState,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    kick: &Signal,
) {
    let paths = Arc::clone(&state.paths);
    let theme_id = theme_id.to_owned();
    let variant_id = variant_id.to_owned();
    let accent_id = accent_id.to_owned();
    let kick = kick.clone();
    state.runtime.spawn_blocking(move || {
        match library::settings::set_appearance(&paths, theme_id, variant_id, accent_id) {
            Ok(()) => kick.bump(),
            Err(e) => log::warn!(
                "persist appearance: {}; suppressing Material You kick (disk write failed, \
                 coordinator would observe stale settings)",
                describe(&e)
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

/// The variant a chip index stands for: the theme's own, then the synthetic System chip where the
/// theme offers one. `None` past the end.
pub(super) fn variant_id_at(theme: &ThemeDef, idx: usize) -> Option<&'static str> {
    if theme.supports_system_mode && idx == theme.variants.len() {
        return Some(themes::SYSTEM_VARIANT_ID);
    }
    theme.variants.get(idx).map(|v| v.id)
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

#[cfg(test)]
#[path = "tests/appearance_tests.rs"]
mod tests;
