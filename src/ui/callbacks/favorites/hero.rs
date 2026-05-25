//! `Favorites.*` hero pills: play-all and shuffle-all. See
//! [`super::wire_favorites`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the `play-all` / `shuffle-all` hero callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();

    // play-all: enqueue every filtered favourite in display order
    // starting at index 0. Mirrors `tracks::on_play_all`.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_play_all(move || {
            let ids = fu.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "favorites::play_all",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)));
        });
    }
    // shuffle-all: enqueue + flip shuffle on. Cheap composite — no
    // dedicated library helper, so we play the list then toggle
    // shuffle if it isn't already on. The play-all path itself does
    // not respect the user's existing shuffle state, mirroring the
    // Album Detail Shuffle pill behaviour.
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
