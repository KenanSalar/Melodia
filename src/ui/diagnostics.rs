//! Wire the Settings page's Diagnostics card, and the toast that follows a crash.
//!
//! Everything `services::{logging, crash_report, diagnostics}` produces lands in
//! one directory and is otherwise unreachable — a user launching from a
//! `.desktop` entry or the Start Menu has no console and no reason to go looking
//! in `~/.local/share`. This module is the whole of what surfaces it: two buttons
//! and one notice.
//!
//! **The toast's `action_kind` and the branch that routes it are one string.**
//! `"crash-report"` here has to match the `kind ==` test in the
//! `Notifications.action` dispatcher (`globals/updater.slint`); a mismatch still
//! renders the button — `notification-stack.slint` gates only on a non-empty
//! action label — and does nothing when clicked, falling off the end of a
//! dispatcher with no `else`. Pinned by `the_toast_kind_matches_its_dispatcher_branch`.

use std::rc::Rc;

use async_compat::Compat;
use chrono::Local;
use slint::{ComponentHandle, SharedString};

use crate::error::AppError;
use crate::services;
use crate::state::AppState;
use crate::ui::launcher;
use crate::ui::notifications::{NotificationParams, NotificationsUi, TOAST_AUTO_DISMISS_MS};
use crate::{AppWindow, Settings};

/// Routing key shared with the `Notifications.action` dispatcher.
const CRASH_TOAST_KIND: &str = "crash-report";

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let settings = ui.global::<Settings>();

    // ---- Open the folder holding the logs and any crash reports ----
    {
        let runtime = state.runtime.clone();
        let logs_dir = state.paths.logs_dir.clone();
        settings.on_open_log_folder(move || {
            runtime.spawn(launcher::open_target(logs_dir.clone(), "open-log-folder"));
        });
    }

    // ---- Save the hand-over bundle somewhere the reporter can find it ----
    {
        let state = state.clone();
        let weak = ui.as_weak();
        let notifications = notifications.clone();
        settings.on_save_diagnostics_report(move || {
            let state = state.clone();
            let weak = weak.clone();
            let notifications = notifications.clone();
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = {
                    let mut d = rfd::AsyncFileDialog::new()
                        .set_title("Save Diagnostics Report")
                        .set_file_name(services::diagnostics::suggested_file_name(Local::now()));
                    if let Some(ui) = weak.upgrade() {
                        d = d.set_parent(&ui.window().window_handle());
                    }
                    d
                };
                let Some(target) = dialog.save_file().await else {
                    return;
                };
                let path = target.path().to_path_buf();

                let result = match services::diagnostics::build_report(&state).await {
                    // `write_text_atomic_sync` is not async, unlike the playlist
                    // export this otherwise mirrors — hence the extra hop.
                    Ok(text) => {
                        let path = path.clone();
                        tokio::task::spawn_blocking(move || {
                            services::write_text_atomic_sync(&path, &text)
                        })
                        .await
                        .map_err(AppError::io_source)
                        .and_then(|inner| inner)
                    }
                    Err(e) => Err(e),
                };

                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                match result {
                    Ok(()) => notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "success".into(),
                            title: settings.invoke_diagnostics_saved_title(),
                            message: settings.invoke_diagnostics_saved_message(
                                SharedString::from(path.display().to_string()),
                            ),
                            action_label: SharedString::default(),
                            action_kind: SharedString::default(),
                        },
                        TOAST_AUTO_DISMISS_MS,
                    ),
                    Err(e) => {
                        log::warn!("save-diagnostics-report: {e}");
                        notifications.show(NotificationParams {
                            variant: "error".into(),
                            title: settings.invoke_diagnostics_failed_title(),
                            message: settings.invoke_diagnostics_failed_message(),
                            action_label: SharedString::default(),
                            action_kind: SharedString::default(),
                        })
                    }
                };
            }));
        });
    }

    notify_previous_crash(ui, state, notifications);
}

/// Announce a panic from the previous run, once.
///
/// Sticky rather than auto-dismissing: a crash notice the user was looking away
/// for is a crash notice that did nothing. Runs synchronously here because it is
/// one `read_dir` over a directory holding at most a handful of entries, and a
/// marker write only when there is actually something to report.
fn notify_previous_crash(ui: &AppWindow, state: &AppState, notifications: &NotificationsUi) {
    let Some(report) = services::crash_report::take_unseen(&state.paths.logs_dir) else {
        return;
    };
    log::info!("previous run left a crash report: {}", report.display());

    let settings = ui.global::<Settings>();
    notifications.show(NotificationParams {
        variant: "warning".into(),
        title: settings.invoke_crash_report_title(),
        message: settings.invoke_crash_report_message(),
        action_label: settings.invoke_crash_report_action_label(),
        action_kind: CRASH_TOAST_KIND.into(),
    });
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
