//! `RecentlyPlayed.*` card actions and sub-view routing: the Most Played
//! play-track and shuffle actions, the tab switch, and the grid's column-count
//! push. See [`super::wire`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::spawn_play_then_shuffle;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::tab_bar::{UNFETCHED_COUNT, should_announce_warm};
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};
use melodia_app::library;
use melodia_app::state::AppState;

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
        let ru = rp_ui.clone();
        g.on_play_track(move |id| {
            let id = i64::from(id);
            let ids = ru.most_played_track_ids();
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
        let ru = rp_ui.clone();
        g.on_shuffle_most_played(move || {
            spawn_play_then_shuffle(
                &s,
                "recently_played::shuffle_most_played",
                ru.most_played_track_ids(),
            );
        });
    }

    // --- Tab switch -----------------------------------------------
    // The bar has already moved `tab-idx` and cleared the Slint-side filter by
    // the time this runs, so this is the catch-up: drop the Rust filter shadow to
    // match, build the entering tab's model, then off-thread persist the pick and
    // swap the cover tier over.
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

            // Shadow first: the model build below and every later fetch decide
            // which model to fill and which tier to warm from it, and both can
            // run off the UI thread where the global isn't reachable.
            ru.set_active_tab(entering);
            recently_played_ui_mod::set_filter(&ru, "");

            // Asked *before* the apply below, because the apply is what has to know:
            // the answer decides whether the count it writes stands for anything.
            let needs_grid_fetch = entering
                == recently_played_ui_mod::RecentlyPlayedTab::MostPlayed
                && ru.take_grid_dirty();

            // The entering tier was cleared when its tab was last left, so the
            // cards mount cold: hold the lookups at cache-only until the prewarm
            // below reports back, or each visible card drags a grid-tier decode onto
            // this thread in the frame that paints the grid.
            g.set_covers_generation(0);

            // Fill the entering tab's model on this tick — it mounts on the next
            // frame, and the `_now` variants are what make "this tick" true: a
            // hidden tab's model is empty, and the count that gates its empty
            // state is not, so a grid that paints before its rows land shows a
            // bare panel rather than a placeholder, and a list does the same with
            // a header band over nothing.
            recently_played_ui_mod::apply_filtered_grid_now(&ui, &ru);
            if entering == recently_played_ui_mod::RecentlyPlayedTab::Songs {
                recently_played_ui_mod::apply_filtered_tracks_now(&ui, &ru);
            } else {
                // Leaving empties the model rather than leaving it holding its
                // last rows — the same trade `write_filtered_grid` makes for the
                // grid it isn't mounting, and for the same reason: a row per
                // track is one `TrackListRow` of `SharedString`s pinned behind a
                // tab the user has left. `apply_filtered_tracks` refuses to
                // refill it while that's true, so nothing puts them back.
                clear_vec_model::<UiTrackListRow>(
                    &g.get_tracks(),
                    "recently_played: leave songs tab",
                );
            }

            // The grid's query only runs while its tab is mounted, so entering
            // it is where a tick skipped on the Songs tab gets paid for. Spawned
            // *after* the synchronous apply above, so whatever the cache already
            // holds paints on this tick and the fetch refreshes behind it; the
            // dirty flag is what keeps a pick back and forth from re-querying a
            // cache nothing has invalidated. See `lifecycle::kick_full_refresh`.
            //
            // **And the apply's count has to be taken back, because the cache it
            // walked is the one this fetch is about to fill.** A `0` there is the
            // one value that means "there is nothing here" — it mounts
            // `GridEmptyState`'s "Nothing played yet" over a library that has
            // plenty, for a full uncapped `get_most_played` plus the cover-decode
            // burst it awaits. `UNFETCHED_COUNT` matches neither `== 0` nor `> 0`,
            // so the panel and the Shuffle pill both stay quiet until the fetch
            // answers; the band's chips go with them, `most_played_chips` publishing
            // none at a zero total, which is the shape an empty hero already has.
            if needs_grid_fetch {
                g.set_most_played_count(UNFETCHED_COUNT);
                let s_fetch = s.clone();
                let ru_fetch = ru.clone();
                let weak_fetch = weak.clone();
                s.runtime.spawn(async move {
                    recently_played_ui_mod::refresh_grid(&s_fetch, &ru_fetch, &weak_fetch).await;
                });
            }

            let ru_covers = ru.clone();
            let s_disk = s.clone();
            let weak_warm = weak.clone();
            let persist_disk = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                // Scoped to the write alone: a superseded tab drops its own disk
                // hop, and the cover swap below is this task's regardless.
                persist_disk.write_if_current(tab, || {
                    if let Err(e) = library::settings::set_recently_played_tab(&s_disk, tab) {
                        log::warn!("recently_played::set_recently_played_tab: {e}");
                    }
                });
                // `warm` is the decode's own verdict — a section leave landing
                // inside it handed the buffers back — so `Some(entering)` is
                // exactly "we decoded for this tab and still hold it", which is
                // what `should_announce_warm` takes. The other two terms are read
                // on the UI thread, where both shadows are written: a pick made
                // while the decodes ran owns a different tier, and a leave that
                // landed after the prewarm returned owns none.
                let warmed = ru_covers.swap_tab_covers(entering).then_some(entering);
                let _ = weak_warm.upgrade_in_event_loop(move |ui| {
                    if should_announce_warm(
                        warmed,
                        ru_covers.section_active(),
                        ru_covers.active_tab(),
                    ) {
                        recently_played_ui_mod::mark_covers_warm(&ui);
                    }
                });
            });
        });
    }

    // --- Grid column count ----------------------------------------
    // A resize only changes how the cached cards are chunked, so this re-walks
    // the in-memory cache rather than touching the DB.
    {
        let ru = rp_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::apply_filtered_grid_now(&ui, &ru);
        });
    }
}
