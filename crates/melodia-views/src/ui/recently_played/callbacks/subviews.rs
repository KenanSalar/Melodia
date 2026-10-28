//! The Most Played cards' actions and the tab switch.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::spawn_play_then_shuffle;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::{
    self as recently_played_ui_mod, RecentlyPlayedTab, RecentlyPlayedUi,
};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // A card queues its grid rather than the recency list, starting on the card. A card carries
    // no row index, so the start comes from the id.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_play_track(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let id = i64::from(id);
            let ids = recently_played_ui_mod::most_played_ids(&ui);
            if ids.is_empty() {
                return;
            }
            let start = ids.iter().position(|&i| i == id);
            let s = s.clone();
            spawn_logged!(
                s,
                "recently_played::play_track",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }

    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_shuffle_most_played(move || {
            let Some(ui) = weak.upgrade() else { return };
            spawn_play_then_shuffle(
                &s,
                "recently_played::shuffle_most_played",
                recently_played_ui_mod::most_played_ids(&ui),
            );
        });
    }

    // Catches the Rust side up once the bar has moved `tab-idx` and cleared the Slint-side filter.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<RecentlyPlayed>();
            let entering = recently_played_ui_mod::tab_from_index(&g, tab);
            persist.publish(tab);
            // A pick moves no nav index, so `record_current` never logs it.
            crate::ui::view_tag::log_current(&ui);

            // Before the shadow moves: the grid keeps its ids across the pick, under a needle the
            // pick clears. Songs re-walks its cache on entry.
            if ru.active_tab() == RecentlyPlayedTab::MostPlayed
                && !recently_played_ui_mod::current_filter(&ru).is_empty()
            {
                ru.mark_grid_dirty();
            }

            // Shadow first, every load below choosing its model and covers from it.
            ru.set_active_tab(entering);
            recently_played_ui_mod::set_filter(&ru, "");
            crate::ui::hero_chips::publish_recently_played(&ui, &ru);

            if entering == RecentlyPlayedTab::Songs {
                recently_played_ui_mod::apply_filtered_tracks_now(&ui, &ru);
            } else {
                // Songs' rows aren't kept behind a tab the user left, and `apply_filtered_tracks`
                // won't refill them while the grid is up.
                clear_vec_model::<UiTrackListRow>(
                    &g.get_tracks(),
                    "recently_played: leave songs tab",
                );
                if ru.take_grid_dirty() {
                    recently_played_ui_mod::reload_grid(&ui, &s, &ru);
                }
            }

            let ru_covers = ru.clone();
            let s_disk = s.clone();
            let persist_disk = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                // Only the write is superseded; the prewarm runs regardless.
                persist_disk.write_if_current(tab, || {
                    if let Err(e) = library::settings::set_recently_played_tab(&s_disk.paths, tab) {
                        log::warn!("recently_played::set_recently_played_tab: {}", describe(&e));
                    }
                });
                recently_played_ui_mod::prewarm_covers(&ru_covers);
            });
        });
    }

    // A resize only changes how the ids are chunked, so the cached cards stay.
    {
        let weak = weak.clone();
        g.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::set_columns(&ui, cols);
        });
    }
}
