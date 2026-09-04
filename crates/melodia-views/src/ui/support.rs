//! The Ko-fi link, and the one prompt that makes it findable.
//!
//! A row in Settings → About is invisible to anyone who never opens that tab, so the link
//! on its own is the same as no link. Hence a single toast, once ever, on an early launch
//! that isn't the first — and hence this module rather than [`super::settings::about`]:
//! the About row and the toast's action button fire the same `Settings.open-kofi`, so
//! neither owns it. Nothing here is modal or recurring; dismissing is free, the flag
//! having been spent by the time the toast is on screen.

use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;

use crate::ui::launcher;
use crate::ui::shell::notifications::{NotificationsUi, RowText};
use melodia_app::state::AppState;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Settings};

/// A literal rather than a manifest field — Cargo has no `funding` key to read it out
/// of, unlike `about.rs`'s `CARGO_PKG_REPOSITORY`.
const KOFI_URL: &str = "https://ko-fi.com/kenansalar";

/// Routes the toast's action button, and must match the `kind ==` arm in the
/// `Notifications.action` dispatcher — a mismatch still paints the button,
/// `notification-stack.slint` gating only on a non-empty action label, and clicking it
/// falls off the end of a dispatcher with no `else`.
const SUPPORT_TOAST_KIND: &str = "support-melodia";

/// How long into the qualifying launch the toast waits. Long enough that it lands on
/// someone using the app rather than on someone who just opened it.
const PROMPT_DELAY: Duration = Duration::from_mins(2);

pub fn install(
    ui: &AppWindow,
    state: &AppState,
    notifications: Rc<NotificationsUi>,
) -> Result<(), AppError> {
    wire_open_kofi(ui, state);
    schedule_prompt(ui, state, notifications)
}

/// Open the Ko-fi page in the system browser. [`launcher::open_target`] owns the hop off
/// the UI thread — `open::that_detached` still forks and execs a child launcher.
fn wire_open_kofi(ui: &AppWindow, state: &AppState) {
    let runtime = state.runtime.clone();
    ui.global::<Settings>().on_open_kofi(move || {
        runtime.spawn(launcher::open_target(KOFI_URL, "open-kofi"));
    });
}

/// Count this launch and, if it is the one that asks, raise the toast a couple of minutes
/// in. Runs on the UI thread so it can hold the `Rc<NotificationsUi>` and resolve the
/// strings through `Settings` at push time — the locale active when the toast appears, not
/// at boot.
///
/// The two writes are deliberately not one. The count lands at boot, so a launch is a
/// launch however short; the *seen* flag lands beside the `show`, so a session that ends
/// inside the delay leaves the prompt for the next one instead of spending it on nobody.
/// Neither is tied to the dismiss — a toast the user closes must not come back. That
/// second write goes through `persist_blocking`, which swallows a failure, so a read-only
/// config directory shows the toast again next launch.
fn schedule_prompt(
    ui: &AppWindow,
    state: &AppState,
    notifications: Rc<NotificationsUi>,
) -> Result<(), AppError> {
    let weak = ui.as_weak();
    let state = state.clone();

    slint::spawn_local(async_compat::Compat::new(async move {
        let counting = state.clone();
        let due = state
            .runtime
            .spawn_blocking(move || melodia_app::library::settings::record_launch(&counting))
            .await;
        match due {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return,
            Ok(Err(e)) => {
                log::warn!("support prompt: counting this launch failed: {e}");
                return;
            }
            Err(e) => {
                log::warn!("support prompt: launch-count task join failed: {e}");
                return;
            }
        }

        tokio::time::sleep(PROMPT_DELAY).await;
        let Some(ui) = weak.upgrade() else { return };

        state.persist_blocking(
            "persist support_prompt_seen",
            melodia_app::library::settings::mark_support_prompt_seen,
        );

        // Sticky, for the crash notice's reason: this fires once ever, so a notice the
        // user was looking away for is a notice that did nothing.
        notifications.show_localized(&ui, "info", SUPPORT_TOAST_KIND, |ui| {
            let g = ui.global::<Settings>();
            RowText {
                title: g.invoke_support_prompt_title(),
                message: g.invoke_support_prompt_message(),
                action_label: g.invoke_support_prompt_action_label(),
            }
        });
    }))
    .map(|_| ())
    .map_err(|e| AppError::Window(format!("support prompt: {e}")))
}

#[cfg(test)]
#[path = "tests/support_tests.rs"]
mod tests;
