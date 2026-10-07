//! The first panel's one-click offer of the platform's Music folder.
//!
//! Kept current while the card is up rather than read once at open: the picker beside it can add
//! a folder covering the Music folder, and an offer left standing would then be refused as a
//! duplicate the moment it was clicked.

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::ui::settings::library_settings::add_folder_and_scan;
use crate::ui::signal::on_signal;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_core::utils::redact::redact_home;
use melodia_ui::{AppWindow, Onboarding};

pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    wire_add(ui, state);

    let s = state.clone();
    let refreshed =
        on_signal(&state.library_changed, ui.as_weak(), "onboarding-folder", move |ui| {
            if ui.global::<Onboarding>().get_mounted() {
                refresh(ui, &s);
            }
        });
    if let Err(e) = refreshed {
        log::warn!(
            "The welcome card's Music folder offer won't follow the library: {}",
            describe(&e)
        );
    }
}

/// Publish the offer, or withdraw it where there is none.
pub(super) fn refresh(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    state.runtime.spawn(async move {
        let shown = match library::settings::suggested_music_folder(&s).await {
            Ok(folder) => folder
                .map(|path| redact_home(&path.to_string_lossy()).into_owned())
                .unwrap_or_default(),
            Err(e) => {
                log::warn!(
                    "Could not check the Music folder for the welcome card: {}",
                    describe(&e)
                );
                String::new()
            }
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.global::<Onboarding>().set_suggested_folder(SharedString::from(shown));
        });
    });
}

/// Add the offered folder. Resolved again at the click rather than read back off the label, which
/// carries a `~` where the home directory was and may describe a library that has since moved.
fn wire_add(ui: &AppWindow, state: &AppState) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Onboarding>().on_add_suggested_folder(move || {
        let Some(ui) = weak.upgrade() else { return };
        // Withdrawn at the click so a second one can't queue a duplicate behind the first.
        ui.global::<Onboarding>().set_suggested_folder(SharedString::default());

        let s = s.clone();
        let weak = weak.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            match library::settings::suggested_music_folder(&s).await {
                Ok(Some(path)) => {
                    add_folder_and_scan(&s, &weak, path.to_string_lossy().into_owned()).await;
                }
                Ok(None) => {}
                Err(e) => log::warn!("Could not add the Music folder: {}", describe(&e)),
            }
        }));
    });
}
