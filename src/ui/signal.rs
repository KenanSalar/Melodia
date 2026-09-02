//! Running a closure on the UI thread every time a [`Signal`] moves.
//!
//! Four subscribers wanted the same twelve lines — subscribe, `spawn_local` a loop, `await` the
//! change, upgrade the weak handle, do the work — and each spelled them out with its own error
//! label. The loop is the whole of what they shared and none of what they differ on.
//!
//! `changed()` marks the value seen on its own, so there is deliberately no `borrow_and_update`
//! here: the counter is a tick, not a payload, and nothing downstream reads it.

use slint::Weak;

use crate::AppWindow;
use crate::error::{AppError, AppResult};
use crate::state::Signal;

/// Run `on_tick` on the UI thread after every bump of `signal`, until the window goes away.
///
/// `label` names the subscriber in the one error this can raise, which is `spawn_local` refusing
/// off the event loop.
///
/// The closure takes no `Send` bound on purpose: `spawn_local` runs it on the UI thread, so a
/// subscriber may capture the `Rc` handles the shell owns.
pub fn on_signal<F>(
    signal: &Signal,
    weak: Weak<AppWindow>,
    label: &'static str,
    on_tick: F,
) -> AppResult<()>
where
    F: Fn(&AppWindow) + 'static,
{
    let mut rx = signal.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let Some(ui) = weak.upgrade() else { break };
            on_tick(&ui);
        }
    }))
    .map(|_| ())
    .map_err(|e| AppError::Window(format!("{label} subscriber: {e}")))
}
