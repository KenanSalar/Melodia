//! Which Recently Played tab is mounted. The indices are `curated.slint`'s `tab-*` constants,
//! which no Rust file restates; a worker reads the [`super::RecentlyPlayedUi`] shadow instead.

use slint::ComponentHandle;

use super::RecentlyPlayedUi;
use melodia_ui::{AppWindow, RecentlyPlayed};

/// Which Recently-Played sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RecentlyPlayedTab {
    Songs,
    MostPlayed,
}

impl RecentlyPlayedTab {
    /// Every variant, pinned against `tab-count`: [`tab_from_index`] falls back to `Songs`, so a
    /// tab missing here would resolve without a word.
    pub const ALL: [Self; 2] = [Self::Songs, Self::MostPlayed];

    /// The shadow's code, deliberately not the Slint index, which only the Slint defines.
    pub(super) fn as_code(self) -> u8 {
        match self {
            Self::Songs => 0,
            Self::MostPlayed => 1,
        }
    }

    pub(super) fn from_code(code: u8) -> Self {
        match code {
            1 => Self::MostPlayed,
            _ => Self::Songs,
        }
    }
}

/// Resolves a `RecentlyPlayed.tab-idx` value against the global's own `tab-*` constants.
pub fn tab_from_index(g: &RecentlyPlayed<'_>, idx: i32) -> RecentlyPlayedTab {
    if idx == g.get_tab_most_played() {
        RecentlyPlayedTab::MostPlayed
    } else {
        RecentlyPlayedTab::Songs
    }
}

/// Seeds the tab from `views.json`, clamped to `tab-count`, into both the Slint property and the
/// [`RecentlyPlayedUi`] shadow. From [`super::install`], for [`crate::ui::favorites::seed_tab`]'s
/// reason.
pub fn seed_tab(ui: &AppWindow, rp_ui: &RecentlyPlayedUi, persisted_tab: i32) {
    let g = ui.global::<RecentlyPlayed>();
    let clamped = crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count());
    g.set_tab_idx(clamped);
    rp_ui.set_active_tab(tab_from_index(&g, clamped));
}
