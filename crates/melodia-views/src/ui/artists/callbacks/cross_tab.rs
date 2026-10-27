//! Artist Detail's Albums strip: its cover lookup, and the hand-off to an album's detail on the
//! Albums tab.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::albums::AlbumsUi;
use crate::ui::callbacks::cross_tab_nav;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, ArtistDetail};

pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let detail = ui.global::<ArtistDetail>();
    let weak = ui.as_weak();

    {
        // Blocking, unlike the grid's lookup: this callback carries no generation, so a scheduled
        // decode would have nothing to bring the card back.
        detail.on_request_album_cover(|path| {
            crate::ui::grid_prewarm::grid_cover_blocking(path.as_str())
        });
    }

    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        detail.on_open_album(move |album_id| {
            let Some(ui) = weak.upgrade() else { return };
            cross_tab_nav::open_album_cross_tab(
                &s,
                &au,
                &weak,
                i64::from(album_id),
                cross_tab_nav::Origin::read(&ui),
                "artists::open_album_cross_tab",
            );
        });
    }
}
