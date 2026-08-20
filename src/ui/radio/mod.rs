//! The Radio page — two tabs (Browse, Favorites) over the radio-browser.info directory and the
//! stations the user kept.
//!
//! **One section handle for both tabs**, and one `SectionActiveGate` to match. My Library's
//! per-tab gates are what its history left it — five tabs that used to be five sidebar sections,
//! each with a hook of its own — where here a tab flip has to stay inside the page: Browse holds a
//! directory answer bought with a network round trip, and a per-tab gate would hand it back every
//! time the user glanced at their favorites.

mod callbacks;
mod tabs;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;
use crate::{AppWindow, Nav};

use tabs::section_is_up;
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
/// and because two of the three are only findable from this side: a walk back onto a page
/// that no longer routes, and a tooltip left naming a row that no longer exists.
pub fn disable(ui: &AppWindow, state: &AppState) {
    if let Err(e) = crate::library::playback::player_stop_station(&state.playback_ctx()) {
        log::warn!("radio: stop station on disable: {e}");
    }
    state.nav_history.lock().forget_section(NAV_RADIO);

    // `SidebarItem` publishes its identity into the rail's tooltip channel and there is no
    // unmount hook to clear it, so a row dropped under the pointer would leave the pill up.
    let nav = ui.global::<Nav>();
    if nav.get_sidebar_tip_idx() == NAV_RADIO {
        nav.set_sidebar_tip_idx(-1);
    }
}

/// Wire every `Radio.*` callback.
///
/// Returns nothing: each wired closure clones its own strong `Arc`, so the handle built here is
/// kept alive by the wiring rather than by a caller holding it.
pub fn install(cx: ViewCtx<'_>) {
    let radio_ui = Arc::new(RadioUi::new(section_is_up(cx.app)));
    callbacks::wire(cx.app, cx.state, &radio_ui);
}

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen, and whether what it cached went stale while it wasn't.
    /// **Seeded at wire time rather than left to the gate**, which fires on transitions only and
    /// whose `ChangeTracker` baselines silently inside `AppWindow::new()` — a section seeded
    /// wrong has no edge left to correct it.
    section: SectionState,
}

impl RadioUi {
    fn new(section_active: bool) -> Self {
        let section = SectionState::new();
        section.set_active(section_active);
        Self { section }
    }
}
