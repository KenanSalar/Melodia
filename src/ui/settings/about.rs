//! Wire the Settings page's About section.
//!
//! Registers `Settings.open-repository` to open the project's source
//! repository in the system browser. The URL is `CARGO_PKG_REPOSITORY`
//! (Cargo populates it from the `repository` field in `Cargo.toml`), so
//! the link tracks the canonical repo with no hardcoded magic string.
//!
//! The launch itself goes through [`crate::ui::launcher::open_target`], which
//! owns the hop off the UI thread — `open::that_detached` still forks and execs
//! a child launcher, which is not something to do on the event loop.
//!
//! The version shown in the About card is *not* set here — it rides on
//! `MelodiaUpdater.current-version`, seeded by [`super::updater_settings`].

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::launcher;
use crate::{AppWindow, Settings};

/// Source-repository URL, taken from Cargo's `repository` manifest field
/// at compile time. Empty only if the field is ever removed from
/// `Cargo.toml`; the callback guards against handing a blank URL to `open`.
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
