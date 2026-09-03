//! The Motion card: one switch, persist-only.

use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppWindow, Settings};
use crate::{library, services};

/// Seed and wire the Skip Startup Animation switch.
///
/// Nothing is applied here — `boot::ui_setup::hydrate_ui_from_settings` reads the same
/// field and raises `Nav.suppress-enter-animation` from it before the window is shown,
/// which is the only mount the setting speaks for. Seeded with `if let Ok` so an
/// unreadable file leaves the Slint-declared `false`, the same answer the hydrate reaches.
pub fn install(ui: &AppWindow, state: &AppState) {
    if let Ok(settings) = services::settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_skip_startup_animation(settings.motion.skip_startup_animation);
    }

    let state = state.clone();
    ui.global::<Settings>().on_skip_startup_animation_changed(move |on| {
        state.persist_blocking("persist skip_startup_animation", move |s| {
            library::settings::set_skip_startup_animation(s, on)
        });
    });
}
