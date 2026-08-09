//! The My Library page — five tabs (Songs, Albums, Artists, Genres, Playlists) over the
//! five views that used to be five sidebar sections at nav indices 3–7.
//!
//! **This module owns the page, not its contents.** Each of the five keeps its own
//! `*Ui` handle, models, caches, sorts and `view_id` keys; what lives here is the nav
//! index, which tab is mounted, and where a keystroke in the page's single filter box
//! has to land.
//!
//! There is deliberately **no `MyLibraryUi` handle**. A tabbed page usually needs a
//! synchronous shadow of its active tab so off-thread fetchers know which model to fill —
//! Favorites and Recently Played both carry one — but here the answer already exists five
//! times over: `SectionActiveGate` is mounted per tab, so each view's own `section_active`
//! shadow means "My Library is up *and* my tab is mounted". Everything else that asks
//! (the filter dispatcher, `nav_history`, `cross_tab_nav`, `rss_sampler::format_view`)
//! runs on the UI thread with the global in reach.

pub mod filter;
mod tabs;

pub use tabs::{
    MyLibraryTab, NO_TAB, a_detail_hero_is_mounted, close_open_detail, detail_open_on,
    persist_tab, restore_origin, seed_tab, tab_from_index, tab_is_mounted, tab_of_section,
    the_band_is_up, the_chips_still_belong_to_the_band,
};

/// The page's `Nav.selected-index`. **The single definition** — five separate `const`s
/// spelled 4/5/6/7 across `nav_history`, `cross_tab_nav` and `callbacks/artists` used to
/// stand for the sections this page absorbed.
pub const NAV_MY_LIBRARY: i32 = 3;

/// Map a persisted `views.json` nav index onto a live one.
///
/// 4–7 were Albums / Artists / Genres / Playlists. The app is publicly released, so those
/// values are in `views.json` files in the wild; left alone they select no router branch
/// and the user boots onto `PlaceholderView`. Anything else passes through — including
/// values outside the valid range, which the caller still has to bound.
pub fn fold_retired_nav_index(idx: i32) -> i32 {
    if (4..=7).contains(&idx) { NAV_MY_LIBRARY } else { idx }
}

#[cfg(test)]
#[path = "tests/my_library_tests.rs"]
mod tests;
