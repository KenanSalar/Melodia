//! `Genres.*` grid callbacks, wired as `albums/callbacks/grid.rs` argues less the covers.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::card_grid;
use crate::ui::genres::grid::GenresGrid;
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, GenreDetail, Genres};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    genres_ui: &Arc<GenresUi>,
) {
    let genres = ui.global::<Genres>();
    let weak = ui.as_weak();

    if let Some(sort) = persisted_sort(view_state, view_id::GENRES) {
        genres.set_sort_field(SharedString::from(sort.field.as_str()));
        genres.set_sort_dir(SharedString::from(sort.dir.as_str()));
    }

    {
        let weak = weak.clone();
        genres.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            card_grid::set_columns::<GenresGrid>(&ui, cols);
        });
    }

    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            if gu.section_active() {
                card_grid::requery::<GenresGrid>(&ui, &s, &gu);
            }
        });
    }

    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Genres>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            if gu.section_active() {
                card_grid::requery::<GenresGrid>(&ui, &s, &gu);
            }
            persist_view_sort(&s, view_id::GENRES, new_field, new_dir);
        });
    }

    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_open_genre(move |genre_id| {
            let id = i64::from(genre_id);

            if let Some(ui) = weak.upgrade() {
                ui.global::<GenreDetail>().set_origin_nav_index(-1);
            }

            let s_fetch = s.clone();
            let gu_fetch = gu.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(
                s_fetch,
                "genres::open_genre",
                genres_ui_mod::open_genre(
                    &s_fetch,
                    &gu_fetch,
                    weak_fetch,
                    id,
                    melodia_ui::NavEnterFrom::Right
                )
            );

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk.paths,
                    view_id::GENRE_DETAIL,
                    Some(id),
                ) {
                    log::warn!("genres::open_genre persist: {}", describe(&e));
                }
            });
        });
    }
}
