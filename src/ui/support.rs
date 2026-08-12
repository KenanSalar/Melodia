//! The Ko-fi link, and the one prompt that makes it findable.
//!
//! A row in Settings → About is invisible to anyone who never opens that tab, so the
//! link on its own is the same as no link. Hence a single toast, once ever, on an early
//! launch that isn't the first — and hence this module rather than
//! [`super::settings::about`]: the About row and the toast's action button fire the same
//! `Settings.open-kofi`, so neither owns it.
//!
//! Nothing here changes what the player does. There is no paywall, no account, no
//! recurring reminder and nothing modal; dismissing the toast is free, the flag having
//! already been spent by the time it is on screen.

use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;

use crate::error::AppError;
use crate::state::AppState;
use crate::ui::launcher;
use crate::ui::shell::notifications::{NotificationParams, NotificationsUi};
use crate::{AppWindow, Settings};

/// A literal rather than a manifest field — Cargo has no `funding` key to read it out
/// of, unlike `about.rs`'s `CARGO_PKG_REPOSITORY`.
const KOFI_URL: &str = "https://ko-fi.com/kenansalar";

/// Routes the toast's action button. Must match the `kind ==` arm in the
/// `Notifications.action` dispatcher (`globals/updater.slint`) — a mismatch still paints
/// the button, since `notification-stack.slint` gates only on a non-empty action label,
/// and clicking it falls off the end of a dispatcher with no `else`. Pinned by
/// `tests/support_tests.rs`.
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

/// Open the Ko-fi page in the system browser, off the UI thread.
///
/// [`launcher::open_target`] owns the hop; `open::that_detached` still forks and execs a
/// child launcher, which is not something to do on the event loop.
fn wire_open_kofi(ui: &AppWindow, state: &AppState) {
    let runtime = state.runtime.clone();
    ui.global::<Settings>().on_open_kofi(move || {
        runtime.spawn(launcher::open_target(KOFI_URL, "open-kofi"));
    });
}

/// Count this launch and, if it is the one that asks, raise the toast a couple of
/// minutes in.
///
/// Runs on the UI thread so it can hold the `Rc<NotificationsUi>` and resolve the
/// strings through `Settings` at push time — the locale that is active when the toast
/// appears, not the one that was active at boot. Both settings writes go out through the
/// blocking pool.
///
/// The two writes are deliberately not one. The count lands at boot, so a launch is a
/// launch however short; the *seen* flag lands beside the `show`, so a session that ends
/// inside the delay leaves the prompt for the next one instead of spending it on nobody.
/// Neither is tied to the dismiss — a toast the user closes must not come back.
///
/// The message says so out loud ("You'll only see this once"), which makes that second
/// write the one thing here that is a promise rather than a preference. It goes out
/// through `persist_blocking`, which logs a failed write and swallows it, so a config
/// directory that is read-only or full shows the toast again next launch. Accepted: a
/// `settings.json` that won't write has broken more than this.
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
            .spawn_blocking(move || crate::library::settings::record_launch(&counting))
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
            crate::library::settings::mark_support_prompt_seen,
        );

        // Sticky, for the crash notice's reason: this fires once ever, so a notice the
        // user was looking away for is a notice that did nothing.
        let g = ui.global::<Settings>();
        notifications.show(NotificationParams {
            variant: "info".into(),
            title: g.invoke_support_prompt_title(),
            message: g.invoke_support_prompt_message(),
            action_label: g.invoke_support_prompt_action_label(),
            action_kind: SUPPORT_TOAST_KIND.into(),
        });
    }))
    .map(|_| ())
    .map_err(|e| AppError::Window(format!("support prompt: {e}")))
}

#[cfg(test)]
#[path = "tests/support_tests.rs"]
mod tests;
