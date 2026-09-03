//! Running a closure on the UI thread every time a [`Signal`] moves.
//!
//! The subscribe-and-spawn loop is the whole of what the four subscribers share and none of what
//! they differ on, so what each does with the tick is all that stays at the call site.
//!
//! `changed()` marks the value seen on its own, so there is deliberately no `borrow_and_update`
//! here: the counter is a tick, not a payload, and nothing downstream reads it.

use slint::Weak;

use melodia_app::state::Signal;
use melodia_core::error::{AppError, AppResult};
use melodia_ui::AppWindow;

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
