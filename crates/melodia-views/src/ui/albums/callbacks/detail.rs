//! `AlbumDetail.*` callbacks: close-detail here, the rest through
//! [`crate::ui::callbacks::track_detail`].

use std::sync::Arc;

use slint::{ComponentHandle, Global as _};

use crate::ui::albums::AlbumsUi;
use crate::ui::callbacks::track_detail;
use crate::ui::my_library::return_to_section;
use melodia_app::state::AppState;
use melodia_ui::{AlbumDetail, AppWindow};

/// Wire the `AlbumDetail` callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    track_detail::wire(&ui.global::<AlbumDetail>().as_weak(), state, albums_ui);
    wire_close(ui, state, albums_ui);
}

/// The header's back button. Releases the detail-tier `(cover, blur)` pair off-thread and
/// re-warms the grid it hands back to, so its cards are cache hits when it mounts.
fn wire_close(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let s = state.clone();
    let au = albums_ui.clone();
    let weak = ui.as_weak();
    ui.global::<AlbumDetail>().on_close_detail(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<AlbumDetail>();

        // Read only by the cross-section close, whose `selected-index` write mounts a page. A
        // same-page back mounts a body, which takes a fixed `below` (see `ui::nav_transition`).
        crate::ui::nav_transition::mark_drill_back(&ui);

        // If another *section* opened this detail (Favorites, Search, …), return to it in the
        // same UI-thread tick as the id reset, so Slint reroutes straight there without an
        // Albums-grid frame. A drill from a sibling tab records no origin, the tab bar having
        // said Albums for the whole visit. See `cross_tab_nav::origin_stamp`.
        let origin = g.get_origin_nav_index();
        let origin_was_cross_section = origin >= 0;
        if origin_was_cross_section {
            return_to_section(&ui, origin);
            g.set_origin_nav_index(-1);
        }

        track_detail::forget_closed(&ui, &s, &*au, &g);

        let au = au.clone();
        s.runtime.spawn_blocking(move || {
            au.release_detail_artwork();
            // Routing to another section, the grid isn't going to mount, and one tier serves
            // every grid: prewarming would evict covers the destination still needs.
            if !origin_was_cross_section {
                au.prewarm_visible_covers();
            }
        });
    });
}
