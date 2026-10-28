//! The My Library page: five tabs over the Songs, Albums, Artists, Genres and Playlists views.
//! It owns the page, not its contents: each view keeps its own handle, models, caches and sorts,
//! and what lives here is the nav index, the mounted tab and where the filter box's keystrokes
//! land.
//!
//! There is deliberately no `MyLibraryUi` handle: `SectionActiveGate` is mounted per tab, so each
//! view's own `section_active` shadow already means "my tab is mounted", and everything else that
//! asks runs on the UI thread.

mod callbacks;
// Reached by the four details' `open_*`, which reseat the shared box.
pub(super) mod filter;
mod tabs;

use melodia_app::state::AppState;
use melodia_ui::AppWindow;

pub use tabs::{
    MountedSurface, MyLibraryTab, NO_TAB, close_open_detail, detail_id_for, go_to_tab,
    mounted_surface, persist_tab, return_to_section, seed_tab, tab_from_index, tab_is_mounted,
    tab_of_section, the_band_is_up,
};

/// Wires the page's own callbacks, once, after `wire_all`. They take no view handle, so this runs
/// before the five tab slices.
pub fn install(ui: &AppWindow, state: &AppState) {
    callbacks::wire(ui, state);
}

/// The page's `Nav.selected-index`, and the single definition of it.
pub const NAV_MY_LIBRARY: i32 = 3;

/// Maps a persisted nav index onto a live one. 4–7 were the sections this page absorbed and are
/// still in files in the wild, and left alone they boot onto `PlaceholderView`. Anything else
/// passes through, out-of-range values included, for the caller to bound.
pub fn fold_retired_nav_index(idx: i32) -> i32 {
    if (4..=7).contains(&idx) { NAV_MY_LIBRARY } else { idx }
}

#[cfg(test)]
#[path = "tests/my_library_tests.rs"]
mod tests;
