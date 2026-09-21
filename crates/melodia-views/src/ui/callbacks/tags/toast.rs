//! What the user is told after a Save.

use std::rc::Rc;

use slint::{ComponentHandle, Model, SharedString};

use crate::ui::shell::notifications::{Completion, NotificationsUi, RowText};
use crate::ui::util::len_as_i32;
use melodia_app::library::tags::TagEditReport;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Settings};

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
    let message = named_unsupported_message(&settings, &report)
        .filter(|_| failed == 0)
        .unwrap_or_else(|| settings.invoke_tag_edit_message(failed, unsupported));
    notifications.show_completion(
        Completion::partial_if(failed > 0 || unsupported > 0),
        settings.invoke_tag_edit_title(len_as_i32(report.updated)),
        message,
    );
}

/// "Performer can't be saved to MP3 files", where the batch agrees on both halves.
///
/// `None` puts the caller back on the count, which is the honest answer for a batch that fell out
/// several different ways: one sentence naming one field and one format would be wrong about the
/// rest of it.
fn named_unsupported_message(settings: &Settings, report: &TagEditReport) -> Option<SharedString> {
    let (format, fields) = report.unsupported_agreement()?;
    let labels = settings.get_tag_field_labels();
    // Every field has a label, so a missing one is the list having drifted from `TagField` rather
    // than a field legitimately having none. Fall back rather than name the wrong field.
    let named: Option<Vec<SharedString>> =
        fields.iter().map(|field| field.label_index().and_then(|i| labels.row_data(i))).collect();
    // Joined here rather than in Slint, which has no way to build a string from a model: the
    // six shipped locales all separate a list this way.
    let joined = named?.join(", ");
    Some(settings.invoke_tag_unsupported_message(joined.into(), format.into()))
}

/// Sticky, hence the recipe: a row still up when the language changes has to follow it.
fn show_failure_toast(ui: &AppWindow, notifications: &NotificationsUi) {
    notifications.show_failure(ui, |ui| {
        let g = ui.global::<Settings>();
        RowText::plain(g.invoke_tag_edit_failed_title(), g.invoke_tag_edit_failed_message())
    });
}
