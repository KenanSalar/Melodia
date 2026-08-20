//! The Radio page — two tabs (Browse, Favorites) over the radio-browser.info directory and the
//! stations the user kept.
//!
//! **One section handle for both tabs**, and one `SectionActiveGate` to match. My Library's
//! per-tab gates are what its history left it — five tabs that used to be five sidebar sections,
//! each with a hook of its own — where here a tab flip has to stay inside the page: Browse holds a
//! directory answer bought with a network round trip, and a per-tab gate would hand it back every
//! time the user glanced at their favorites.
//!
//! **The section leave is narrower than every other grid page's**, for the same reason: it drops
//! the logo tier and keeps the results. See [`browse`] and [`covers`].

mod browse;
mod callbacks;
mod covers;
mod facets;
mod filter;
mod logos;
mod rows;
mod tabs;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::media::cover_thumbs::CoverThumbs;
use crate::state::AppState;
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;
use crate::{AppWindow, Nav, Radio, RadioFacetRow, RadioStationGridRow};

use browse::BrowseState;
use logos::LogoMemo;
use tabs::section_is_up;

pub use covers::tune_cache_for_display;
pub use tabs::{RadioTab, seed_tab, tab_from_index};

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

/// Wire every `Radio.*` callback and hand back the page's handle.
///
/// Returned for the cover retune alone: every wired closure clones its own strong `Arc`, so the
/// wiring is what keeps the handle alive.
pub fn install(cx: ViewCtx<'_>) -> Arc<RadioUi> {
    install_models(cx.app);
    let radio_ui = Arc::new(RadioUi::new(section_is_up(cx.app)));
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

    let stations: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_browse_rows(ModelRc::from(stations));

    let facets: Rc<VecModel<RadioFacetRow>> = Rc::new(VecModel::default());
    g.set_facet_options(ModelRc::from(facets));
}

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen. **Seeded at wire time rather than left to the gate**, which
    /// fires on transitions only and whose `ChangeTracker` baselines silently inside
    /// `AppWindow::new()` — a section seeded wrong has no edge left to correct it.
    ///
    /// No dirty flag rides with it. What a leave hands back here is the logo tier and nothing
    /// else, and the enter re-warms unconditionally, so there is no state for a flag to describe.
    section: SectionState,
    /// The directory page on screen, and the query it answers.
    browse: Mutex<BrowseState>,
    /// The directory uuids this install has starred. Refreshed on section enter and flipped
    /// optimistically by the toggle, which is what lets the star respond on the click's own frame.
    favorites: Mutex<HashSet<String>>,
    /// What this session knows about station logos, keyed on the URL they came from.
    logos: LogoMemo,
    /// The open picker's list, whole. Kept beside the Slint model because the picker's needle
    /// narrows it and Slint cannot filter an array, so every keystroke rebuilds the model from
    /// here rather than re-asking the facade across the runtime.
    facet_list: Mutex<Option<Arc<[crate::entities::radio::Facet]>>>,
    /// The grid tier the cards decode into. Released by the section leave.
    covers: Arc<CoverThumbs>,
}

impl RadioUi {
    fn new(section_active: bool) -> Self {
        let section = SectionState::new();
        section.set_active(section_active);
        Self {
            section,
            browse: Mutex::new(BrowseState::default()),
            favorites: Mutex::new(HashSet::new()),
            logos: LogoMemo::new(),
            facet_list: Mutex::new(None),
            covers: Arc::new(covers::new_tier()),
        }
    }

    /// Whether the page is the section on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Flip a station's star in the shadow the grid is built from.
    ///
    /// The optimistic half of the toggle, and its revert: the write is a round trip through
    /// `SQLite` and the star has to answer on the click's own frame.
    fn set_local_favorite(&self, station_uuid: &str, favorite: bool) {
        let mut favorites = self.favorites.lock();
        if favorite {
            favorites.insert(station_uuid.to_owned());
        } else {
            favorites.remove(station_uuid);
        }
    }
}
