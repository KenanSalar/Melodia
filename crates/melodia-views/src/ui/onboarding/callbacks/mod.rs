//! The card's wiring. One way out, shared by the X, the backdrop, Escape and Skip.

use std::rc::Rc;
use std::time::Duration;

use crate::ui::settings::settings_page::{self, SettingsTab};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Onboarding, Settings, Theme};
use slint::ComponentHandle;

pub(super) fn wire(ui: &AppWindow, state: &AppState, deferred: Rc<super::DeferredOnce>) {
    wire_dismiss(ui, state, deferred);
    wire_open_services(ui);
    wire_run_again(ui, state);
}

/// The Settings ▸ About row, and the only way back to the card once it has been seen — which is
/// what makes an early dismissal safe to treat as final.
fn wire_run_again(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let state = state.clone();
    ui.global::<Settings>().on_run_onboarding(move || {
        if let Some(ui) = weak.upgrade() {
            super::open(&ui, &state);
        }
    });
}

/// Land on Settings ▸ Services and close the card.
fn wire_open_services(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Onboarding>().on_open_services(move || {
        let Some(ui) = weak.upgrade() else { return };
        settings_page::open_on(&ui, SettingsTab::Services);
        ui.global::<Onboarding>().invoke_dismiss();
    });
}

/// Mark the card seen and close it.
///
/// Every dismissal path lands here, including Skip on the first panel: a card that comes back
/// because it was closed early is a nag, and Settings ▸ About is where someone who dismissed by
/// reflex gets it again.
fn wire_dismiss(ui: &AppWindow, state: &AppState, deferred: Rc<super::DeferredOnce>) {
    let weak = ui.as_weak();
    let state = state.clone();

    ui.global::<Onboarding>().on_dismiss(move || {
        let Some(ui) = weak.upgrade() else { return };
        let onboarding = ui.global::<Onboarding>();
        // Esc pressed twice inside the fade would otherwise queue a second persist and a second
        // unmount timer.
        if !onboarding.get_open() {
            return;
        }
        onboarding.set_open(false);

        state.persist_blocking("onboarding_version", library::settings::set_onboarding_seen);

        // Held until the fade has run, so the card isn't cut off mid-animation. Taken from the
        // token the overlay animates on rather than restated here.
        let fade_ms = u64::try_from(ui.global::<Theme>().get_dur_fast()).unwrap_or(0);
        let fade = Duration::from_millis(fade_ms);
        let weak = ui.as_weak();
        let deferred = Rc::clone(&deferred);
        slint::Timer::single_shot(fade, move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Onboarding>().set_mounted(false);
            }
            // The surfaces boot held back so the card wasn't one of three at once.
            deferred.run();
        });
    });
}
