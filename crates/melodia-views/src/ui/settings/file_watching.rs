//! The File Watching toggle. `library::settings::set_folder_watching_enabled` persists before it
//! starts or stops the watcher, so a failed start leaves the file saying what the user asked for
//! and `tasks::resume_watching::run` retries it next launch.
//!
//! On by default (`LibraryFlags::default()`), a fresh install with it off having stranded new
//! users on a library that never moved. Turning it off raises a warning, and turning it on clears
//! that.

use std::rc::Rc;

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::services::settings;
use melodia_app::state::AppState;
use melodia_core::error::describe;

use crate::ui::shell::notifications::{NotificationsUi, RowText};
use melodia_ui::{AppWindow, NotificationKind, NotificationVariant, Settings};

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    // A failed read leaves the Slint default, as on a first launch.
    if let Ok(s) = settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_watch_for_file_changes(s.library.folder_watching_enabled);
    }

    let state_clone = state.clone();
    let notifications = notifications.clone();
    let ui_weak = ui.as_weak();
    ui.global::<Settings>().on_watch_for_file_changes_changed(move |on| {
        // A start queries the enabled folders before taking the watcher's lock: not UI-thread work.
        let s_for_task = state_clone.clone();
        state_clone.runtime.spawn(async move {
            if let Err(e) = library::settings::set_folder_watching_enabled(&s_for_task, on).await {
                log::warn!("set_folder_watching_enabled: {}", describe(&e));
            }
        });

        if on {
            notifications.dismiss_by_kind(NotificationKind::WatcherDisabled);
        } else {
            // Strings come from the `Settings.watcher-disabled-*` pure callbacks, and the row
            // is sticky on the page carrying the language picker, so the recipe is kept.
            let Some(ui) = ui_weak.upgrade() else { return };
            notifications.show_localized(
                &ui,
                NotificationVariant::Warning,
                NotificationKind::WatcherDisabled,
                |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_watcher_disabled_title(),
                        g.invoke_watcher_disabled_message(),
                    )
                },
            );
        }
    });
}
