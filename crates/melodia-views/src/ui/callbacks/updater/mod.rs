//! The `Updater.*` callbacks: `check` and `install` run the backends in [`check`] and
//! [`install`], `restart` hands off to `window_chrome::request_respawn_and_quit`, `skip`
//! persists the offered version as skipped, and `auto-check-changed` persists the toggle.
//!
//! On Windows a successful install rarely reports at all, Windows Installer closing the app and
//! reopening the new build. [`paint`] holds the UI-thread painters both backends call back
//! through, the daily task reaching them via [`check_painter`].

mod check;
mod install;
mod paint;

use std::rc::Rc;

use slint::ComponentHandle;
use tokio::sync::watch;

use crate::ui::shell::notifications::NotificationsUi;
use melodia_app::library;
use melodia_app::services::updater::UpdaterEvent;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, MelodiaUpdater, NotificationKind};

use check::spawn_manual_check;
use install::spawn_install;
pub use paint::check_painter;

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

    {
        let state = state.clone();
        let weak = weak.clone();
        let event_tx = event_tx.clone();
        updater.on_check(move || {
            spawn_manual_check(state.clone(), weak.clone(), event_tx.clone());
        });
    }

    {
        let state = state.clone();
        let weak = weak.clone();
        let event_tx = event_tx.clone();
        updater.on_install(move || {
            spawn_install(state.clone(), weak.clone(), event_tx.clone());
        });
    }

    updater.on_restart(|| {
        crate::ui::window_chrome::request_respawn_and_quit();
    });

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
                    library::settings::updates::set_skipped_release(&state_for_disk.paths, version)
                {
                    log::warn!("updater: set_skipped_release: {}", describe(&e));
                }
            });
            // The panel repaints to "up to date" at once, and a still-visible Available toast
            // goes with it.
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<MelodiaUpdater>();
                g.set_update_available(false);
                g.set_up_to_date(true);
            }
            notifications.dismiss_by_kind(NotificationKind::InstallUpdate);
        });
    }

    {
        let state = state.clone();
        updater.on_auto_check_changed(move |on| {
            let state_for_disk = state.clone();
            state.runtime.spawn_blocking(move || {
                // Kicked inside the write and only on `Ok(())`: the loop re-reads the file on
                // waking, so a kick over a failed write would wake it onto the old answer.
                match library::settings::updates::set_auto_check_enabled(&state_for_disk.paths, on)
                {
                    Ok(()) => state_for_disk.auto_check_changed.bump(),
                    Err(e) => log::warn!("updater: set_auto_check_enabled: {}", describe(&e)),
                }
            });
        });
    }
}
