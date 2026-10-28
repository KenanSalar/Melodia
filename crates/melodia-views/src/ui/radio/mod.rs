//! The Radio page: three tabs over the radio-browser.info directory, the stations the user kept,
//! and the ones they played.
//!
//! **One section handle and one `SectionActiveGate` for all three**, so a tab flip stays inside
//! the page: a per-tab gate would hand back a directory answer bought with a network round trip
//! every time the user glanced at their favorites.
//!
//! **Station history is a tab rather than a sort of the kept list**: playing a station out of
//! Browse keeps it without starring it, and a favorites-only list could never reach that row.
//!
//! **The section leave drops the logo tier and the seated heroes and keeps every list**, so it
//! rewinds no count and its dirty flag describes the hero alone (`callbacks::lifecycle`). It is
//! also `tasks::radio_logo_cache`'s only trigger: the browsed-logo store grows only while this page
//! is open, and the artwork sweep's own trigger, a scan, may not run for weeks.

mod browse;
mod callbacks;
mod covers;
mod detail;
mod facets;
mod filter;
mod history;
mod identity;
mod kept;
mod logos;
mod rows;
mod state;
mod suggest;
mod tabs;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ui::view_ctx::ViewCtx;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Nav, Radio, RadioFacetRow, RadioStationGridRow, RadioSuggestionRow};

use tabs::section_is_up;

pub use callbacks::files::wire as wire_files;
pub use covers::tune_cache_for_display;
pub use detail::{StationRef, open_station_with, seed_detail_from_settings};
pub use history::install as install_history;
pub use identity::{StationTile, station_tile};
pub use rows::{display_tags, station_has_row};
// Re-exported so every sibling writes `super::RadioUi`, the handle being the page.
pub use state::RadioUi;
pub use tabs::{RadioTab, mounted_tab, seed_tab, tab_from_index};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_RADIO: i32 = 10;

/// Maps a persisted nav index onto a live one while Radio is off, so a boot doesn't select a
/// router branch that is gated away. A sibling of
/// [`crate::ui::my_library::fold_retired_nav_index`] rather than an arm in it: a retired index is
/// a fact about the file, where this one flips with a setting.
pub fn fold_disabled_nav_index(idx: i32, radio_enabled: bool) -> i32 {
    if idx == NAV_RADIO && !radio_enabled { crate::ui::my_library::NAV_MY_LIBRARY } else { idx }
}

/// Everything switching Radio off has to undo past the row and the router branch, here so the
/// page owns its teardown: a history walk onto a page that no longer routes, a selected index with
/// no branch, and a tooltip naming a row that is gone.
pub fn disable(ui: &AppWindow, state: &AppState) {
    if let Err(e) = melodia_app::library::playback::player_stop_station(&state.playback_ctx()) {
        log::warn!("radio: stop station on disable: {}", describe(&e));
    }
    crate::ui::nav_history::nav().history().forget_section(NAV_RADIO);

    // Through the callback, so the persisted id and the hero's images go their usual way; a
    // station left in `views.json` would reopen when the switch went back on.
    let radio = ui.global::<Radio>();
    if radio.get_detail_open() {
        radio.invoke_close_detail();
    }
    // The other tabs' pages never reached `detail-open` and only need forgetting, the one name
    // `views.json` carries being the mounted tab's.
    if let Some(radio_ui) = crate::ui::nav_history::nav().handles().radio.lock().clone() {
        state.runtime.spawn_blocking(move || {
            detail::forget_all_seats(&radio_ui);
            radio_ui.release_detail_artwork();
        });
    }

    let nav = ui.global::<Nav>();
    // Through the boot path's own fold, so the two can't disagree about where 10 goes. The
    // placeholder fall-through excludes 10, so anything landing there paints an empty panel.
    let selected = nav.get_selected_index();
    let landed = fold_disabled_nav_index(selected, false);
    if landed != selected {
        nav.set_selected_index(landed);
        nav.invoke_persist_selected_index(landed);
    }

    // A row unmounted under the pointer leaves its tooltip up with nothing to retract it, the
    // backstop `changed watched-mini-render` owes the rail too.
    if nav.get_sidebar_tip_idx() == NAV_RADIO {
        nav.set_sidebar_tip_idx(-1);
    }
}

/// Drops the cached facet lists so the chips rebuild against a setting that just changed; parking
/// `facet-shown` stops the picker reusing the list it has in hand.
pub fn forget_facets(ui: &AppWindow) {
    if let Some(radio_ui) = crate::ui::nav_history::nav().handles().radio.lock().clone() {
        *radio_ui.facet_index.lock() = facets::FacetIndex::default();
        radio_ui.facet_list.lock().take();
    }
    ui.global::<Radio>().set_facet_shown(-1);
}

/// Wires every `Radio.*` callback and returns the handle, for the cover retune alone: the wired
/// closures are what keep it alive.
pub fn install(cx: ViewCtx<'_>) -> Arc<RadioUi> {
    install_models(cx.app);
    let radio_ui =
        Arc::new(RadioUi::new(section_is_up(cx.app), crate::ui::detail_artwork::blur_spec(cx.app)));
    // A boot landing elsewhere owes the flag its own leave never set: a station page restored
    // while Radio is off screen publishes into neither shared global, and the first enter must.
    if !radio_ui.section_active() {
        radio_ui.mark_dirty();
    }
    // A landed decode changes nothing the card binding read, so the generation re-runs it.
    crate::ui::cover_generation::notify_on_decode(&radio_ui.covers, cx.app, covers::repaint);
    callbacks::wire(cx.app, cx.state, cx.view_state, &radio_ui);
    radio_ui
}

/// Hands the global its empty `VecModel`s, which every later write downcasts back to: an array
/// left on its declared default makes `replace_rows` miss, and the grid stays empty.
fn install_models(ui: &AppWindow) {
    let g = ui.global::<Radio>();

    let browsed: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_browse_rows(ModelRc::from(browsed));

    let favorites: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_favorites_rows(ModelRc::from(favorites));

    let recent: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_recent_rows(ModelRc::from(recent));

    let facets: Rc<VecModel<RadioFacetRow>> = Rc::new(VecModel::default());
    g.set_facet_options(ModelRc::from(facets));

    let suggestions: Rc<VecModel<RadioSuggestionRow>> = Rc::new(VecModel::default());
    g.set_suggestions(ModelRc::from(suggestions));
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
