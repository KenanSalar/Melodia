//! The card's wiring. One way out, shared by the X, the backdrop, Escape and Skip.

use std::rc::Rc;
use std::time::Duration;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Nav, Onboarding, Settings, SettingsPage, Theme};
use slint::ComponentHandle;

/// Settings' own nav index. Spelled here rather than reached for: `ui::my_library` owns the only
/// other one, and neither is the other's to publish.
const NAV_SETTINGS: i32 = 9;

pub(super) fn wire(ui: &AppWindow, state: &AppState, deferred: Rc<super::DeferredOnce>) {
    wire_dismiss(ui, state, deferred);
    wire_open_services(ui);
    wire_run_again(ui);
}

/// The Settings ▸ About row, and the only way back to the card once it has been seen — which is
/// what makes an early dismissal safe to treat as final.
fn wire_run_again(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Settings>().on_run_onboarding(move || {
        if let Some(ui) = weak.upgrade() {
            super::open(&ui);
        }
    });
}

/// Land on Settings ▸ Services and close the card.
///
/// Tab first, then nav, so the page mounts on the body it is meant to show — `my_library::go_to_tab`
/// for the same reason. Both writes go through the callbacks that already own an `IndexPersist`;
/// calling the disk setters directly would be a seventh writer, which
/// `crates/melodia/tests/index_persist.rs` pins against.
fn wire_open_services(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Onboarding>().on_open_services(move || {
        let Some(ui) = weak.upgrade() else { return };

        let page = ui.global::<SettingsPage>();
        let services = page.get_tab_services();
        page.set_tab_idx(services);
        page.invoke_tab_changed(services);

        let nav = ui.global::<Nav>();
        nav.set_selected_index(NAV_SETTINGS);
        nav.invoke_persist_selected_index(NAV_SETTINGS);

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
            // The surfaces `main` held back so the card wasn't one of three at once.
            deferred.run();
        });
    });
}
