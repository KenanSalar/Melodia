//! Wire the Settings page's About section.
//!
//! Registers `Settings.open-repository`, whose URL is `CARGO_PKG_REPOSITORY` so the link
//! tracks the canonical repo with no hardcoded string. The launch goes through
//! [`crate::ui::launcher::open_target`], which owns the hop off the UI thread.
//!
//! The version shown in the About card is *not* set here — it rides on
//! `MelodiaUpdater.current-version`, seeded by [`super::updater_settings`].

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::launcher;
use crate::{AppWindow, Settings};

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
