//! Chrome wiring for the Settings page — the search predicate its sections filter through,
//! the persistence for which category is showing, and the deep link landing on one from
//! elsewhere. Distinct from the per-concern installers, which wire the values the page
//! *configures*.

use std::cell::RefCell;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::nav_transition;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, Nav, NavEnterFrom, SettingsPage};

/// This page's `Nav.selected-index`, spelled once in Rust. The map itself is `globals/nav.slint`'s.
const NAV_SETTINGS: i32 = 9;

/// Which Settings category is showing. The `FavoritesTab` / `RecentlyPlayedTab` shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsCategory {
    Library,
    Playback,
    Effects,
    Output,
    Appearance,
    Language,
    Window,
    Radio,
    Lyrics,
    Integrations,
    Updates,
    Accessibility,
    About,
}

impl SettingsCategory {
    /// Every variant, so a category added to the `.slint` without one here fails a test rather
    /// than falling through [`category_from_index`]'s default.
    pub const ALL: [Self; 13] = [
        Self::Library,
        Self::Playback,
        Self::Effects,
        Self::Output,
        Self::Appearance,
        Self::Language,
        Self::Window,
        Self::Radio,
        Self::Lyrics,
        Self::Integrations,
        Self::Updates,
        Self::Accessibility,
        Self::About,
    ];

    /// What `views.json` keeps, so a category inserted mid-list moves no saved choice.
    pub fn key(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Playback => "playback",
            Self::Effects => "effects",
            Self::Output => "output",
            Self::Appearance => "appearance",
            Self::Language => "language",
            Self::Window => "window",
            Self::Radio => "radio",
            Self::Lyrics => "lyrics",
            Self::Integrations => "integrations",
            Self::Updates => "updates",
            Self::Accessibility => "accessibility",
            Self::About => "about",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.key() == key)
    }
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
        SettingsCategory::Effects => page.get_category_effects(),
        SettingsCategory::Output => page.get_category_output(),
        SettingsCategory::Appearance => page.get_category_appearance(),
        SettingsCategory::Language => page.get_category_language(),
        SettingsCategory::Window => page.get_category_window(),
        SettingsCategory::Radio => page.get_category_radio(),
        SettingsCategory::Lyrics => page.get_category_lyrics(),
        SettingsCategory::Integrations => page.get_category_integrations(),
        SettingsCategory::Updates => page.get_category_updates(),
        SettingsCategory::Accessibility => page.get_category_accessibility(),
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

/// Seed the active category from `views.json`'s key, Library where it names none. Call from
/// `boot::ui_setup::hydrate::hydrate_ui_from_settings`, which already has the view state loaded.
pub fn seed_category(ui: &AppWindow, persisted_key: &str) {
    let page = ui.global::<SettingsPage>();
    let category = SettingsCategory::from_key(persisted_key).unwrap_or(SettingsCategory::Library);
    page.set_category_idx(category_index(&page, category));
}

pub fn install(ui: &AppWindow, state: &AppState) {
    let page = ui.global::<SettingsPage>();

    // Slint 1.18 has no `.contains()` on a string, so every row's visibility test comes here, the
    // filter boxes' own predicate, and an ASCII query reaches the accented catalogue labels.
    // Memoized against the raw needle: invoked per *field* rather than per pass, this is the one
    // `Needle` caller that can't hold a folded shadow.
    let folded: RefCell<(String, Needle)> = RefCell::new((String::new(), Needle::default()));
    page.on_matches(move |haystack, needle| {
        let mut memo = folded.borrow_mut();
        if memo.0 != needle.as_str() {
            memo.1 = fold_needle(&needle);
            memo.0 = needle.into();
        }
        memo.1.contains(&haystack)
    });

    let weak = ui.as_weak();
    page.on_open_category(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let category = category_from_index(&ui.global::<SettingsPage>(), idx);
        open_on(&ui, category);
    });

    // A catch-up write, the panel's `<=>` already showing the pick, ordered by `IndexPersist`. Not
    // `spawn_blocking_logged!`: a pick logs its own `nav:` line, which the macro's would repeat.
    let s = state.clone();
    let weak = ui.as_weak();
    let persist = Arc::new(IndexPersist::new(page.get_category_idx()));
    page.on_category_changed(move |category| {
        let Some(ui) = weak.upgrade() else { return };
        // As on the two curated pages' tabs: a pick moves no nav index, so
        // `nav_history::record_current` never hears about it.
        let page = ui.global::<SettingsPage>();
        page.invoke_clear_search();
        crate::ui::view_tag::log_current(&ui);
        let key = category_from_index(&page, category).key();

        persist.publish(category);
        let s_disk = s.clone();
        let persist = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            persist.write_if_current(category, || {
                if let Err(e) = library::settings::set_settings_category(&s_disk.paths, key) {
                    log::warn!("settings_page: set_settings_category({key}): {}", describe(&e));
                }
            });
        });
    });
}

#[cfg(test)]
#[path = "tests/settings_page_tests.rs"]
mod tests;
