//! Wire the Settings page's About section.
//!
//! Registers `Settings.open-repository` to open the project's source
//! repository in the system browser. The URL is `CARGO_PKG_REPOSITORY`
//! (Cargo populates it from the `repository` field in `Cargo.toml`), so
//! the link tracks the canonical repo with no hardcoded magic string.
//!
//! `open::that` spawns and waits on a child process (`xdg-open` / `open`
//! / `explorer`), so the launch runs on the blocking pool off the UI
//! thread — the same shape `library::tracks::reveal_in_file_manager`
//! uses for the "Open Containing Folder" action.
//!
//! The version shown in the About card is *not* set here — it rides on
//! `MelodiaUpdater.current-version`, seeded by [`crate::ui::updater_settings`].

use slint::ComponentHandle;

use crate::state::AppState;
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
        // Off the UI thread: `open::that` blocks until the child launcher
        // is spawned. Reuse the tokio runtime's blocking pool rather than
        // stalling the event loop.
        runtime.spawn(async {
            match tokio::task::spawn_blocking(|| open::that(REPOSITORY_URL)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("open-repository: open::that failed: {e}"),
                Err(e) => log::warn!("open-repository: launch task join failed: {e}"),
            }
        });
    });
}
