//! The hero's shuffle pills. See [`super::wire`].
//!
//! The pill is per-tab, so each one shuffles what its own tab is showing —
//! and both id lists are filter-aware, so the queue can't disagree with what
//! is on screen.

use slint::ComponentHandle;

use crate::ui::callbacks::spawn_play_then_shuffle;
use crate::ui::favorites as favorites_ui_mod;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Favorites};

/// Wire the two hero shuffle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Favorites>();

    {
        let s = state.clone();
        let weak = ui.as_weak();
        g.on_shuffle_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            let ids = favorites_ui_mod::displayed_ids(&ui.global::<Favorites>());
            spawn_play_then_shuffle(&s, "favorites::shuffle_all", ids);
        });
    }

    {
        let s = state.clone();
        let weak = ui.as_weak();
        g.on_shuffle_most_played(move || {
            let Some(ui) = weak.upgrade() else { return };
            spawn_play_then_shuffle(
                &s,
                "favorites::shuffle_most_played",
                favorites_ui_mod::most_played_ids(&ui),
            );
        });
    }
}
