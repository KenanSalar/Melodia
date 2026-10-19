//! `Genres.*` grid callbacks: re-chunk on column count, the filter and the
//! sort, and the open-genre drill-in. Genres have no covers, so (unlike
//! `albums/grid.rs`) there's no `request-cover` handler or cover prewarm.

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

/// Wire the `Genres` grid callbacks. See [`super::wire`].
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    genres_ui: &Arc<GenresUi>,
) {
    let genres = ui.global::<Genres>();
    let weak = ui.as_weak();

    // Seed the grid's sort pill from the persisted `view_sort["genres"]`.
    if let Some(sort) = persisted_sort(view_state, view_id::GENRES) {
        genres.set_sort_field(SharedString::from(sort.field.as_str()));
        genres.set_sort_dir(SharedString::from(sort.dir.as_str()));
    }

    // columns-changed: the view recomputed its integer column count and
    // already wrote `Genres.columns`. Re-chunk the ids, keeping every cached card.
    {
        let weak = weak.clone();
        genres.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            card_grid::set_columns::<GenresGrid>(&ui, cols);
        });
    }

    // apply-filter: `Genres.filter` is already updated via the SearchBar's
    // two-way binding, so the ids reload under it. A hidden grid reloads on
    // its enter instead.
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

    // request-sort: clicking a sort pill. Same field flips dir; a new
    // field resets to ascending. The ids reload in the new order.
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

    // open-genre: a card click. Fetches the detail header + track
    // list and flips `GenreDetail.genre-id >= 0`, swapping the grid
    // for the detail. Also stamps the Genres entry in
    // `views.json`'s `last_detail_ids` so a restart on the Genres tab
    // reopens this same detail page.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_open_genre(move |genre_id| {
            let id = i64::from(genre_id);

            // Same-tab open: defensively zero any stale cross-section origin —
            // see `albums::grid`'s copy for the path it guards against.
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
