//! Chrome wiring for the Settings page — the search predicate its sections filter through
//! and the persistence for which tab is showing. Distinct from the per-concern installers,
//! which wire the values the page *configures*.

use std::cell::RefCell;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::nav_transition;
use crate::ui::row_match::{self, Needle};
use crate::ui::tab_bar::clamp_tab;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Nav, NavEnterFrom, SettingsPage};

/// This page's `Nav.selected-index`, spelled once in Rust. The map itself is `globals/nav.slint`'s.
const NAV_SETTINGS: i32 = 9;

/// Which Settings tab is showing. The `FavoritesTab` / `RecentlyPlayedTab` shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    Library,
    Playback,
    Interface,
    Services,
    About,
}

impl SettingsTab {
    /// Every variant, so a tab added to the `.slint` without one here fails a test rather
    /// than falling through [`tab_from_index`]'s default arm.
    pub const ALL: [Self; 5] =
        [Self::Library, Self::Playback, Self::Interface, Self::Services, Self::About];
}

/// Resolve the page's live index against the Slint-declared `tab-*` constants, read rather
/// than restated so the numbering lives in one place.
pub fn tab_from_index(page: &SettingsPage<'_>, idx: i32) -> SettingsTab {
    if idx == page.get_tab_playback() {
        SettingsTab::Playback
    } else if idx == page.get_tab_interface() {
        SettingsTab::Interface
    } else if idx == page.get_tab_services() {
        SettingsTab::Services
    } else if idx == page.get_tab_about() {
        SettingsTab::About
    } else {
        SettingsTab::Library
    }
}

/// The page's index for `tab`, read off the same constants as [`tab_from_index`].
fn tab_index(page: &SettingsPage<'_>, tab: SettingsTab) -> i32 {
    match tab {
        SettingsTab::Library => page.get_tab_library(),
        SettingsTab::Playback => page.get_tab_playback(),
        SettingsTab::Interface => page.get_tab_interface(),
        SettingsTab::Services => page.get_tab_services(),
        SettingsTab::About => page.get_tab_about(),
    }
}

/// Opens Settings on `tab` from anywhere, Now Playing included.
///
/// Tab first, then nav, so the page mounts on the body it is meant to show, `my_library::go_to_tab`
/// for the same reason. Both writes go through the callbacks that already own an `IndexPersist`;
/// calling the disk setters directly would be a seventh writer, which
/// `crates/melodia/tests/index_persist.rs` pins against. The index moves only where it has to, as
/// in `nav_history`'s `PendingNav::apply`: with Settings already under Now Playing, closing that is
/// the whole of the navigation.
pub fn open_on(ui: &AppWindow, tab: SettingsTab) {
    let page = ui.global::<SettingsPage>();
    let idx = tab_index(&page, tab);
    page.set_tab_idx(idx);
    page.invoke_tab_changed(idx);

    nav_transition::mark(ui, NavEnterFrom::Above);
    let nav = ui.global::<Nav>();
    nav.set_now_playing_open(false);
    if nav.get_selected_index() != NAV_SETTINGS {
        nav.set_selected_index(NAV_SETTINGS);
        nav.invoke_persist_selected_index(NAV_SETTINGS);
    }
}

/// Seed the active tab from `views.json`. Call from
/// `boot::ui_setup::hydrate_ui_from_settings`, which already has the view state loaded.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let page = ui.global::<SettingsPage>();
    let clamped = clamp_tab(persisted_tab, page.get_tab_count());
    page.set_tab_idx(clamped);
}

/// Wire the Settings page's chrome. Call once during startup.
pub fn install(ui: &AppWindow, state: &AppState) {
    let page = ui.global::<SettingsPage>();

    // Slint 1.16 has no `.contains()` on string, so every section's row-visibility
    // expression routes its substring test through here — the same predicate the library
    // filter boxes run, so an ASCII query reaches the accented catalogue labels.
    //
    // The fold is memoized against the raw needle because this is the one `row_match`
    // caller that can't hold a folded shadow: `matches` is invoked per *field*, not per
    // pass, and the page has dozens of call sites live at once while searching.
    let folded: RefCell<(String, Needle)> = RefCell::new((String::new(), Needle::default()));
    page.on_matches(move |haystack, needle| {
        let mut memo = folded.borrow_mut();
        if memo.0 != needle.as_str() {
            memo.1 = row_match::fold_needle(&needle);
            memo.0 = needle.into();
        }
        memo.1.contains(&haystack)
    });

    // The tab bar two-way binds `tab-idx`, so the UI is already showing the new tab and
    // the disk write is pure catch-up. Hand-rolled rather than `spawn_blocking_logged!`,
    // which this otherwise matches: a tab pick already logs its own `nav:` line, so the
    // macro's `view state:` line would say the same thing twice.
    let s = state.clone();
    let weak = ui.as_weak();
    // Ordered: a bounce queues a value per pick and two blocking tasks have none of their
    // own, so a reversed pair reopens the page on a tab the user only passed through.
    let persist = Arc::new(IndexPersist::new(page.get_tab_idx()));
    page.on_tab_changed(move |tab| {
        // As on the two curated pages: a tab pick moves no nav index, so
        // `nav_history::record_current` never hears about it.
        if let Some(ui) = weak.upgrade() {
            ui.global::<SettingsPage>().invoke_clear_search();
            crate::ui::view_tag::log_current(&ui);
        }
        persist.publish(tab);
        let s_disk = s.clone();
        let persist = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            persist.write_if_current(tab, || {
                if let Err(e) = library::settings::set_settings_tab(&s_disk.paths, tab) {
                    log::warn!("settings_page: set_settings_tab({tab}): {}", describe(&e));
                }
            });
        });
    });
}

#[cfg(test)]
#[path = "tests/settings_page_tests.rs"]
mod tests;
