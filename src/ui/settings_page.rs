//! Chrome wiring for the Settings page — the search predicate its sections
//! filter through, and the persistence for which tab is showing.
//!
//! Distinct from the per-concern installers (`playback_settings`,
//! `scrobbling_settings`, …), which wire the values the page *configures*.
//! This module owns the page itself.

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::{AppWindow, SettingsPage};

/// Clamp a persisted tab index into range. `tab_count` comes from the
/// `SettingsPage` global rather than a const here, so the number of tabs has
/// exactly one definition — in the Slint that declares them.
///
/// The guard matters on read, not write: the tab bar can only ever produce a
/// valid index, but a `views.json` left by a build with more tabs would
/// otherwise select a branch that mounts nothing and show a blank page.
fn clamp_tab(tab: i32, tab_count: i32) -> i32 {
    tab.clamp(0, (tab_count - 1).max(0))
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
    page.on_matches(|haystack, needle| haystack.to_lowercase().contains(&needle.to_lowercase()));

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
