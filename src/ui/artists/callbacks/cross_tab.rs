//! Artist Detail → Albums cross-tab hand-off: the Albums sub-section's
//! lazy cover lookup and the `open-album` callback that moves to the Albums
//! tab and opens that album's detail in one atomic frame.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::callbacks::cross_tab_nav;
use crate::{AppWindow, ArtistDetail};

/// Wire the Artist Detail Albums sub-section callbacks. See
/// [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let detail = ui.global::<ArtistDetail>();
    let weak = ui.as_weak();

    // request-album-cover: lazy per-card cover lookup for the Albums
    // sub-section. Shares the Albums-tab grid-tier cache so the same
    // one grid-tier decode serves both surfaces.
    {
        let albums_ui_cb = albums_ui.clone();
        detail.on_request_album_cover(move |path| albums_ui_cb.grid_cover(path.as_str()));
    }

    // open-album: the shared cross-tab hand-off, which owns all of it — stamp the
    // origin pair synchronously, fetch, and move to the Albums tab inside the same
    // `upgrade_in_event_loop` closure that writes `AlbumDetail.album-id`, so the user
    // sees one atomic Artist Detail → Album Detail transition with no Albums-grid frame
    // between. This was a hand-rolled copy of that function with the origin hardcoded to
    // the Artists tab; `Origin::read` is the same answer, and asking for it keeps the
    // "did the user navigate away mid-fetch" guard spelled in exactly one place.
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
