//! Wire the `Updater.*` Slint callbacks to backend tasks.
//!
//! Five callbacks:
//!
//! - `Updater.check()` — manual "Check for Updates" button. Same flow
//!   as the daily task but triggered by the user; doesn't push a toast
//!   (the user is already on the Settings page watching the state
//!   transition). On a `not_modified` / `up_to_date` result, the
//!   "You're up to date!" row paints; on `Available` the
//!   "Version X Available" row appears with the Install + Skip buttons.
//!   Backend lives in [`check`].
//! - `Updater.install()` — fires `services::updater::download_and_install`
//!   on the tokio runtime. Progress writes flow back through
//!   `upgrade_in_event_loop`. On success → `Updater.restart-needed=true`
//!   + Installed event onto the channel. On failure → `Updater.error-message`
//!   + Failed event. Backend lives in [`install`].
//! - `Updater.restart()` — hands off to
//!   `ui::window_chrome::request_respawn_and_quit`, which arms the respawn
//!   flag and quits the event loop (or keeps the app up and toasts, if the
//!   binary it would relaunch has gone). `shutdown::respawn_if_requested`
//!   (already last step of `main()`) then relaunches, using the path
//!   `spawn_install` recorded via `ui::window_chrome::set_respawn_exe` on a
//!   successful install — captured before the swap, while the path still
//!   pointed at the live binary.
//! - `Updater.skip()` — persists `available-version` into
//!   `settings.updates.skipped_release`. Clears
//!   `Updater.update-available` locally so the panel repaints to the
//!   "up to date" state.
//! - `Updater.auto-check-changed(bool)` — persists the toggle.
//!
//! The [`paint`] submodule holds the shared UI-thread painters both the
//! check and install backends call back through.

mod check;
mod install;
mod paint;

use std::rc::Rc;

use slint::ComponentHandle;
use tokio::sync::watch;

use crate::library;
use crate::services::updater::UpdaterEvent;
use crate::state::AppState;
use crate::ui::shell::notifications::NotificationsUi;
use crate::{AppWindow, MelodiaUpdater};

use check::spawn_manual_check;
use install::spawn_install;

/// Wire the five `Updater.*` callbacks on the `Updater` global. Must
/// be called after `ui::settings::updater_settings::install` (which seeds the
/// global's initial values).
pub fn wire(
    ui: &AppWindow,
    state: &AppState,
    notifications: &Rc<NotificationsUi>,
    event_tx: &watch::Sender<Option<UpdaterEvent>>,
) {
    let updater = ui.global::<MelodiaUpdater>();
    let weak = ui.as_weak();

    // ---- Updater.check ----
    {
        let state = state.clone();
        let weak = weak.clone();
        let event_tx = event_tx.clone();
        updater.on_check(move || {
            spawn_manual_check(state.clone(), weak.clone(), event_tx.clone());
        });
    }

    // ---- Updater.install ----
    {
        let state = state.clone();
        let weak = weak.clone();
        let event_tx = event_tx.clone();
        updater.on_install(move || {
            spawn_install(state.clone(), weak.clone(), event_tx.clone());
        });
    }

    // ---- Updater.restart ----
    updater.on_restart(|| {
        crate::ui::window_chrome::request_respawn_and_quit();
    });

    // ---- Updater.skip ----
    {
        let state = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        updater.on_skip(move || {
            let version = weak
                .upgrade()
                .map(|ui| ui.global::<MelodiaUpdater>().get_available_version().to_string())
                .unwrap_or_default();
            if version.is_empty() {
                log::warn!("updater: skip clicked but available_version was empty");
                return;
            }
            let state_for_disk = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::updates::set_skipped_release(&state_for_disk, version)
                {
                    log::warn!("updater: set_skipped_release: {e}");
                }
            });
            // Repaint the Settings → Updates panel to the "up to
            // date" state so the user sees an immediate effect, and
            // dismiss any still-visible "Update available" toast so the
            // bottom-right matches the Settings panel.
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<MelodiaUpdater>();
                g.set_update_available(false);
                g.set_up_to_date(true);
            }
            notifications.dismiss_by_kind("install-update");
        });
    }

    // ---- Updater.auto-check-changed ----
    {
        let state = state.clone();
        updater.on_auto_check_changed(move |on| {
            let state_for_disk = state.clone();
            state.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::updates::set_auto_check_enabled(&state_for_disk, on)
                {
                    log::warn!("updater: set_auto_check_enabled: {e}");
                }
            });
        });
    }
}

/// The cached `If-None-Match` `ETag` from the last successful manifest fetch,
/// or `None` when no usable tag is on disk.
fn read_etag(state: &AppState) -> Option<String> {
    crate::services::settings::read_settings(&state.paths)
        .ok()
        .map(|s| s.updates.last_manifest_etag)
        .filter(|tag| !tag.is_empty())
}

/// The release version the user explicitly skipped, or an empty string.
fn current_skipped_release(state: &AppState) -> String {
    crate::services::settings::read_settings(&state.paths)
        .ok()
        .map(|s| s.updates.skipped_release)
        .unwrap_or_default()
}
