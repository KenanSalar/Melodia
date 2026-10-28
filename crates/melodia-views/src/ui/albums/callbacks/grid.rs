//! `Albums.*` grid callbacks.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::albums::grid::AlbumsGrid;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::card_grid;
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AlbumDetail, Albums, AppWindow};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    albums_ui: &Arc<AlbumsUi>,
) {
    let albums = ui.global::<Albums>();
    let weak = ui.as_weak();

    // Ahead of the first load, so it runs in the remembered order.
    if let Some(sort) = persisted_sort(view_state, view_id::ALBUMS) {
        albums.set_sort_field(SharedString::from(sort.field.as_str()));
        albums.set_sort_dir(SharedString::from(sort.dir.as_str()));
    }

    // `generation` is read for its effect on the binding, never its value; the notifier that
    // moves it is `boot::ui_setup::views::install_grid_covers`, one for the shared tier.
    albums.on_request_cover(|path, _generation| crate::ui::grid_prewarm::grid_cover(path.as_str()));

    {
        let weak = weak.clone();
        albums.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            card_grid::set_columns::<AlbumsGrid>(&ui, cols);
        });
    }

    // A hidden grid reloads under the filter on its enter instead.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            if au.section_active() {
                card_grid::requery::<AlbumsGrid>(&ui, &s, &au);
            }
        });
    }

    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Albums>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            if au.section_active() {
                card_grid::requery::<AlbumsGrid>(&ui, &s, &au);
            }
            persist_view_sort(&s, view_id::ALBUMS, new_field, new_dir);
        });
    }

    // The grid unmounts under the detail, so its covers go back as proxies until the close
    // re-warms them. The persisted id is what a restart on this tab reopens.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_open_album(move |album_id| {
            let id = i64::from(album_id);

            // A "Go to Album" from Favorites stamps an origin only `close-detail` clears, so
            // reaching this grid past that detail would send the next back press to Favorites.
            if let Some(ui) = weak.upgrade() {
                ui.global::<AlbumDetail>().set_origin_nav_index(-1);
            }

            let s_fetch = s.clone();
            let au_fetch = au.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(
                s_fetch,
                "albums::open_album",
                albums_ui_mod::open_album(
                    &s_fetch,
                    &au_fetch,
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
                    crate::ui::track_list_view::view_id::ALBUM_DETAIL,
                    Some(id),
                ) {
                    log::warn!("albums::open_album persist: {}", describe(&e));
                }
            });
        });
    }
}
