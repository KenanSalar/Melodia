//! Which Favorites tab is mounted. The indices are `curated.slint`'s `tab-*` constants, which no
//! Rust file restates; a worker reads the [`super::FavoritesUi`] shadow instead.

use slint::ComponentHandle;

use super::FavoritesUi;
use melodia_ui::{AppWindow, Favorites};

/// Which Favorites sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FavoritesTab {
    Songs,
    MostPlayed,
    Artists,
}

impl FavoritesTab {
    /// Every variant, pinned against `tab-count`: [`tab_from_index`] falls back to `Songs`, so a
    /// tab missing here would resolve without a word.
    pub const ALL: [Self; 3] = [Self::Songs, Self::MostPlayed, Self::Artists];

    /// The shadow's code, deliberately not the Slint index, which only the Slint defines.
    pub(super) fn as_code(self) -> u8 {
        match self {
            Self::Songs => 0,
            Self::MostPlayed => 1,
            Self::Artists => 2,
        }
    }

    pub(super) fn from_code(code: u8) -> Self {
        match code {
            1 => Self::MostPlayed,
            2 => Self::Artists,
            _ => Self::Songs,
        }
    }
}

/// Resolves a `Favorites.tab-idx` value against the global's own `tab-*` constants.
pub fn tab_from_index(g: &Favorites<'_>, idx: i32) -> FavoritesTab {
    if idx == g.get_tab_most_played() {
        FavoritesTab::MostPlayed
    } else if idx == g.get_tab_artists() {
        FavoritesTab::Artists
    } else {
        FavoritesTab::Songs
    }
}

/// Seeds the tab from `views.json`, clamped to `tab-count`, into both the Slint property and the
/// [`FavoritesUi`] shadow. Hence [`super::install`] rather than `hydrate_ui_from_settings`, which
/// runs once the handle is out of scope: a shadow left on `Songs` would have the first fetch warm
/// the wrong tier on a session resuming on a grid tab.
pub fn seed_tab(ui: &AppWindow, fav_ui: &FavoritesUi, persisted_tab: i32) {
    let g = ui.global::<Favorites>();
    let clamped = crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count());
    g.set_tab_idx(clamped);
    fav_ui.set_active_tab(tab_from_index(&g, clamped));
}
