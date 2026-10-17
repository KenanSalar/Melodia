//! Chrome wiring for the Settings page — the search predicate its sections filter through
//! and the persistence for which category is showing. Distinct from the per-concern installers,
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

/// Which Settings category is showing. The `FavoritesTab` / `RecentlyPlayedTab` shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsCategory {
    Library,
    Playback,
    Output,
    Appearance,
    Window,
    Radio,
    Lyrics,
    Integrations,
    Updates,
    About,
}

impl SettingsCategory {
    /// Every variant, so a category added to the `.slint` without one here fails a test rather
    /// than falling through [`category_from_index`]'s default.
    pub const ALL: [Self; 10] = [
        Self::Library,
        Self::Playback,
        Self::Output,
        Self::Appearance,
        Self::Window,
        Self::Radio,
        Self::Lyrics,
        Self::Integrations,
        Self::Updates,
        Self::About,
    ];
}

/// Resolve the page's live index against the Slint-declared `category-*` constants, read
/// rather than restated so the numbering lives in one place.
pub fn category_from_index(page: &SettingsPage<'_>, idx: i32) -> SettingsCategory {
    SettingsCategory::ALL
        .into_iter()
        .find(|&category| category_index(page, category) == idx)
        .unwrap_or(SettingsCategory::Library)
}

/// The page's index for `category`, the one place the variants meet the constants.
fn category_index(page: &SettingsPage<'_>, category: SettingsCategory) -> i32 {
    match category {
        SettingsCategory::Library => page.get_category_library(),
        SettingsCategory::Playback => page.get_category_playback(),
        SettingsCategory::Output => page.get_category_output(),
        SettingsCategory::Appearance => page.get_category_appearance(),
        SettingsCategory::Window => page.get_category_window(),
        SettingsCategory::Radio => page.get_category_radio(),
        SettingsCategory::Lyrics => page.get_category_lyrics(),
        SettingsCategory::Integrations => page.get_category_integrations(),
        SettingsCategory::Updates => page.get_category_updates(),
        SettingsCategory::About => page.get_category_about(),
    }
}

/// Opens Settings on `category` from anywhere, Now Playing included.
///
/// Category first, then nav, so the page mounts on the body it is meant to show,
/// `my_library::go_to_tab` for the same reason. Both writes go through the callbacks that already
/// own an `IndexPersist`; calling the disk setters directly would be a seventh writer, which
/// `crates/melodia/tests/index_persist.rs` pins against. The index moves only where it has to, as
/// in `nav_history`'s `PendingNav::apply`: with Settings already under Now Playing, closing that is
/// the whole of the navigation.
pub fn open_on(ui: &AppWindow, category: SettingsCategory) {
    let page = ui.global::<SettingsPage>();
    let idx = category_index(&page, category);
    page.set_category_idx(idx);
    page.invoke_category_changed(idx);

    nav_transition::mark(ui, NavEnterFrom::Above);
    let nav = ui.global::<Nav>();
    nav.set_now_playing_open(false);
    if nav.get_selected_index() != NAV_SETTINGS {
        nav.set_selected_index(NAV_SETTINGS);
        nav.invoke_persist_selected_index(NAV_SETTINGS);
    }
}

/// Seed the active category from `views.json`. Call from
/// `boot::ui_setup::hydrate::hydrate_ui_from_settings`, which already has the view state loaded.
pub fn seed_category(ui: &AppWindow, persisted_category: i32) {
    let page = ui.global::<SettingsPage>();
    let clamped = clamp_tab(persisted_category, page.get_category_count());
    page.set_category_idx(clamped);
}

/// Wire the Settings page's chrome. Call once during startup.
pub fn install(ui: &AppWindow, state: &AppState) {
    let page = ui.global::<SettingsPage>();

    // Slint 1.18 has no `.contains()` on string, so every section's row-visibility
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

    // The panel two-way binds `category-idx`, so the UI is already showing the new category
    // and the disk write is pure catch-up. Hand-rolled rather than `spawn_blocking_logged!`,
    // which this otherwise matches: a pick already logs its own `nav:` line, so the macro's
    // `view state:` line would say the same thing twice.
    let s = state.clone();
    let weak = ui.as_weak();
    // Ordered: a bounce queues a value per pick and two blocking tasks have none of their
    // own, so a reversed pair reopens the page on a category the user only passed through.
    let persist = Arc::new(IndexPersist::new(page.get_category_idx()));
    page.on_category_changed(move |category| {
        // As on the two curated pages' tabs: a pick moves no nav index, so
        // `nav_history::record_current` never hears about it.
        if let Some(ui) = weak.upgrade() {
            ui.global::<SettingsPage>().invoke_clear_search();
            crate::ui::view_tag::log_current(&ui);
        }
        persist.publish(category);
        let s_disk = s.clone();
        let persist = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            persist.write_if_current(category, || {
                if let Err(e) = library::settings::set_settings_category(&s_disk.paths, category) {
                    log::warn!(
                        "settings_page: set_settings_category({category}): {}",
                        describe(&e)
                    );
                }
            });
        });
    });
}

#[cfg(test)]
#[path = "tests/settings_page_tests.rs"]
mod tests;
