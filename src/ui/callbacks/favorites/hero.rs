//! The `Favorites.shuffle-all` hero pill. See [`super::wire_favorites`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::callbacks::spawn_play_then_shuffle;
use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the `shuffle-all` hero callback.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();

    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_shuffle_all(move || {
            spawn_play_then_shuffle(&s, "favorites::shuffle_all", fu.filtered_track_ids());
        });
    }
}
