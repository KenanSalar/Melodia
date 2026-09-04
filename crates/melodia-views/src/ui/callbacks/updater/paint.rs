//! UI-thread helpers that paint the `MelodiaUpdater` global from the
//! background check / install futures. Each hops to the event loop via
//! `upgrade_in_event_loop`, so they're safe to call from any thread.

use slint::{ComponentHandle, SharedString, Weak};

use melodia_ui::{AppWindow, MelodiaUpdater};

pub(super) fn set_is_checking(weak: &Weak<AppWindow>, on: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<MelodiaUpdater>().set_is_checking(on);
    });
}

pub(super) fn set_is_installing(weak: &Weak<AppWindow>, on: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<MelodiaUpdater>().set_is_installing(on);
        if !on {
            ui.global::<MelodiaUpdater>().set_download_progress(0);
        }
    });
}

pub(super) fn paint_up_to_date(weak: &Weak<AppWindow>) {
    let _ = weak.upgrade_in_event_loop(|ui| {
        let g = ui.global::<MelodiaUpdater>();
        g.set_up_to_date(true);
        g.set_update_available(false);
        g.set_error_message("".into());
    });
}

pub(super) fn paint_available(
    weak: &Weak<AppWindow>,
    version: String,
    notes_short: String,
    critical: bool,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<MelodiaUpdater>();
        g.set_up_to_date(false);
        g.set_update_available(true);
        g.set_available_version(SharedString::from(version));
        g.set_notes_short(SharedString::from(notes_short));
        g.set_is_critical(critical);
        g.set_error_message("".into());
    });
}

pub(super) fn paint_error(weak: &Weak<AppWindow>, reason: String) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<MelodiaUpdater>().set_error_message(SharedString::from(reason));
    });
}

pub(super) fn paint_restart_needed(weak: &Weak<AppWindow>) {
    let _ = weak.upgrade_in_event_loop(|ui| {
        let g = ui.global::<MelodiaUpdater>();
        g.set_is_installing(false);
        g.set_restart_needed(true);
        g.set_download_progress(100);
    });
}
