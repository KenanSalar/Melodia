//! Chrome wiring for the Settings page — the search predicate its sections filter through,
//! the row split its wrapping strips lay themselves out on, and the persistence for which
//! tab is showing. Distinct from the per-concern installers, which wire the values the
//! page *configures*.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::row_match::{self, Needle};
use crate::ui::tab_bar::clamp_tab;
use crate::{AppWindow, SettingsPage};

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
    pub const ALL: [Self; 5] = [
        Self::Library,
        Self::Playback,
        Self::Interface,
        Self::Services,
        Self::About,
    ];
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

/// Split `0..count` into rows of at most `per_row` — the wrapping chip and swatch strips
/// need their items grouped and Slint can't build a nested array. Indices rather than the
/// items themselves, so a chip still knows which option it is.
///
/// `per_row` is floored at 1: it comes from a measured width, which is zero for the frame
/// before the first layout reports one.
fn chunk_indices(count: i32, per_row: i32) -> Vec<Vec<i32>> {
    let count = count.max(0);
    let per_row = per_row.max(1);

    let row_count =
        usize::try_from(count).unwrap_or(0).div_ceil(usize::try_from(per_row).unwrap_or(1));
    let mut rows = Vec::with_capacity(row_count);
    let mut start = 0;
    while start < count {
        let end = start.saturating_add(per_row).min(count);
        rows.push((start..end).collect());
        start = end;
    }
    rows
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

    // Row split for the wrapping chip / swatch strips — see `chunk_indices`.
    page.on_chunk_indices(|count, per_row| {
        let rows: Vec<ModelRc<i32>> = chunk_indices(count, per_row)
            .into_iter()
            .map(|row| ModelRc::from(Rc::new(VecModel::from(row))))
            .collect();
        ModelRc::from(Rc::new(VecModel::from(rows)))
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
            crate::ui::view_tag::log_current(&ui);
        }
        persist.publish(tab);
        let s_disk = s.clone();
        let persist = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            persist.write_if_current(tab, || {
                if let Err(e) = library::settings::set_settings_tab(&s_disk, tab) {
                    log::warn!("settings_page: set_settings_tab({tab}): {e}");
                }
            });
        });
    });
}

#[cfg(test)]
#[path = "tests/settings_page_tests.rs"]
mod tests;
