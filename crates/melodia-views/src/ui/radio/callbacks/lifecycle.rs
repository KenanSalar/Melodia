//! What entering and leaving the section costs; why the leave keeps every list is
//! [`crate::ui::radio`]'s module doc. No count is rewound, no model being cleared.
//!
//! **The hero is the exception, and the only thing the dirty flag describes**: its images and the
//! colours it published into the shared globals can't outlive the page, another section's hero
//! painting from them, so the leave hands them back and the enter rebuilds them from the seats.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::detail_view::{release_hero_slots, release_shared_hero};
use crate::ui::radio::{RadioUi, browse, covers, detail, facets, kept};
use melodia_app::state::AppState;
use melodia_app::tasks::TaskSpawner;
use melodia_ui::{AppWindow, CardSelection, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();

    let s = state.clone();
    let ru = radio_ui.clone();
    let weak = ui.as_weak();
    g.on_section_active_changed(move |active| {
        ru.section.set_active(active);
        let Some(ui) = weak.upgrade() else { return };
        if active {
            enter(&ui, &s, &ru);
        } else {
            // The card selection goes back with the page though its rows stay
            // (`card-selection.slint`).
            ui.global::<CardSelection>().invoke_clear();
            leave(&ui, &s, &ru);
        }
    });
}

/// Brings the page up to date. The two `browse` calls guard each other's case; the kept lists are
/// re-read unconditionally, being `SQLite` and the fetch Browse's stars come from.
fn enter(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    kept::refresh(ui, state, radio_ui);
    browse::ensure_loaded(ui, state, radio_ui);
    browse::rewarm(ui, state, radio_ui);
    facets::prime(ui, state, radio_ui);
    // Consumed whether or not a page is open, so an idle leave can't leave it armed.
    if radio_ui.section.take_dirty() {
        detail::rewarm_hero(ui, state, radio_ui);
    }
}

/// Hand back the tier and, where a station page is open, its hero.
fn leave(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    // Synchronously, ahead of the release below; the enter consumes it.
    radio_ui.mark_dirty();
    covers::release(ui, radio_ui);

    // Any tab seated, not just the painted one: a leave mid-collapse kills the band's
    // `hero-collapsed` timer, leaving the slots holding a station nothing paints.
    let g = ui.global::<Radio>();
    if detail::any_seated(radio_ui) {
        // Each tab keeps its station, but the shared globals go with the images, a different hero
        // painting over them. The seats stay set, so `hero_chips::is_open` reads the nav index.
        release_hero_slots(&g);
        release_shared_hero(ui);
        detail::release_seated_artwork(radio_ui);
        let ru = radio_ui.clone();
        state.runtime.spawn_blocking(move || ru.release_detail_artwork());
    } else {
        // The arena walk can't run on the UI thread; the seated arm trims in
        // `release_detail_artwork`.
        state.runtime.spawn_blocking(melodia_platform::services::platform::allocator::trim);
    }

    melodia_app::tasks::radio_logo_cache::spawn(&TaskSpawner::from_state(state), state);
}
