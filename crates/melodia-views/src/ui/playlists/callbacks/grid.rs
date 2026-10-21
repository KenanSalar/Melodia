//! `Playlists.*` grid callbacks: lazy cover lookup, re-chunk on column
//! count, the filter, the open-playlist drill-in, and the per-id name /
//! description lookups backing the grid card's Rename overlay.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::card_grid;
use crate::ui::playlists::grid::PlaylistsGrid;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Playlists};

/// Wire the `Playlists` grid callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // request-cover: lazy per-card cover lookup off the shared grid tier. Only on-screen cards
    // invoke this, and `generation` is read for its effect on the binding, never its value.
    playlists
        .on_request_cover(|path, _generation| crate::ui::grid_prewarm::grid_cover(path.as_str()));

    {
        let weak = weak.clone();
        playlists.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            card_grid::set_columns::<PlaylistsGrid>(&ui, cols);
        });
    }

    // A hidden grid reloads under the filter on its enter instead.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        playlists.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            if pu.section_active() {
                card_grid::requery::<PlaylistsGrid>(&ui, &s, &pu);
            }
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
            spawn_logged!(
                s_fetch,
                "playlists::open_playlist",
                playlists_ui_mod::open_playlist(
                    &s_fetch,
                    &pu_fetch,
                    weak_fetch,
                    id,
                    melodia_ui::NavEnterFrom::Right
                )
            );

            s.runtime.spawn_blocking(crate::ui::grid_prewarm::hand_back_covers);

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk.paths,
                    crate::ui::track_list_view::view_id::PLAYLIST_DETAIL,
                    Some(id),
                ) {
                    log::warn!("playlists::open_playlist persist: {}", describe(&e));
                }
            });
        });
    }

    // row-name / row-description: per-id lookups against the card on screen —
    // back the grid card's "Rename" overlay, which only carries the playlist
    // id. A card whose fetch hasn't landed reads as empty.
    {
        let weak = weak.clone();
        playlists.on_row_name(move |id| {
            let Some(ui) = weak.upgrade() else { return SharedString::default() };
            playlists_ui_mod::shown_card(&ui, i64::from(id)).map(|p| p.name).unwrap_or_default()
        });
    }
    playlists.on_row_description(move |id| {
        let Some(ui) = weak.upgrade() else { return SharedString::default() };
        playlists_ui_mod::shown_card(&ui, i64::from(id)).map(|p| p.description).unwrap_or_default()
    });
}
