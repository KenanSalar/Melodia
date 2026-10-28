//! The page's own `MyLibrary.*` handlers. What a tab does on entry and exit stays in its view's
//! lifecycle, driven by its own `SectionActiveGate`.
//!
//! **The hero is the page's, not a tab's.** A tab switch clears no detail id, so the banner is
//! what the band collapses out of and morphs back into on the next pick. Handing it back splits
//! between `hero-collapsed` (per id, once the morph ends) and `page-active-changed` (all of it,
//! once the page is gone).

use std::cell::Cell;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::detail_view::release_hero_slots;
use crate::ui::my_library as my_library_mod;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AlbumDetail, AppWindow, ArtistDetail, MyLibrary, PlaylistDetail};

/// Writes the active tab to `views.json`. One `persist` covers both callers, a pick and
/// `nav_history::replay`'s moves writing the one field.
fn persist_tab(state: &AppState, persist: &Arc<IndexPersist>, tab: i32) {
    persist.publish(tab);
    let s = state.clone();
    let persist = Arc::clone(persist);
    state.runtime.spawn_blocking(move || {
        persist.write_if_current(tab, || {
            if let Err(e) = library::settings::set_my_library_tab(&s.paths, tab) {
                log::warn!("my_library: set_my_library_tab({tab}): {}", describe(&e));
            }
        });
    });
}

/// Hands back what the band's hero held once its collapse has finished, rather than at the
/// `close-detail` that started it: every hero fact is a ternary over the detail id, so an earlier
/// teardown collapses a placeholder over a reset gradient.
///
/// Only the globals whose id cleared: a tab switch out of an open detail collapses the band too,
/// and picking that tab again morphs the same banner back open. The colour set is
/// [`release_page_hero`]'s for that reason. Safe unguarded, `LibraryTabBand` cancelling its timer
/// on a re-open.
fn release_collapsed_hero(ui: &AppWindow) {
    let album = ui.global::<AlbumDetail>();
    if album.get_album_id() < 0 {
        release_hero_slots(&album);
    }
    let artist = ui.global::<ArtistDetail>();
    if artist.get_artist_id() < 0 {
        release_hero_slots(&artist);
    }
    let playlist = ui.global::<PlaylistDetail>();
    if playlist.get_playlist_id() < 0 {
        release_hero_slots(&playlist);
    }
    crate::ui::hero_chips::clear_if_stale(ui);
}

/// Hands back every hero global the page owns once the page is left, unconditionally: past this,
/// no id brings a banner back without a fetch. It is the only reach a detail held on another tab
/// has, the per-tab gates firing for the mounted tab alone; `globals/my-library.slint` says why a
/// sixth gate can't deliver it either.
fn release_page_hero(ui: &AppWindow) {
    release_hero_slots(&ui.global::<AlbumDetail>());
    release_hero_slots(&ui.global::<ArtistDetail>());
    release_hero_slots(&ui.global::<PlaylistDetail>());
    crate::ui::hero_backdrop::reset(ui);
    crate::ui::hero_chips::clear(ui);
}

pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<MyLibrary>();
    let weak = ui.as_weak();
    let tab_persist = Arc::new(IndexPersist::new(g.get_tab_idx()));

    // Fires once the bar has moved `tab-idx` and the entering tab's gate has re-fetched. Clears
    // both sides of the filter, the box and the entering tab's own needle, and records history,
    // a pick being the one tab move that is the user's own.
    {
        let s = state.clone();
        let weak = weak.clone();
        let persist = Arc::clone(&tab_persist);
        g.on_tab_changed(move |tab| {
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<MyLibrary>();
                g.set_filter(SharedString::default());
                g.set_blur_search_tick(g.get_blur_search_tick() + 1);
                my_library_mod::filter::clear_mounted(&ui);
                crate::ui::nav_history::record_current(&ui);
            }
            persist_tab(&s, &persist, tab);
        });
    }

    // The same write without the filter clear, for a drill or a Mouse-4/5 walk moving the tab
    // on the user's behalf.
    {
        let s = state.clone();
        g.on_persist_tab_idx(move |tab| persist_tab(&s, &tab_persist, tab));
    }

    {
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            my_library_mod::filter::dispatch(&ui, text.as_str());
        });
    }

    {
        let weak = weak.clone();
        g.on_detail_scope_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            my_library_mod::filter::sync_box(&ui);
        });
    }

    // The band's arrow goes through the mounted tab's own `close-detail`, so its teardown stays
    // where it is.
    {
        let weak = weak.clone();
        g.on_back(move || {
            let Some(ui) = weak.upgrade() else { return };
            let tab = {
                let g = ui.global::<MyLibrary>();
                my_library_mod::tab_from_index(&g, g.get_tab_idx())
            };
            my_library_mod::close_open_detail(&ui, tab);
        });
    }

    {
        let weak = weak.clone();
        g.on_hero_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            release_collapsed_hero(&ui);
        });
    }

    // The index is re-read rather than trusted from the argument, covering the band not being
    // leaving it. Latched, because the mirror fires on every nav change and can't say which
    // index it left: unlatched, Favorites → Recently Played would clear the hero globals those
    // bands publish into. Seeded from the same question, `install_views` hydrating the nav index
    // first.
    {
        let weak = weak.clone();
        let was_up = Cell::new(my_library_mod::the_band_is_up(ui));
        g.on_page_active_changed(move |_active| {
            let Some(ui) = weak.upgrade() else { return };
            let up = my_library_mod::the_band_is_up(&ui);
            if was_up.replace(up) && !up {
                release_page_hero(&ui);
            }
        });
    }
}
