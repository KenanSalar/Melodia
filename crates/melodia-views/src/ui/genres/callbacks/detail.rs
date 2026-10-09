//! `GenreDetail.*` callbacks: close-detail here, the rest through
//! [`crate::ui::callbacks::track_detail`].

use std::sync::Arc;

use slint::{ComponentHandle, Global as _};

use crate::ui::callbacks::track_detail;
use crate::ui::genres::GenresUi;
use crate::ui::my_library::return_to_section;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, GenreDetail};

/// Wire the `GenreDetail` callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    track_detail::wire(&ui.global::<GenreDetail>().as_weak(), state, genres_ui);
    wire_close(ui, state, genres_ui);
}

/// The header's back button. Genres have no `(cover, blur)` pair, but a long track list holds
/// tens of `SharedString`s per row, so the bulk free is still worth a trim.
fn wire_close(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let s = state.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    ui.global::<GenreDetail>().on_close_detail(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<GenreDetail>();

        // See `albums/callbacks/detail.rs` for why only a cross-section close reads this, and
        // for why a drill from a sibling tab records no origin.
        crate::ui::nav_transition::mark_drill_back(&ui);
        let origin = g.get_origin_nav_index();
        if origin >= 0 {
            return_to_section(&ui, origin);
            g.set_origin_nav_index(-1);
        }

        track_detail::forget_closed(&ui, &s, &*gu, &g);

        let gu = gu.clone();
        s.runtime.spawn_blocking(move || gu.release_caches());
    });
}
