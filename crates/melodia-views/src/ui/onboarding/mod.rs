//! The first-run welcome card.
//!
//! In `ui/` at the root rather than under `settings/`: the card is owned by no page, the way
//! `sleep_timer` and `equalizer` are. It takes `(&AppWindow, &AppState)` for the same reason —
//! `ViewCtx` carries the cover tier and `views.json` that the ten nav sections need and this
//! doesn't.
//!
//! **Rust owns the mount and unmount timing.** The overlay is `if`-mounted on
//! `Onboarding.mounted`, so a `changed` handler inside it watching either of these globals would
//! stay registered against a property that outlives its own branch and panic the next time it
//! moves — which re-running the card does to both. So the two edges are `slint::Timer::single_shot`
//! here instead: one frame after `mounted` to raise `open` and give `animate` an edge to run on,
//! and one fade later to drop the branch.

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
/// when it fires, so skipping it loses the report.
///
/// A type rather than a bare `Option` because the once-ness is the contract, and holding an
/// `FnOnce` behind a `take` is what makes a second run unrepresentable: `dismiss` is an `Fn` a
/// stray second Escape can re-enter, and running this twice spawns a second daily updater task.
pub(super) struct DeferredOnce {
    work: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl DeferredOnce {
    fn new(work: impl FnOnce() + 'static) -> Rc<Self> {
        Rc::new(Self { work: RefCell::new(Some(Box::new(work))) })
    }

    /// Run the held work. Every call after the first is a no-op.
    pub(super) fn run(&self) {
        // Taken before the call, not during: the work reaches back into Slint, and a re-entrant
        // `dismiss` inside it would otherwise find the borrow still open.
        let work = self.work.borrow_mut().take();
        if let Some(work) = work {
            work();
        }
    }
}

/// One frame, so the branch exists before `open` moves and the fade has an edge to animate from.
/// `init` would be too early: it runs after bindings resolve, making `true` the initial value.
const MOUNT_SETTLE: Duration = Duration::from_millis(1);

/// Whether this install is owed the card.
///
/// A fresh install arrives here as `Some` at revision `0` — `read_settings` defaults a missing or
/// unparseable file rather than failing — so `None` is the narrower case of a file that exists and
/// won't read. Owed anyway: the alternative suppresses the one surface that explains an empty
/// window on the launch likeliest to have one.
fn card_is_owed(startup_settings: Option<&SettingsData>) -> bool {
    startup_settings.is_none_or(|settings| settings.onboarding.needs_onboarding())
}

/// Wire the card and open it if this install has not seen the current revision.
///
/// `startup_settings` is the snapshot `main` already read; `deferred` runs when the card closes,
/// or right here when it never opens.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    startup_settings: Option<&SettingsData>,
    deferred: impl FnOnce() + 'static,
) {
    let deferred = DeferredOnce::new(deferred);
    callbacks::wire(ui, state, Rc::clone(&deferred));

    if card_is_owed(startup_settings) {
        open(ui);
        return;
    }
    deferred.run();
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
