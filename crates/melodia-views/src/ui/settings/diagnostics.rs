//! The Diagnostics card and the toast after a crash. Everything the logging, crash-report and
//! bundle services write lands in one directory a user launching from a desktop entry has no
//! console or reason to look in, and this is the whole of what surfaces it.

use std::rc::Rc;

use async_compat::Compat;
use chrono::Local;
use slint::{ComponentHandle, SharedString, Weak};

use crate::ui::shell::notifications::{Completion, NotificationsUi, RowText};
use crate::ui::{file_dialog, launcher};
use melodia_app::state::AppState;
use melodia_app::{library, services};
use melodia_platform::services::platform;
use melodia_ui::{AppWindow, NotificationKind, NotificationVariant, Settings};

pub fn install(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    wire_open_log_folder(ui, state);
    wire_save_report(ui, state, notifications);
    wire_verbose_logging(ui, state);
}

/// Opens the logs directory, for the card's button and the crash toast's action alike.
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

/// Asks where the bundle goes, writes it and says how that went. A cancel says nothing.
async fn save_report(state: AppState, weak: Weak<AppWindow>, notifications: Rc<NotificationsUi>) {
    let dialog = file_dialog::parented(&weak, "Save Diagnostics Report")
        .set_file_name(services::diagnostics::suggested_file_name(Local::now()));
    let Some(target) = dialog.save_file().await else {
        return;
    };
    let path = target.path().to_path_buf();

    // Written on the blocking pool: this future runs on the UI thread.
    let result = match services::diagnostics::build_report(&state).await {
        Ok(text) => melodia_core::utils::atomic_file::write_text(path.clone(), text).await,
        Err(e) => Err(e),
    };

    let Some(ui) = weak.upgrade() else { return };
    let settings = ui.global::<Settings>();
    match result {
        Ok(()) => {
            notifications.show_completion(
                Completion::Complete,
                settings.invoke_diagnostics_saved_title(),
                settings.invoke_diagnostics_saved_message(SharedString::from(
                    path.display().to_string(),
                )),
            );
        }
        Err(e) => {
            log::warn!("save-diagnostics-report: {}", melodia_core::error::describe(&e));
            notifications.show_failure(&ui, |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_diagnostics_failed_title(),
                    g.invoke_diagnostics_failed_message(),
                )
            });
        }
    }
}

/// Seeded with `if let Ok` rather than [`crate::ui::settings_bind::read_or_default`], so an
/// unreadable file leaves the Slint-declared `false`, the answer `logging::install` reached off
/// the same read.
fn wire_verbose_logging(ui: &AppWindow, state: &AppState) {
    if let Ok(settings) = services::settings::read_settings(&state.paths) {
        ui.global::<Settings>().set_verbose_logging(settings.diagnostics.verbose_logging);
    }

    let state = state.clone();
    ui.global::<Settings>().on_verbose_logging_changed(move |on| {
        // Live first, so a failed write can't undo what the user already sees working.
        platform::logging::set_verbose(on);
        state.persist_blocking("persist verbose_logging", move |paths| {
            library::settings::set_verbose_logging(paths, on)
        });
    });
}

/// Announces a panic from the previous run, once. Sticky, since a notice the user looked away
/// from did nothing, and synchronous, being one `read_dir` over a handful of entries and a
/// marker write only when there is something to report.
///
/// Called by `boot::ui_setup::install` rather than from [`install`], so a launch that opens the
/// welcome card can hold it until the card closes. It must be **deferred and never skipped**:
/// `take_unseen` consumes the marker, so a suppressed notice is a report the user never hears
/// about.
pub fn notify_previous_crash(ui: &AppWindow, state: &AppState, notifications: &NotificationsUi) {
    let Some(report) = platform::crash_report::take_unseen(&state.paths.logs_dir) else {
        return;
    };
    log::info!("previous run left a crash report: {}", report.display());

    notifications.show_localized(
        ui,
        NotificationVariant::Warning,
        NotificationKind::CrashReport,
        |ui| {
            let g = ui.global::<Settings>();
            RowText {
                title: g.invoke_crash_report_title(),
                message: g.invoke_crash_report_message(),
                action_label: g.invoke_crash_report_action_label(),
            }
        },
    );
}

#[cfg(test)]
#[path = "tests/diagnostics_tests.rs"]
mod tests;
