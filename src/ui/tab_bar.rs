//! The Rust half of `melodia-ui/ui/components/tab-bar.slint`.
//!
//! Two pages mount a `TabBar` and persist which tab was showing — the Settings
//! page (`SettingsPage.tab-idx`, `views.json`'s `settings_tab`) and Favorites
//! (`Favorites.tab-idx`, `favorites_tab`). Both need the same read-side clamp,
//! and the component's source-level invariants are pinned here rather than
//! under either host, since neither owns it any more.

/// Clamp a persisted tab index into range.
///
/// `tab_count` comes from the mounting page's Slint global rather than a const
/// here, so the number of tabs has exactly one definition — in the Slint that
/// declares them.
///
/// The guard matters on read, not write: the tab bar can only ever produce a
/// valid index, but a `views.json` left by a build with more tabs would
/// otherwise select a branch that mounts nothing and show a blank page.
pub(crate) fn clamp_tab(tab: i32, tab_count: i32) -> i32 {
    tab.clamp(0, (tab_count - 1).max(0))
}

#[cfg(test)]
#[path = "tests/tab_bar_tests.rs"]
mod tests;
