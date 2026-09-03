//! Wire the Settings page's Diagnostics card, and the toast that follows a crash.
//!
//! Everything `services::{logging, crash_report, diagnostics}` produces lands in
//! one directory and is otherwise unreachable — a user launching from a
//! `.desktop` entry or the Start Menu has no console and no reason to go looking
//! in `~/.local/share`. This module is the whole of what surfaces it: two
//! buttons, a switch and one notice.
//!
//! **The toast's `action_kind` and the branch that routes it are one string.**
//! `"crash-report"` here has to match the `kind ==` test in the
//! `Notifications.action` dispatcher (`globals/updater.slint`); a mismatch still
//! renders the button — `notification-stack.slint` gates only on a non-empty
//! action label — and does nothing when clicked, falling off the end of a
//! dispatcher with no `else`. Pinned by `the_toast_kind_matches_its_dispatcher_branch`.

use std::path::PathBuf;
use std::rc::Rc;

use async_compat::Compat;
use chrono::Local;
use slint::{ComponentHandle, SharedString, Weak};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::ui::shell::notifications::{
    NotificationParams, NotificationsUi, RowText, TOAST_AUTO_DISMISS_MS,
};
use crate::ui::{file_dialog, launcher};
use crate::{AppWindow, Settings};
use crate::{library, services};

/// Routing key shared with the `Notifications.action` dispatcher.
const CRASH_TOAST_KIND: &str = "crash-report";

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    wire_open_log_folder(ui, state);
    wire_save_report(ui, state, notifications);
    wire_verbose_logging(ui, state);
    notify_previous_crash(ui, state, notifications);
}

/// Hand the logs directory to the desktop's file manager. The same callback the
/// crash toast's action button reaches, hence the two buttons and one handler.
fn wire_open_log_folder(ui: &AppWindow, state: &AppState) {
    let runtime = state.runtime.clone();
    let logs_dir = state.paths.logs_dir.clone();
    ui.global::<Settings>().on_open_log_folder(move || {
        runtime.spawn(launcher::open_target(logs_dir.clone(), "open-log-folder"));
    });
}

/// On the UI thread throughout: the picker has to be, and `Rc<NotificationsUi>`
/// is `!Send`. `Compat` supplies the tokio reactor the awaited halves need.
fn wire_save_report(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let state = state.clone();
    let weak = ui.as_weak();
    let notifications = notifications.clone();
    ui.global::<Settings>().on_save_diagnostics_report(move || {
        let state = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        let _ = slint::spawn_local(Compat::new(save_report(state, weak, notifications)));
    });
}

/// Ask where the bundle goes, build it, write it, and say which way it went.
/// A cancelled picker is the ordinary case and says nothing.
async fn save_report(state: AppState, weak: Weak<AppWindow>, notifications: Rc<NotificationsUi>) {
    let dialog = file_dialog::parented(&weak, "Save Diagnostics Report")
        .set_file_name(services::diagnostics::suggested_file_name(Local::now()));
    let Some(target) = dialog.save_file().await else {
        return;
    };
    let path = target.path().to_path_buf();

    let result = match services::diagnostics::build_report(&state).await {
        Ok(text) => write_report(path.clone(), text).await,
        Err(e) => Err(e),
    };

    let Some(ui) = weak.upgrade() else { return };
    let settings = ui.global::<Settings>();
    match result {
        Ok(()) => {
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    "success",
                    settings.invoke_diagnostics_saved_title(),
                    settings.invoke_diagnostics_saved_message(SharedString::from(
                        path.display().to_string(),
                    )),
                ),
                TOAST_AUTO_DISMISS_MS,
            );
        }
        Err(e) => {
            log::warn!("save-diagnostics-report: {e}");
            notifications.show_localized(&ui, "error", "", |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_diagnostics_failed_title(),
                    g.invoke_diagnostics_failed_message(),
                )
            });
        }
    }
}

/// `atomic_file::write_text_sync` is not async, unlike the playlist export this
/// otherwise mirrors — hence the hop off the UI thread for one file write.
async fn write_report(path: PathBuf, text: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || crate::utils::atomic_file::write_text_sync(&path, &text))
        .await
        .map_err(AppError::io_source)?
}

/// Apply the Verbose Logging switch live, then persist it.
///
/// Seeded with `if let Ok` rather than [`crate::ui::settings_bind::read_or_default`]
/// so an unreadable file leaves the Slint-declared `false` — the same answer
/// `logging::install` reached from the same read, so the two can't disagree.
fn wire_verbose_logging(ui: &AppWindow, state: &AppState) {
    if let Ok(settings) = services::settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_verbose_logging(settings.diagnostics.verbose_logging);
    }

    let state = state.clone();
    ui.global::<Settings>().on_verbose_logging_changed(move |on| {
        // Live first: a failed disk write must not undo what the user can
        // already see working.
        services::platform::logging::set_verbose(on);
        state.persist_blocking("persist verbose_logging", move |s| {
            library::settings::set_verbose_logging(s, on)
        });
    });
}

/// Announce a panic from the previous run, once.
///
/// Sticky rather than auto-dismissing: a crash notice the user was looking away
/// for is a crash notice that did nothing. Runs synchronously here because it is
/// one `read_dir` over a directory holding at most a handful of entries, and a
/// marker write only when there is actually something to report.
fn notify_previous_crash(ui: &AppWindow, state: &AppState, notifications: &NotificationsUi) {
    let Some(report) = services::platform::crash_report::take_unseen(&state.paths.logs_dir) else {
        return;
    };
    log::info!("previous run left a crash report: {}", report.display());

    notifications.show_localized(ui, "warning", CRASH_TOAST_KIND, |ui| {
        let g = ui.global::<Settings>();
        RowText {
            title: g.invoke_crash_report_title(),
            message: g.invoke_crash_report_message(),
            action_label: g.invoke_crash_report_action_label(),
        }
    });
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
