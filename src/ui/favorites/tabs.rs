//! Which Favorites sub-view is mounted, and how that answer is seeded.
//!
//! The indices themselves live in `curated.slint`'s `tab-*` constants — no
//! Rust file restates them. [`tab_from_index`] resolves one on the UI thread;
//! everything off it reads the [`super::FavoritesUi`] shadow, which is the
//! only form reachable from a worker.

use slint::ComponentHandle;

use super::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Which Favorites sub-view is mounted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FavoritesTab {
    Songs,
    MostPlayed,
    Artists,
}

impl FavoritesTab {
    /// Every variant. [`tab_from_index`] ends in a default arm, so a tab added
    /// to `curated.slint` without one here resolves to `Songs` — which reads
    /// right on the tab nobody named. Pinned against `tab-count`.
    pub const ALL: [Self; 3] = [Self::Songs, Self::MostPlayed, Self::Artists];

    /// Storage code for the atomic shadow. Deliberately *not* the Slint index
    /// — that lives in the Slint, and these two numbering schemes agreeing
    /// today is a coincidence worth not depending on.
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

/// Resolve a `Favorites.tab-idx` value against the global's own `tab-*`
/// constants. UI thread only — that's where the global is reachable.
pub fn tab_from_index(g: &Favorites<'_>, idx: i32) -> FavoritesTab {
    if idx == g.get_tab_most_played() {
        FavoritesTab::MostPlayed
    } else if idx == g.get_tab_artists() {
        FavoritesTab::Artists
    } else {
        FavoritesTab::Songs
    }
}

/// Seed the active tab from `views.json`, clamped against the Slint-declared
/// `tab-count` (see [`crate::ui::tab_bar::clamp_tab`]).
///
/// Seeds **both** the Slint property and the [`FavoritesUi`] shadow, which is
/// why it takes the handle and why it runs as the last statement of
/// [`super::install`] rather than alongside its siblings in
/// `hydrate_ui_from_settings` — that runs after the handle has gone out of
/// scope, and a shadow left at its `Songs` default would have the first fetch
/// warm the wrong tier for a session that resumes on a grid tab.
pub fn seed_tab(ui: &AppWindow, fav_ui: &FavoritesUi, persisted_tab: i32) {
    let g = ui.global::<Favorites>();
    let clamped = crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count());
    g.set_tab_idx(clamped);
    fav_ui.set_active_tab(tab_from_index(&g, clamped));
}
