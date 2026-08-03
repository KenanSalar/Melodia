//! Chrome wiring for the Settings page — the search predicate its sections
//! filter through, the row split its wrapping strips lay themselves out on,
//! and the persistence for which tab is showing.
//!
//! Distinct from the per-concern installers (`playback_settings`,
//! `scrobbling_settings`, …), which wire the values the page *configures*.
//! This module owns the page itself.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::library;
use crate::state::AppState;
use crate::ui::row_match;
use crate::ui::tab_bar::clamp_tab;
use crate::{AppWindow, SettingsPage};

/// Split `0..count` into rows of at most `per_row`.
///
/// The wrapping chip and swatch strips need their items grouped into rows, and
/// Slint can't build a nested array — so the split comes from here and the
/// strip iterates the groups. Indices rather than the items themselves, so a
/// chip still knows which option it is and can compare against the selection.
///
/// `per_row` is floored at 1: it is derived from a measured width, which is
/// zero for the frame before the first layout reports one.
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
/// `boot::ui_setup::hydrate_ui_from_settings`, which already has the view
/// state loaded.
pub fn seed_tab(ui: &AppWindow, persisted_tab: i32) {
    let page = ui.global::<SettingsPage>();
    let clamped = clamp_tab(persisted_tab, page.get_tab_count());
    page.set_tab_idx(clamped);
}

/// Wire the Settings page's chrome. Call once during startup.
pub fn install(ui: &AppWindow, state: &AppState) {
    let page = ui.global::<SettingsPage>();

    // Slint 1.16 has no `.contains()` on string, so every section's
    // row-visibility expression routes its substring test through here.
    // Same predicate the library filter boxes run, so an ASCII query
    // reaches the accented labels in the translated catalogues.
    page.on_matches(|haystack, needle| {
        row_match::field_contains(&haystack, &row_match::fold_needle(&needle))
    });

    // Row split for the wrapping chip / swatch strips — see `chunk_indices`.
    page.on_chunk_indices(|count, per_row| {
        let rows: Vec<ModelRc<i32>> = chunk_indices(count, per_row)
            .into_iter()
            .map(|row| ModelRc::from(Rc::new(VecModel::from(row))))
            .collect();
        ModelRc::from(Rc::new(VecModel::from(rows)))
    });

    // The tab bar two-way binds `tab-idx`, so the UI is already showing the
    // new tab by the time this runs — the disk write is pure catch-up and a
    // failure must not try to undo it.
    let s = state.clone();
    page.on_tab_changed(move |tab| {
        s.persist_blocking("settings_page::set_settings_tab", move |st| {
            library::settings::set_settings_tab(st, tab)
        });
    });
}

#[cfg(test)]
#[path = "tests/settings_page_tests.rs"]
mod tests;
