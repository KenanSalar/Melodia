//! The About card's repository link, `CARGO_PKG_REPOSITORY` so it follows the canonical repo. The
//! version it shows is `MelodiaUpdater.current-version`, which [`super::updater_settings`] seeds.

use slint::ComponentHandle;

use crate::ui::launcher;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Settings};

/// Empty only if the `repository` field is removed from `Cargo.toml`; the callback guards
/// against handing a blank URL to `open`.
const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");

pub fn install(ui: &AppWindow, state: &AppState) {
    let runtime = state.runtime.clone();
    ui.global::<Settings>().on_open_repository(move || {
        if REPOSITORY_URL.is_empty() {
            log::warn!("open-repository: CARGO_PKG_REPOSITORY is empty");
            return;
        }
        runtime.spawn(launcher::open_target(REPOSITORY_URL, "open-repository"));
    });
}
