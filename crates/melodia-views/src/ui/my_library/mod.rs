//! The My Library page — five tabs (Songs, Albums, Artists, Genres, Playlists) over the
//! five views that used to be five sidebar sections at nav indices 3–7.
//!
//! **This module owns the page, not its contents.** Each of the five keeps its own `*Ui`
//! handle, models, caches, sorts and `view_id` keys; what lives here is the nav index,
//! which tab is mounted, and where a keystroke in the single filter box has to land.
//!
//! There is deliberately **no `MyLibraryUi` handle**. A tabbed page usually needs a
//! synchronous shadow of its active tab so off-thread fetchers know which model to fill,
//! but here the answer already exists five times over: `SectionActiveGate` is mounted per
//! tab, so each view's own `section_active` shadow means "My Library is up *and* my tab is
//! mounted". Everything else that asks runs on the UI thread.

mod callbacks;
// `pub(super)` is `pub(in crate::ui)`: the dispatcher is reached by the four detail
// slices' `open_*`, which reseat the shared box, and by nothing outside `ui::`.
pub(super) mod filter;
mod tabs;

use crate::AppWindow;
use melodia_app::state::AppState;

pub use tabs::{
    MountedSurface, MyLibraryTab, NO_TAB, close_open_detail, detail_id_for, go_to_tab,
    mounted_surface, persist_tab, return_to_section, seed_tab, tab_from_index, tab_is_mounted,
    tab_of_section, the_band_is_up,
};

/// Wire the page's own callbacks. Takes no view handle, which is why it runs before the
/// five tab slices rather than after them. Call once, after `wire_all`.
pub fn install(ui: &AppWindow, state: &AppState) {
    callbacks::wire(ui, state);
}

/// The page's `Nav.selected-index`. **The single definition** — five separate `const`s
/// spelled 4/5/6/7 across `nav_history`, `cross_tab_nav` and `artists/callbacks` used to
/// stand for the sections this page absorbed.
pub const NAV_MY_LIBRARY: i32 = 3;

/// Map a persisted `views.json` nav index onto a live one. 4–7 were Albums / Artists /
/// Genres / Playlists and are in files in the wild; left alone they select no router
/// branch and the user boots onto `PlaceholderView`. Anything else passes through,
/// including values outside the valid range, which the caller still has to bound.
pub fn fold_retired_nav_index(idx: i32) -> i32 {
    if (4..=7).contains(&idx) {
        NAV_MY_LIBRARY
    } else {
        idx
    }
}

#[cfg(test)]
#[path = "tests/my_library_tests.rs"]
mod tests;
