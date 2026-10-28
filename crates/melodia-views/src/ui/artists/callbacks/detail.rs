//! `ArtistDetail.*` callbacks: close-detail and the Albums sub-section collapse here, the rest
//! through [`crate::ui::callbacks::track_detail`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::macros::spawn_blocking_logged;
use crate::ui::callbacks::{grid_tab, track_detail};
use crate::ui::my_library::return_to_section;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, ArtistDetail};

pub(super) fn wire(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    track_detail::wire(state, artists_ui);
    wire_close(ui, state, artists_ui);
    wire_albums_collapse(ui, state);
}

/// The header's back button, with the cross-section origin pattern `albums/callbacks/detail.rs`
/// argues.
fn wire_close(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let s = state.clone();
    let au = artists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<ArtistDetail>().on_close_detail(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<ArtistDetail>();

        crate::ui::nav_transition::mark_drill_back(&ui);
        let origin = g.get_origin_nav_index();
        let origin_was_cross_section = origin >= 0;
        if origin_was_cross_section {
            return_to_section(&ui, origin);
            g.set_origin_nav_index(-1);
        }

        track_detail::forget_closed(&ui, &s, &*au, &g);
        let reloaded = !origin_was_cross_section && grid_tab::reload_owed_grid(&ui, &s, &au);

        let au = au.clone();
        s.runtime.spawn_blocking(move || {
            au.release_detail_artwork();
            // One arm or the other: the grid and the strip share a tier, so a hand-back beside the
            // prewarm would shrink the screenful it just decoded. Leaving the page, the pixels
            // are dead weight; back on the grid, an owed reload warms it itself.
            if origin_was_cross_section {
                crate::ui::grid_prewarm::hand_back_covers();
            } else if !reloaded {
                au.prewarm_visible_covers();
            }
        });
    });
}

/// Flips the flag synchronously so the UI repaints this frame, then persists it. Collapsing hands
/// the tier's pixels back, the strip's scroller being unmounted; its blocking lookup takes a
/// proxy for a miss, so the strip re-decodes on the way back either way.
fn wire_albums_collapse(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<ArtistDetail>().on_toggle_albums_collapsed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<ArtistDetail>();
        let new_state = !g.get_albums_collapsed();
        g.set_albums_collapsed(new_state);
        if new_state {
            s.runtime.spawn_blocking(crate::ui::grid_prewarm::hand_back_covers);
        }
        let s = s.clone();
        spawn_blocking_logged!(
            s,
            "artists::set_albums_collapsed",
            library::settings::set_artist_albums_collapsed(&s.paths, new_state)
        );
    });
}
