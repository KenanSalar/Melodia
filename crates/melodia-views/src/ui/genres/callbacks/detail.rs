//! `GenreDetail.*` callbacks: close-detail here, the rest through
//! [`crate::ui::callbacks::track_detail`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::{grid_tab, track_detail};
use crate::ui::genres::GenresUi;
use crate::ui::my_library::return_to_section;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, GenreDetail};

pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    track_detail::wire(state, genres_ui);
    wire_close(ui, state, genres_ui);
}

/// The header's back button, with the cross-section origin pattern `albums/callbacks/detail.rs`
/// argues. No `(cover, blur)` pair to drop, but a long track list's strings are still worth a
/// trim.
fn wire_close(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let s = state.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    ui.global::<GenreDetail>().on_close_detail(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<GenreDetail>();

        crate::ui::nav_transition::mark_drill_back(&ui);
        let origin = g.get_origin_nav_index();
        let origin_was_cross_section = origin >= 0;
        if origin_was_cross_section {
            return_to_section(&ui, origin);
            g.set_origin_nav_index(-1);
        }

        track_detail::forget_closed(&ui, &s, &*gu, &g);
        if !origin_was_cross_section {
            grid_tab::reload_owed_grid(&ui, &s, &gu);
        }

        let gu = gu.clone();
        s.runtime.spawn_blocking(move || gu.release_caches());
    });
}
