//! What the user is told after a Save.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::shell::notifications::{NotificationParams, NotificationsUi, RowText};
use crate::ui::util::len_as_i32;
use melodia_app::library::tags::TagEditReport;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Settings};

/// Auto-dismiss window for the completion toast, matching the playlist
/// import/export toasts.
const TOAST_MS: u32 = 3000;

/// Show the Save completion toast from the report — a partial failure or an
/// unsupported field must be visible, not swallowed.
pub(super) fn show_report_toast(
    ui: &AppWindow,
    notifications: &Rc<NotificationsUi>,
    result: Result<TagEditReport, AppError>,
) {
    let settings = ui.global::<Settings>();
    let report = match result {
        Ok(report) => report,
        Err(e) => {
            log::warn!("apply_tag_edit: {e}");
            show_failure_toast(ui, notifications);
            return;
        }
    };

    if report.updated == 0 {
        show_failure_toast(ui, notifications);
        return;
    }

    let failed = len_as_i32(report.failures.len());
    let unsupported = len_as_i32(report.unsupported.len());
    let variant = if failed == 0 && unsupported == 0 { "success" } else { "warning" };
    notifications.show_auto_dismiss(
        NotificationParams::plain(
            variant,
            settings.invoke_tag_edit_title(len_as_i32(report.updated)),
            settings.invoke_tag_edit_message(failed, unsupported),
        ),
        TOAST_MS,
    );
}

/// Sticky, hence the recipe: a row still up when the language changes has to follow it.
fn show_failure_toast(ui: &AppWindow, notifications: &NotificationsUi) {
    notifications.show_localized(ui, "error", "", |ui| {
        let g = ui.global::<Settings>();
        RowText::plain(g.invoke_tag_edit_failed_title(), g.invoke_tag_edit_failed_message())
    });
}
