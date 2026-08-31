//! The Radio page — three tabs over the radio-browser.info directory, the stations the user kept,
//! and the ones they played.
//!
//! **One section handle for all three**, and one `SectionActiveGate` to match. My Library's
//! per-tab gates are what its history left it — five tabs that used to be five sidebar sections,
//! each with a hook of its own — where here a tab flip has to stay inside the page: Browse holds a
//! directory answer bought with a network round trip, and a per-tab gate would hand it back every
//! time the user glanced at their favorites.
//!
//! **Station history is a tab rather than a sort of the kept list**, and the reason is which rows
//! it can reach: playing a station out of Browse keeps it without starring it, so a favorites-only
//! list is the one place such a row could never be found, re-starred or deleted from.
//!
//! **The section leave is narrower than every other grid page's**: it drops the logo tier and
//! keeps every list. See [`browse`], [`kept`] and [`covers`].
//!
//! It is also the *only* trigger for `tasks::radio_logo_cache`, which is the one piece of teardown
//! here that reaches past the UI. The browsed-logo store grows only while this page is open, so a
//! leave is exactly when it stops, and the artwork sweep's own trigger is a library scan — which a
//! user who browses radio and never touches their music folders may not run for weeks.

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

use crate::state::AppState;
use crate::ui::view_ctx::ViewCtx;
use crate::{AppWindow, Nav, Radio, RadioFacetRow, RadioStationGridRow, RadioSuggestionRow};

use tabs::section_is_up;

pub use callbacks::files::wire as wire_files;
pub use covers::tune_cache_for_display;
pub use detail::{StationRef, open_station_with, seed_detail_from_settings};
pub use history::install as install_history;
pub use identity::{StationTile, station_tile};
pub use rows::{display_tags, station_has_row};
// Re-exported so every sibling still writes `super::RadioUi`: the handle is the page, and which
// file declares it is nobody else's business.
pub use state::RadioUi;
pub use tabs::{RadioTab, mounted_tab, seed_tab, tab_from_index};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_RADIO: i32 = 10;

/// Map a persisted `views.json` nav index onto a live one when Radio is switched off.
///
/// A **sibling** of [`crate::ui::my_library::fold_retired_nav_index`] rather than an arm
/// inside it: that one answers "this index was retired", which is true of a file forever,
/// where this one answers "this index is unreachable in this install" and flips with a
/// setting. Left unfolded, a boot with the switch off selects a router branch that is
/// gated away and paints nothing.
pub fn fold_disabled_nav_index(idx: i32, radio_enabled: bool) -> i32 {
    if idx == NAV_RADIO && !radio_enabled {
        crate::ui::my_library::NAV_MY_LIBRARY
    } else {
        idx
    }
}

/// Everything switching Radio off has to undo, past the row and the router branch that
/// simply stop being mounted.
///
/// Stated here rather than in the settings callback so the page owns its own teardown,
/// and because three of the four are only findable from this side: a walk back onto a page
/// that no longer routes, a selected index the router no longer has a branch for, and a
/// tooltip left naming a row that no longer exists.
pub fn disable(ui: &AppWindow, state: &AppState) {
    if let Err(e) = crate::library::playback::player_stop_station(&state.playback_ctx()) {
        log::warn!("radio: stop station on disable: {e}");
    }
    state.nav_history.lock().forget_section(NAV_RADIO);

    // Through the callback rather than by clearing the flag, so the persisted id and the hero's
    // images go the one way they ever go. It is the whole page that is being taken away; leaving
    // a station named in `views.json` would reopen it the next time the switch went back on.
    let radio = ui.global::<Radio>();
    if radio.get_detail_open() {
        radio.invoke_close_detail();
    }
    // The pages waiting on the other two tabs never reached `detail-open`, so nothing above can
    // see them. Nothing is owed but forgetting them: the page they belong to is gone, and the
    // one name `views.json` carries is the mounted tab's, which the close above already cleared.
    if let Some(radio_ui) = state.ui_handles.radio.lock().clone() {
        state.runtime.spawn_blocking(move || {
            detail::forget_all_seats(&radio_ui);
            radio_ui.release_detail_artwork();
        });
    }

    let nav = ui.global::<Nav>();
    // Through the boot path's own fold, so the two can't disagree about where 10 goes.
    // Settings being the only way to the switch keeps this quiet today, but the placeholder
    // fall-through excludes 10 deliberately: anything landing there with the switch down
    // paints an empty panel, and `origin-nav-index` is one drill away from being able to.
    let selected = nav.get_selected_index();
    let landed = fold_disabled_nav_index(selected, false);
    if landed != selected {
        nav.set_selected_index(landed);
        nav.invoke_persist_selected_index(landed);
    }

    // `SidebarItem` publishes its identity into the rail's tooltip channel and clears it on
    // hover-exit, so a row unmounted under the pointer leaves the pill up with nothing left to
    // retract it. Same backstop `changed watched-mini-render` already owes the rail.
    if nav.get_sidebar_tip_idx() == NAV_RADIO {
        nav.set_sidebar_tip_idx(-1);
    }
}

/// Drop the cached facet lists, so the chips rebuild against a setting that just changed.
///
/// Nothing else re-asks inside one session: the section's prime skips a list already held, and the
/// picker skips the fetch for a chip whose list is still in hand. Parking `facet-shown` is what
/// takes away that second shortcut.
pub fn forget_facets(ui: &AppWindow, state: &AppState) {
    if let Some(radio_ui) = state.ui_handles.radio.lock().clone() {
        *radio_ui.facet_index.lock() = facets::FacetIndex::default();
        radio_ui.facet_list.lock().take();
    }
    ui.global::<Radio>().set_facet_shown(-1);
}

/// Wire every `Radio.*` callback and hand back the page's handle.
///
/// Returned for the cover retune alone: every wired closure clones its own strong `Arc`, so the
/// wiring is what keeps the handle alive.
pub fn install(cx: ViewCtx<'_>) -> Arc<RadioUi> {
    install_models(cx.app);
    let radio_ui =
        Arc::new(RadioUi::new(section_is_up(cx.app), crate::ui::detail_artwork::blur_spec(cx.app)));
    // **A boot landing on another section owes the flag its own leave never set.**
    // `SectionState` starts clean so the boot pre-fetch wins the first enter, but a station page
    // restored from `views.json` while Radio is off screen publishes into neither shared global —
    // and with nothing marked, the first enter would consume no flag and never republish, leaving
    // the band on a hero with no chips and no backdrop. The four detail sections seed it the same
    // way.
    if !radio_ui.section_active() {
        radio_ui.mark_dirty();
    }
    // A scheduled logo decode landing has nothing the card binding read to change, so the tier
    // signals and the generation is what re-runs it. Never off `0`: a batch landing after a
    // leave cleared the tier would otherwise read as warm.
    crate::ui::cover_generation::notify_on_decode(&radio_ui.covers, cx.app, covers::repaint);
    callbacks::wire(cx.app, cx.state, &radio_ui);
    radio_ui
}

/// Hand the global its empty `VecModel`s. Every later write finds them by downcasting back, so a
/// property left on the declared default is a silent no-op: an unbound array is a model of its own
/// kind, `write_grid`'s downcast misses, and the grid stays empty with one warning per attempt.
fn install_models(ui: &AppWindow) {
    let g = ui.global::<Radio>();

    // One per grid — the three tabs draw the same card but each keeps its own rows.
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
