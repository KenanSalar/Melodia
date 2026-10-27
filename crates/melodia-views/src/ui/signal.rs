//! Running a closure on the UI thread every time a [`Signal`] moves. There is deliberately no
//! `borrow_and_update`: `changed()` marks the value seen itself, and the counter is a tick
//! nothing reads.

use slint::Weak;

use melodia_app::state::Signal;
use melodia_core::error::{AppError, AppResult};
use melodia_ui::AppWindow;

/// Run `on_tick` on the UI thread after every bump of `signal`, until the window goes away.
/// `label` names the subscriber in the one error this raises, `spawn_local` refusing off the
/// event loop. No `Send` bound, so a subscriber may capture the shell's `Rc` handles.
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
    .map_err(|e| AppError::io(format!("{label} subscriber"), e))
}
