//! The first-run welcome card.
//!
//! In `ui/` at the root rather than under `settings/`: the card is owned by no page, the way
//! `sleep_timer` and `equalizer` are. It takes `(&AppWindow, &AppState)` for the same reason —
//! `ViewCtx` carries the cover tier and `views.json` that the ten nav sections need and this
//! doesn't.
//!
//! **Rust owns the mount and unmount timing.** The overlay is `if`-mounted on
//! `Onboarding.mounted`, so a `changed` handler inside it would outlive its own branch and panic
//! the next time the watched property moved — which re-running the card does. So the two edges are
//! `slint::Timer::single_shot` here instead: one frame after `mounted` to raise `open` and give
//! `animate` an edge to run on, and one fade later to drop the branch.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use melodia_app::services::settings::SettingsData;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Onboarding};
use slint::ComponentHandle;

mod callbacks;

/// Work `main` hands over to run once the card is gone, or immediately if it never opens.
///
/// The first thirty seconds shouldn't be three stacked surfaces, so the update toast and the crash
/// notice wait behind the card. **Deferred, not suppressed**: the crash notice consumes its marker
/// when it fires, so skipping it loses the report. `FnOnce` behind a `Cell` because the dismiss
/// callback is an `Fn` that a stray second Escape can re-enter.
pub(super) type Deferred = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

/// One frame, so the branch exists before `open` moves and the fade has an edge to animate from.
/// `init` would be too early: it runs after bindings resolve, making `true` the initial value.
const MOUNT_SETTLE: Duration = Duration::from_millis(1);

/// Wire the card and open it if this install has not seen the current revision.
///
/// `startup_settings` is the snapshot `main` already read; `None` means the file was unreadable,
/// which on a first run is exactly what a missing file looks like, so the card is owed. `deferred`
/// runs when the card closes, or right here when it never opens.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    startup_settings: Option<&SettingsData>,
    deferred: impl FnOnce() + 'static,
) {
    let owed = startup_settings.is_none_or(|settings| settings.onboarding.needs_onboarding());
    if !owed {
        callbacks::wire(ui, state, Rc::new(RefCell::new(None)));
        deferred();
        return;
    }

    callbacks::wire(ui, state, Rc::new(RefCell::new(Some(Box::new(deferred)))));
    open(ui);
}

/// Mount the card, then raise it a frame later.
pub fn open(ui: &AppWindow) {
    let onboarding = ui.global::<Onboarding>();
    onboarding.set_step(0);
    onboarding.set_mounted(true);

    let weak = ui.as_weak();
    slint::Timer::single_shot(MOUNT_SETTLE, move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Onboarding>().set_open(true);
        }
    });
}

#[cfg(test)]
#[path = "tests/onboarding_tests.rs"]
mod tests;
