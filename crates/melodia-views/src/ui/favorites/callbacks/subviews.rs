//! `Favorites.*` card actions and sub-view routing: Most Played
//! play-track, the cross-tab open-artist hand-off, the tab switch, and the
//! grid's column-count push. See [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{cross_tab_nav, next_sort, persist_view_sort};
use crate::ui::favorites::NAV_FAVORITES;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesTab, FavoritesUi};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Favorites};

/// Wire the card-action and sub-view-routing callbacks.
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

/// Clicking a Most Played card loads that grid into the queue
/// and starts on that card — the grid is the context, not the Songs list
/// on the other tab. The Slint callback carries no row index (these are
/// cards, not list rows), so the start slot comes from the id.
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

/// Clicking a favorite artist card drills into the Artists tab's
/// Artist Detail; the shared hand-off stamps the origin so the back
/// arrow returns to Favorites. The grid is unmounted the moment Nav
/// flips, so its cover tier goes with it — the same release
/// `on_open_album` does for the Albums grid.
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

/// The bar has already moved `tab-idx` and cleared the Slint-side
/// filter by the time this runs, so this is the catch-up: drop the Rust
/// filter shadow to match, reload the entering tab if it missed a tick, then
/// off-thread persist the pick and warm the entering grid's covers.
///
/// Every tab keeps its list across a pick, so the entering one paints what it
/// showed last on this tick and a reload it owes refreshes behind it. Its count
/// is still the one that list was shown with, so nothing is rewound.
fn wire_tab_changed(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    // Ordered: a bounce queues a value per pick and two blocking tasks have
    // none of their own, so a reversed pair reopens the page on a tab the
    // user only passed through.
    let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
    g.on_tab_changed(move |tab| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let entering = favorites_ui_mod::tab_from_index(&g, tab);
        persist.publish(tab);
        // A sub-view pick moves no nav index, so `record_current` never
        // hears about it. The bar has already written `tab-idx`, so the tag
        // reads the tab being entered.
        crate::ui::view_tag::log_current(&ui);

        // Asked before the shadow moves, so it reads the tab being left: the
        // pick is about to clear the needle that tab was loaded under, and the
        // pick that brings it back has to reload it.
        if fu.shows_filtered() {
            fu.mark_tab_dirty(fu.active_tab());
        }

        // Shadow first: every later load decides which list to fill and which
        // covers to warm from it, and both can run off the UI thread where the
        // global isn't reachable.
        fu.set_active_tab(entering);
        favorites_ui_mod::set_filter(&fu, "");
        crate::ui::hero_chips::publish_favorites(&ui, &fu);

        // Each tab's load only runs while that tab is mounted, so entering one
        // is where a tick skipped on another gets paid for; the flag is what
        // keeps a pick back and forth from reloading a list nothing has
        // invalidated. See `lifecycle::kick_full_refresh`.
        if fu.take_tab_dirty(entering) {
            reload_tab(&ui, &s, &fu, entering);
        }

        let fu_covers = fu.clone();
        let s_disk = s.clone();
        let persist_disk = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            // Scoped to the write alone: a superseded tab drops its own disk
            // hop, and the cover swap below is this task's regardless.
            persist_disk.write_if_current(tab, || {
                if let Err(e) = library::settings::set_favorites_tab(&s_disk.paths, tab) {
                    log::warn!("favorites::set_favorites_tab: {}", describe(&e));
                }
            });
            // Whatever the entering grid painted last visit is still in the tier
            // as a proxy; this sharpens it, and anything genuinely absent
            // schedules itself off the card's own lookup.
            favorites_ui_mod::prewarm_tab_covers(&fu_covers, entering);
        });
    });
}

/// The entering tab's reload, behind the list it kept from its last visit. UI thread.
fn reload_tab(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>, tab: FavoritesTab) {
    if tab != FavoritesTab::Songs {
        favorites_ui_mod::reload_grid(ui, state, fav_ui, tab);
        return;
    }
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        if let Err(e) = favorites_ui_mod::refresh_tracks(&s, &fu, &weak).await {
            log::warn!("favorites::refresh_tracks: {}", describe(&e));
        }
    });
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
