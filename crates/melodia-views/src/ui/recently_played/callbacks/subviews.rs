//! `RecentlyPlayed.*` card actions and sub-view routing: the Most Played
//! play-track and shuffle actions, the tab switch, and the grid's column-count
//! push. See [`super::wire`].

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

/// Wire the card-action and sub-view-routing callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // play-track: clicking a Most Played card loads that grid into the queue and
    // starts on that card — the grid is the context, not the recency list on the
    // other tab. The Slint callback carries no row index (these are cards, not
    // list rows), so the start slot comes from the id.
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

    // --- Hero pill: Shuffle the Most Played grid -------------------
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

    // --- Tab switch -----------------------------------------------
    // The bar has already moved `tab-idx` and cleared the Slint-side filter by
    // the time this runs, so this is the catch-up: drop the Rust filter shadow to
    // match, fill or reload the entering tab, then off-thread persist the pick
    // and warm the grid's covers.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        // Ordered: a bounce queues a value per pick and two blocking tasks have
        // none of their own, so a reversed pair reopens the page on a tab the
        // user only passed through.
        let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<RecentlyPlayed>();
            let entering = recently_played_ui_mod::tab_from_index(&g, tab);
            persist.publish(tab);
            // A sub-view pick moves no nav index, so `record_current` never
            // hears about it. The bar has already written `tab-idx`, so the tag
            // reads the tab being entered.
            crate::ui::view_tag::log_current(&ui);

            // Asked before the shadow moves, so it reads the tab being left: the
            // grid keeps its ids across the pick, and the needle it was loaded
            // under is about to be cleared. Songs re-walks its cache on entry.
            if ru.active_tab() == RecentlyPlayedTab::MostPlayed
                && !recently_played_ui_mod::current_filter(&ru).is_empty()
            {
                ru.mark_grid_dirty();
            }

            // Shadow first: every later load decides which model to fill and
            // which covers to warm from it, and both can run off the UI thread
            // where the global isn't reachable.
            ru.set_active_tab(entering);
            recently_played_ui_mod::set_filter(&ru, "");
            crate::ui::hero_chips::publish_recently_played(&ui, &ru);

            if entering == RecentlyPlayedTab::Songs {
                // Through the `_now` variant: the tab mounts on the next frame,
                // and a posted write that loses the race paints a `TrackList` of
                // headers over the model the leave below emptied.
                recently_played_ui_mod::apply_filtered_tracks_now(&ui, &ru);
            } else {
                // Leaving empties the model rather than leaving a `TrackListRow`
                // of `SharedString`s per track pinned behind a tab the user has
                // left. `apply_filtered_tracks` refuses to refill it while that's
                // true, so nothing puts them back.
                clear_vec_model::<UiTrackListRow>(
                    &g.get_tracks(),
                    "recently_played: leave songs tab",
                );
                // The grid kept its cards, so it paints on this tick; a tick it
                // missed while Songs was up reloads behind them. See
                // `lifecycle::kick_full_refresh`.
                if ru.take_grid_dirty() {
                    recently_played_ui_mod::reload_grid(&ui, &s, &ru);
                }
            }

            let ru_covers = ru.clone();
            let s_disk = s.clone();
            let persist_disk = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                // Scoped to the write alone: a superseded tab drops its own disk
                // hop, and the cover swap below is this task's regardless.
                persist_disk.write_if_current(tab, || {
                    if let Err(e) = library::settings::set_recently_played_tab(&s_disk.paths, tab) {
                        log::warn!("recently_played::set_recently_played_tab: {}", describe(&e));
                    }
                });
                // Whatever the grid painted last visit is still in the tier as a
                // proxy; this sharpens it, and anything genuinely absent
                // schedules itself off the card's own lookup.
                recently_played_ui_mod::prewarm_covers(&ru_covers);
            });
        });
    }

    // --- Grid column count ----------------------------------------
    // A resize only changes how the ids are chunked, so the cached cards stay.
    {
        let weak = weak.clone();
        g.on_columns_changed(move |cols| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::set_columns(&ui, cols);
        });
    }
}
