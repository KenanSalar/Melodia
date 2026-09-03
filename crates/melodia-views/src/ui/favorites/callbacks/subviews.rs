//! `Favorites.*` card actions and sub-view routing: Most Played
//! play-track, the cross-tab open-artist hand-off, the tab switch, and the
//! grid's column-count push. See [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{cross_tab_nav, next_sort, persist_view_sort};
use crate::ui::favorites::NAV_FAVORITES;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::model_diff::clear_vec_model;
use crate::ui::tab_bar::{UNFETCHED_COUNT, should_announce_warm};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};
use melodia_app::library;
use melodia_app::state::AppState;

/// Wire the card-action and sub-view-routing callbacks.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // play-track: clicking a Most Played card loads that grid into the queue
    // and starts on that card — the grid is the context, not the Songs list
    // on the other tab. The Slint callback carries no row index (these are
    // cards, not list rows), so the start slot comes from the id.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_play_track(move |id| {
            let id = i64::from(id);
            let ids = fu.most_played_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = ids.iter().position(|&i| i == id);
            let s = s.clone();
            spawn_logged!(
                s,
                "favorites::play_track",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }

    // --- Cross-tab open-artist ------------------------------------
    // Clicking a favorite artist card drills into the Artists tab's
    // Artist Detail; the shared hand-off stamps the origin so the back
    // arrow returns to Favorites. The grid is unmounted the moment Nav
    // flips, so its cover tier goes with it — the same release
    // `on_open_album` does for the Albums grid.
    {
        let s = state.clone();
        let aru = artists_ui.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_open_artist(move |artist_id| {
            let fu_release = fu.clone();
            s.runtime.spawn_blocking(move || fu_release.release_artist_covers());
            cross_tab_nav::open_artist_cross_tab(
                &s,
                &aru,
                &weak,
                i64::from(artist_id),
                cross_tab_nav::Origin::section(NAV_FAVORITES),
                "favorites::open_artist",
            );
        });
    }

    // --- Tab switch -----------------------------------------------
    // The bar has already moved `tab-idx` and cleared the Slint-side
    // filter by the time this runs, so this is the catch-up: drop the Rust
    // filter shadow to match, build the entering tab's model, then off-thread
    // persist the pick and swap the cover tiers over.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        // Ordered: a bounce queues a value per pick and two blocking tasks have
        // none of their own, so a reversed pair reopens the page on a tab the
        // user only passed through.
        let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Favorites>();
            let entering = favorites_ui_mod::tab_from_index(&g, tab);
            persist.publish(tab);
            // A sub-view pick moves no nav index, so `record_current` never
            // hears about it. The bar has already written `tab-idx`, so the tag
            // reads the tab being entered.
            crate::ui::view_tag::log_current(&ui);

            // Shadow first: the model build below and every later fetch decide
            // which model to fill and which tier to warm from it, and both can
            // run off the UI thread where the global isn't reachable.
            fu.set_active_tab(entering);
            favorites_ui_mod::set_filter(&fu, "");

            // Asked *before* the applies below, because they are what has to
            // know: the answer decides whether the counts they write stand for
            // anything. Each tab's fetch only runs while that tab is mounted, so
            // entering one is where a tick skipped on another gets paid for; the
            // flag is what keeps a pick back and forth from re-querying a cache
            // nothing has invalidated. See `lifecycle::kick_full_refresh`.
            let songs = entering == favorites_ui_mod::FavoritesTab::Songs;
            let needs_fetch = if songs {
                fu.take_songs_dirty()
            } else {
                fu.take_grids_dirty()
            };

            // The entering tier was cleared when its tab was last left, so the
            // cards mount cold: hold the lookups at cache-only until the
            // prewarm below reports back, or each visible card drags a grid-tier
            // decode onto this thread in the frame that paints the grid.
            g.set_covers_generation(0);

            // Fill the entering tab's model on this tick — it mounts on the
            // next frame, and the `_now` variant is what makes "this tick"
            // true: the hidden tab's model is empty, and the count that gates
            // its empty state is not, so a grid that paints before its rows
            // land shows a bare panel rather than a placeholder.
            favorites_ui_mod::apply_filtered_grids_now(&ui, &fu);
            // Only when Songs is the tab being entered: rebuilding a list
            // nobody can see costs one prepared row per favourite on this
            // thread, and every entry into Songs comes back through here. The
            // `_now` variant for the reason the grids' is used two lines up —
            // the tab mounts on the next frame, and a posted write can lose that
            // race to a `TrackList` of headers over an emptied model.
            // Leaving it empties that model rather than leaving it holding its
            // last rows — the same trade `write_filtered_grids` makes for the
            // grid it isn't mounting, and for the same reason: a row per
            // favourite is one `TrackListRow` of `SharedString`s pinned behind
            // a tab the user has left. `apply_filtered_tracks` refuses to
            // refill it while that's true, so nothing puts them back.
            if songs {
                favorites_ui_mod::apply_filtered_tracks_now(&ui, &fu);
            } else {
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "favorites: leave songs tab");
            }

            // Spawned *after* the synchronous applies above, so whatever the
            // cache already holds paints on this tick and the fetch refreshes
            // behind it.
            //
            // **And a grid pick has to take its count back, because the cache it
            // just walked is the one this fetch is about to fill.** A `0` there
            // is the one value meaning "there is nothing here" — it mounts
            // `GridEmptyState` over a library that has plenty, for the length of
            // the query plus the cover-decode burst it awaits.
            // `UNFETCHED_COUNT` matches neither `== 0` nor `> 0`, so the panel
            // stays quiet until the fetch answers. Songs owes no equivalent:
            // `Favorites.track-count` is written by `refresh_hero`, which is
            // never gated, so it is answered on every tab either way.
            if needs_fetch {
                match entering {
                    favorites_ui_mod::FavoritesTab::MostPlayed => {
                        g.set_most_played_count(UNFETCHED_COUNT);
                    }
                    favorites_ui_mod::FavoritesTab::Artists => {
                        g.set_artist_count(UNFETCHED_COUNT);
                    }
                    favorites_ui_mod::FavoritesTab::Songs => {}
                }
                let s_fetch = s.clone();
                let fu_fetch = fu.clone();
                let weak_fetch = weak.clone();
                s.runtime.spawn(async move {
                    if songs {
                        if let Err(e) =
                            favorites_ui_mod::refresh_tracks(&s_fetch, &fu_fetch, &weak_fetch).await
                        {
                            log::warn!("favorites::refresh_tracks: {e}");
                        }
                    } else {
                        favorites_ui_mod::refresh_grids(&s_fetch, &fu_fetch, &weak_fetch).await;
                    }
                });
            }

            let fu_covers = fu.clone();
            let s_disk = s.clone();
            let weak_warm = weak.clone();
            let persist_disk = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                // Scoped to the write alone: a superseded tab drops its own disk
                // hop, and the cover swap below is this task's regardless.
                persist_disk.write_if_current(tab, || {
                    if let Err(e) = library::settings::set_favorites_tab(&s_disk, tab) {
                        log::warn!("favorites::set_favorites_tab: {e}");
                    }
                });
                // `warm` is the decode's own verdict — a section leave landing
                // inside it handed the buffers back — so `Some(entering)` is
                // exactly "we decoded for this tab and still hold it", which is
                // what `should_announce_warm` takes. The other two terms are read
                // on the UI thread, where both shadows are written: a pick made
                // while the decodes ran owns a different tier, and a leave that
                // landed after the prewarm returned owns none.
                let warmed = fu_covers.swap_tab_covers(entering).then_some(entering);
                let _ = weak_warm.upgrade_in_event_loop(move |ui| {
                    if should_announce_warm(
                        warmed,
                        fu_covers.section_active(),
                        fu_covers.active_tab(),
                    ) {
                        favorites_ui_mod::mark_covers_warm(&ui);
                    }
                });
            });
        });
    }

    // --- Favorite Artists sort ------------------------------------
    // Re-orders the cached Vec and re-applies, no DB round-trip — the same
    // in-memory path the filter and the column count take. `set_artist_sort`
    // moves the cache with the shadow, which is what keeps the cover prewarm
    // reading the order the cards are actually in.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_request_artist_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Favorites>();
            let (new_field, new_dir) = next_sort(
                g.get_artist_sort_field().as_str(),
                g.get_artist_sort_dir().as_str(),
                &field,
            );
            g.set_artist_sort_field(SharedString::from(new_field.as_str()));
            g.set_artist_sort_dir(SharedString::from(new_dir.as_str()));
            favorites_ui_mod::set_artist_sort(&fu, new_field.clone(), new_dir);
            favorites_ui_mod::apply_filtered_grids_now(&ui, &fu);
            persist_view_sort(&s, view_id::FAVORITE_ARTISTS, new_field, new_dir);
        });
    }

    // --- Grid column count ----------------------------------------
    // A resize only changes how the cached cards are chunked, so this
    // re-walks the in-memory caches rather than touching the DB.
    {
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::apply_filtered_grids_now(&ui, &fu);
        });
    }
}
