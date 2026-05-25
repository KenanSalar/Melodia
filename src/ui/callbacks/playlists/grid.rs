//! `Playlists.*` grid callbacks: lazy cover lookup, re-chunk on column
//! count, client-side filter, the open-playlist drill-in, and the
//! per-id name / description lookups backing the grid card's Rename overlay.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{AppWindow, Playlists};

/// Wire the `Playlists` grid callbacks. See [`super::wire_playlists`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // request-cover: lazy per-card cover lookup, backed by the
    // playlist-tier `CoverThumbs`. Only on-screen cards invoke this.
    {
        let pu = playlists_ui.clone();
        playlists.on_request_cover(move |path| pu.grid_cover(path.as_str()));
    }

    {
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            playlists_ui_mod::rebuild_grid(&ui, &pu);
        });
    }

    {
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            playlists_ui_mod::rebuild_grid(&ui, &pu);
        });
    }

    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_open_playlist(move |playlist_id| {
            let id = i64::from(playlist_id);

            let s_fetch = s.clone();
            let pu_fetch = pu.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(s_fetch, "playlists::open_playlist",
                playlists_ui_mod::open_playlist(
                    &s_fetch, &pu_fetch, weak_fetch, id, crate::NavEnterFrom::Right));

            let pu_release = pu.clone();
            s.runtime.spawn_blocking(move || pu_release.release_grid_covers());

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::PLAYLIST_DETAIL,
                    Some(id),
                ) {
                    log::warn!("playlists::open_playlist persist: {e}");
                }
            });
        });
    }

    // row-name / row-description: per-id lookups against the cached
    // grid `PlaylistStats` list — back the grid card's "Rename" overlay
    // which only carries the playlist id (the `PlaylistGridRow` doesn't
    // include description).
    {
        let pu = playlists_ui.clone();
        playlists.on_row_name(move |id| {
            pu.grid_stats_by_id(i64::from(id))
                .map(|p| SharedString::from(p.name.as_str()))
                .unwrap_or_default()
        });
    }
    {
        let pu = playlists_ui.clone();
        playlists.on_row_description(move |id| {
            pu.grid_stats_by_id(i64::from(id))
                .and_then(|p| p.description)
                .map(SharedString::from)
                .unwrap_or_default()
        });
    }
}
