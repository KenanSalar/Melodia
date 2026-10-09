//! `Updater.check()` — the "Check for Updates" button.

use slint::Weak;
use tokio::sync::watch;

use melodia_app::services::updater::{FailureKind, PanelPaint, UpdaterEvent, run_check};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::AppWindow;

use super::paint::{check_painter, report_failure};

/// Runs the daily task's check on demand. Unlike that background check, a failure here is shown:
/// the user asked, and is looking at the panel.
pub(super) fn spawn_manual_check(
    state: AppState,
    weak: Weak<AppWindow>,
    event_tx: watch::Sender<Option<UpdaterEvent>>,
) {
    let paint = check_painter(weak.clone());
    paint(PanelPaint::CheckStarted);
    let runtime = state.runtime.clone();
    runtime.spawn(async move {
        let checked = run_check(state.http_client(), &state.paths).await;
        paint(PanelPaint::CheckEnded);

        match checked {
            Ok(finding) => finding.deliver(&paint, &event_tx),
            Err(e) => {
                let kind = FailureKind::classify(&e);
                log::warn!("updater: manual check failed ({kind:?}): {}", describe(&e));
                report_failure(&weak, &event_tx, format!("{e}"), kind);
            }
        }
    });
}
