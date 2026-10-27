//! The grid cards' actions and the tab switch.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{cross_tab_nav, next_sort, persist_view_sort};
use crate::ui::favorites::NAV_FAVORITES;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesTab, FavoritesUi};
use crate::ui::paged_rows::track_rows;
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Favorites};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    wire_play_track(ui, state);
    wire_open_artist(ui, state, artists_ui);
    wire_tab_changed(ui, state, fav_ui);
    wire_artist_sort(ui, state, fav_ui);
    wire_columns_changed(ui);
}

/// A Most Played card queues that grid rather than the Songs list, starting on the card. A card
/// carries no row index, so the start comes from the id.
fn wire_play_track(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_play_track(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let id = i64::from(id);
        let ids = favorites_ui_mod::most_played_ids(&ui);
        if ids.is_empty() {
            return;
        }
        let start = ids.iter().position(|&i| i == id);
        let s = s.clone();
        spawn_logged!(
            s,
            "favorites::play_track",
            library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
        );
    });
}

/// Drills into Artist Detail, handing the grid's covers back as Nav unmounts it.
fn wire_open_artist(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let s = state.clone();
    let aru = artists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_open_artist(move |artist_id| {
        s.runtime.spawn_blocking(crate::ui::grid_prewarm::hand_back_covers);
        cross_tab_nav::open_artist_cross_tab(
            &s,
            &aru,
            &weak,
            i64::from(artist_id),
            cross_tab_nav::Origin::section(NAV_FAVORITES),
            "favorites::open_artist",
        );
    });
}

/// Catches the Rust side up once the bar has moved `tab-idx` and cleared the Slint-side filter.
/// Every tab keeps its list across a pick, so nothing is rewound and a reload the entering tab
/// owes lands behind what it shows.
fn wire_tab_changed(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
    g.on_tab_changed(move |tab| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let entering = favorites_ui_mod::tab_from_index(&g, tab);
        persist.publish(tab);
        // A pick moves no nav index, so `record_current` never logs it.
        crate::ui::view_tag::log_current(&ui);

        // Before the shadow moves, so it asks about the tab being left, whose needle the pick
        // clears.
        if fu.shows_filtered() {
            fu.mark_tab_dirty(fu.active_tab());
        }

        // Shadow first, every load below choosing its list and covers from it.
        fu.set_active_tab(entering);
        favorites_ui_mod::set_filter(&fu, "");
        crate::ui::hero_chips::publish_favorites(&ui, &fu);

        if fu.take_tab_dirty(entering) {
            reload_tab(&ui, &s, &fu, entering);
        }

        let fu_covers = fu.clone();
        let s_disk = s.clone();
        let persist_disk = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            // Only the write is superseded; the prewarm runs regardless.
            persist_disk.write_if_current(tab, || {
                if let Err(e) = library::settings::set_favorites_tab(&s_disk.paths, tab) {
                    log::warn!("favorites::set_favorites_tab: {}", describe(&e));
                }
            });
            favorites_ui_mod::prewarm_tab_covers(&fu_covers, entering);
        });
    });
}

fn reload_tab(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>, tab: FavoritesTab) {
    if tab != FavoritesTab::Songs {
        favorites_ui_mod::reload_grid(ui, state, fav_ui, tab);
        return;
    }
    let s = state.clone();
    let fu = fav_ui.clone();
    spawn_logged!(s, "favorites::reload_songs", track_rows::reload(&s, &fu));
}

/// A sort pill: the ids reload in the new order, every cached card kept.
fn wire_artist_sort(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_request_artist_sort(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let (new_field, new_dir) =
            next_sort(g.get_artist_sort_field().as_str(), g.get_artist_sort_dir().as_str(), &field);
        g.set_artist_sort_field(SharedString::from(new_field.as_str()));
        g.set_artist_sort_dir(SharedString::from(new_dir.as_str()));
        if fu.section_active() {
            favorites_ui_mod::requery_grid(&ui, &s, &fu);
        }
        persist_view_sort(&s, view_id::FAVORITE_ARTISTS, new_field, new_dir);
    });
}

/// A resize only changes how the ids are chunked, so the cached cards stay.
fn wire_columns_changed(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_columns_changed(move |cols| {
        let Some(ui) = weak.upgrade() else { return };
        favorites_ui_mod::set_columns(&ui, cols);
    });
}
