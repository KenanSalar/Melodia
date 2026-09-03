//! Wire the Settings page's File Watching toggle to Rust.
//!
//! Seeds `Settings.watch-for-file-changes` from `settings.json` and registers the change
//! callback, which routes into `library::settings::set_folder_watching_enabled` — that one
//! **persists first**, then flips the watcher's start/stop state, so a `start()` failure
//! leaves disk consistent with the user's intent and `tasks::first_launch::run` retries
//! from the persisted flag next launch.
//!
//! The default is ON, enforced by `LibraryFlags::default()`: every consumer player
//! auto-watches with no toggle at all, and a fresh install with watching off stranded new
//! users in a stale-UI failure mode.
//!
//! The OFF transition surfaces a notification so the user knows the consequence rather
//! than silently moving on; re-enabling clears the lingering row by kind.

use std::rc::Rc;

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::services::settings;
use melodia_app::state::AppState;

use crate::ui::shell::notifications::{NotificationsUi, RowText};
use crate::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    // A missing or unreadable file leaves the Slint default in place, matching the
    // first-launch path.
    if let Ok(s) = settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_watch_for_file_changes(s.library.folder_watching_enabled);
    }

    let state_clone = state.clone();
    let notifications = notifications.clone();
    let ui_weak = ui.as_weak();
    ui.global::<Settings>().on_watch_for_file_changes_changed(move |on| {
        // Async: the watcher's start needs a DB query for the enabled folder paths
        // *before* the parking_lot watcher lock, so it can't run on the UI thread.
        let s_for_task = state_clone.clone();
        state_clone.runtime.spawn(async move {
            if let Err(e) = library::settings::set_folder_watching_enabled(&s_for_task, on).await {
                log::warn!("set_folder_watching_enabled: {e}");
            }
        });

        if on {
            notifications.dismiss_by_kind("watcher-disabled");
        } else {
            // Strings come from the `Settings.watcher-disabled-*` pure callbacks, and the row
            // is sticky on the page carrying the language picker, so the recipe is kept.
            let Some(ui) = ui_weak.upgrade() else { return };
            notifications.show_localized(&ui, "warning", "watcher-disabled", |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_watcher_disabled_title(),
                    g.invoke_watcher_disabled_message(),
                )
            });
        }
    });
}
