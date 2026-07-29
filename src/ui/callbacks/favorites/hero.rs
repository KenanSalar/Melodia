//! The `Favorites.shuffle-all` hero pill. See [`super::wire_favorites`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the `shuffle-all` hero callback.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();

    // shuffle-all: enqueue every filtered favourite in display order, then
    // flip shuffle on. Cheap composite — no dedicated library helper, so we
    // play the list then toggle shuffle if it isn't already on.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_shuffle_all(move || {
            let ids = fu.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)).await
                {
                    log::warn!("favorites::shuffle_all play: {e}");
                    return;
                }
                let shuffle_on = {
                    let g = crate::player::state::lock_state(&s.player_state);
                    g.queue.shuffle_enabled
                };
                if !shuffle_on
                    && let Err(e) = library::queue::queue_toggle_shuffle(&s)
                {
                    log::warn!("favorites::shuffle_all toggle: {e}");
                }
            });
        });
    }
}
