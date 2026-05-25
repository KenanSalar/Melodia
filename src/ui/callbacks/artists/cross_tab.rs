//! Artist Detail → Albums cross-tab hand-off: the Albums sub-section's
//! lazy cover lookup and the `open-album` callback that flips the sidebar
//! to the Albums tab and opens that album's detail in one atomic frame.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::{AlbumDetail, AppWindow, ArtistDetail, Nav};

use super::{NAV_ALBUMS, NAV_ARTISTS};

/// Wire the Artist Detail Albums sub-section callbacks. See
/// [`super::wire_artists`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let detail = ui.global::<ArtistDetail>();
    let weak = ui.as_weak();

    // request-album-cover: lazy per-card cover lookup for the Albums
    // sub-section. Shares the Albums-tab grid-tier cache so the same
    // 448 px decode serves both surfaces.
    {
        let albums_ui_cb = albums_ui.clone();
        detail.on_request_album_cover(move |path| albums_ui_cb.grid_cover(path.as_str()));
    }

    // open-album: cross-tab nav. Stash the originating sidebar index so
    // the back button can restore it, then fetch the album detail in the
    // background; the Nav flip itself happens inside the same
    // `upgrade_in_event_loop` closure that writes `AlbumDetail.album-id`
    // (via the `open_album_with` `on_applied` hook), so the user sees one
    // atomic transition Artist Detail → Album Detail with no Albums-grid
    // frame in between. `AlbumsUi::open_album_with` already persists
    // `view_id::ALBUM_DETAIL`, so the next launch reopens that album.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        detail.on_open_album(move |album_id| {
            let id = i64::from(album_id);
            let Some(ui) = weak.upgrade() else { return };

            // Remember the origin synchronously: even if the user
            // immediately spam-clicks back, the value is set before any
            // async work yields. `wire_albums::on_close_detail` reads
            // this on the back path to restore `Nav.selected-index`.
            ui.global::<AlbumDetail>().set_origin_nav_index(NAV_ARTISTS);

            let s_fetch = s.clone();
            let au_fetch = au.clone();
            let weak_fetch = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = albums_ui_mod::open_album_with(
                    &s_fetch,
                    &au_fetch,
                    weak_fetch,
                    id,
                    crate::NavEnterFrom::Right,
                    |ui: &AppWindow| {
                        // Same-frame Nav flip: runs as the last statement
                        // of `open_album_with`'s `upgrade_in_event_loop`
                        // closure, after `album-id` is set. Guard on the
                        // current `Nav.selected-index` so a sidebar nav
                        // away during the fetch isn't clobbered — if the
                        // user moved off the Artists tab while the fetch
                        // was in flight, leave them where they are
                        // (`album-id` is still written but invisible).
                        let nav = ui.global::<Nav>();
                        if nav.get_selected_index() == NAV_ARTISTS {
                            nav.set_selected_index(NAV_ALBUMS);
                            nav.invoke_persist_selected_index(NAV_ALBUMS);
                        }
                    },
                )
                .await
                {
                    log::warn!("artists::open_album_cross_tab: {e}");
                }
            });

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::ALBUM_DETAIL,
                    Some(id),
                ) {
                    log::warn!("artists::open_album_cross_tab persist: {e}");
                }
            });
        });
    }
}
