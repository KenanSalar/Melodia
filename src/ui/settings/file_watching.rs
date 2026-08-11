//! Wire the Settings page's File Watching toggle to Rust.
//!
//! Seeds `Settings.watch-for-file-changes` from `settings.json` at
//! startup so the toggle reflects the persisted choice across restarts,
//! and registers the change callback so user toggles flow into
//! `library::settings::set_folder_watching_enabled` — which **persists
//! first** (`library.folder_watching_enabled`) then flips the watcher's
//! start/stop state. Persist-first means a `start()` failure leaves disk
//! consistent with the user's intent; `tasks::first_launch::run` retries
//! on the next launch from the persisted flag.
//!
//! The default (`true`) is enforced by `LibraryFlags::default()`
//! (`src/services/settings/`). First-launch installs read no
//! `settings.json`, fall through to defaults, and the toggle paints ON —
//! consumer music players (Apple Music, Groove, Windows Media Player,
//! Spotify) all auto-watch with no toggle, and a fresh install with
//! watching off stranded new users in a stale-UI failure mode.
//!
//! On the OFF transition we surface a transient notification via
//! [`crate::ui::shell::notifications`] so the user knows the consequence ("your library
//! won't auto-sync") instead of silently moving on. Re-enabling clears the
//! lingering row by kind. The notification system is reusable — the
//! future auto-updater pushes its three states through the same
//! `NotificationsUi::show` path with action buttons attached.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::library;
use crate::services::settings;
use crate::state::AppState;
use slint::SharedString;

use crate::ui::shell::notifications::{NotificationParams, NotificationsUi};
use crate::{AppWindow, Settings};

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    // Seed the toggle from disk. Missing / unreadable file leaves the
    // Slint default (`true`) in place — matching the first-launch path.
    if let Ok(s) = settings::read_settings(&state.paths) {
        ui.global::<Settings>()
            .set_watch_for_file_changes(s.library.folder_watching_enabled);
    }

    let state_clone = state.clone();
    let notifications = notifications.clone();
    let ui_weak = ui.as_weak();
    ui.global::<Settings>()
        .on_watch_for_file_changes_changed(move |on| {
            // `set_folder_watching_enabled` is async — the watcher's
            // start needs a DB query to fetch enabled folder paths
            // *before* the parking_lot watcher lock. Spawn onto the
            // tokio runtime so the UI thread isn't blocked.
            let s_for_task = state_clone.clone();
            state_clone.runtime.spawn(async move {
                if let Err(e) =
                    library::settings::set_folder_watching_enabled(&s_for_task, on).await
                {
                    log::warn!("set_folder_watching_enabled: {e}");
                }
            });

            if on {
                // Re-enabled — clear any lingering "watcher disabled" row
                // so the user isn't told something stale.
                notifications.dismiss_by_kind("watcher-disabled");
            } else {
                // Disabled — surface a warning so the user understands the
                // library won't auto-sync. Strings come from the
                // Settings.watcher-disabled-* pure callbacks so they're
                // resolved through Slint's bundled-translation pipeline.
                let Some(ui) = ui_weak.upgrade() else { return };
                let g = ui.global::<Settings>();
                let title = g.invoke_watcher_disabled_title();
                let message = g.invoke_watcher_disabled_message();
                notifications.show(NotificationParams {
                    variant: "warning".into(),
                    title,
                    message,
                    action_label: SharedString::default(),
                    action_kind: "watcher-disabled".into(),
                });
            }
        });
}
