//! `ArtistDetail.*` callbacks: close, play / shuffle / play-row, queue
//! actions, favorite toggle, row selection, in-memory sort, column
//! toggle, in-memory filter, and the Albums sub-section collapse toggle.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::callbacks::{collect_track_ids, play_row_start, spawn_play_then_shuffle};
use crate::ui::callbacks::macros::{
    release_detail_hero_images, spawn_logged, spawn_logged_sync, wire_row_flag,
};
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, ArtistDetail, Nav};

/// Wire the `ArtistDetail` callbacks. See [`super::wire_artists`].
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    albums_ui: &Arc<AlbumsUi>,
) {
    let detail = ui.global::<ArtistDetail>();
    let weak = ui.as_weak();

    // close-detail: back to the grid + drop cached detail + clear the
    // persisted detail id so the next launch lands on the grid. Same
    // cross-tab origin pattern as `albums::on_close_detail` — if the
    // detail was opened from another tab (currently only Favorites,
    // index 2), flip `Nav.selected-index` back to that tab in the same
    // UI-thread tick as the `artist-id` reset so the Slint conditional
    // reroutes straight to the origin tab without an Artists-grid
    // frame in between.
    {
        let s = state.clone();
        let au = artists_ui.clone();
        let weak = weak.clone();
        let au_albums = albums_ui.clone();
        detail.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<ArtistDetail>();

            // View-transition direction: `Left` = returning from a detail.
            // Set before any property write that flips the `if` branch —
            // for cross-tab origin restores that's the `selected-index`
            // write below; for same-tab back it's the `artist-id = -1`
            // write further down. One up-front set covers both.
            crate::ui::nav_transition::mark_drill_back(&ui);

            let origin = g.get_origin_nav_index();
            let origin_was_cross_tab = origin >= 0;
            if origin_was_cross_tab {
                let nav = ui.global::<Nav>();
                nav.set_selected_index(origin);
                nav.invoke_persist_selected_index(origin);
                g.set_origin_nav_index(-1);
            }

            g.set_artist_id(-1);
            // Drop the hero Image properties so their `SharedPixelBuffer`s
            // release the Arc the LRU is about to clear too. Without this,
            // ~650 KiB of CPU buffer + ~1.5 MiB of mapped GPU texture stay
            // pinned on the global until another detail page overwrites it.
            release_detail_hero_images!(ui, g);
            // Clear the SearchBar so the next open lands on the full
            // tracks + albums set, not a stale needle from the last
            // detail. `clear_detail` drops the Rust-side mirror too.
            g.set_filter(SharedString::from(""));
            artists_ui_mod::clear_detail(&au);

            let au_swap = au.clone();
            let albums_swap = au_albums.clone();
            s.runtime.spawn_blocking(move || {
                au_swap.release_detail_artwork();
                // Skip the Artists-grid prewarm on the cross-tab back
                // path: the grid isn't going to mount (Slint swings to
                // the origin tab's view), and warming Artists covers
                // the user won't see wastes cache pressure on tiles
                // the destination tab may actually need.
                if !origin_was_cross_tab {
                    au_swap.prewarm_visible_covers();
                }
                // Free the Albums sub-section's cover thumbnails (they
                // sat in `AlbumsUi.grid_covers`, borrowed for this
                // detail page). The next artist-detail open will
                // re-fetch what it needs.
                albums_swap.release_grid_covers();
            });

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::ARTIST_DETAIL,
                    None,
                ) {
                    log::warn!("artists::close_detail persist: {e}");
                }
            });

            // Record the post-close state — see the matching call in
            // `albums/detail.rs::on_close_detail` for the rationale.
            crate::ui::nav_history::record_current(&s, &ui);
        });
    }

    {
        let s = state.clone();
        let au = artists_ui.clone();
        detail.on_shuffle_artist(move || {
            spawn_play_then_shuffle(&s, "artists::shuffle_artist", au.detail_track_ids());
        });
    }

    // play-row: double-click loads the artist's tracks into the queue and
    // starts on the clicked one.
    {
        let s = state.clone();
        let au = artists_ui.clone();
        detail.on_play_row(move |track_id, idx| {
            let ids = au.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "artists::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }

    {
        let s = state.clone();
        detail.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "artists::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        detail.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "artists::add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each affected row (rating never changes list membership, so a
    // surgical per-row patch suffices).
    {
        let au = artists_ui.clone();
        wire_row_flag!(detail, on_toggle_row_favorite, state, "artists::set_favorite",
            library::favorites::set_favorite, collect_track_ids,
            captures: [weak, au],
            after: |id_vec, fav| {
                for id in &id_vec {
                    au.flip_detail_favorite(*id, fav);
                    artists_ui_mod::apply_detail_row_favorite(&weak, *id, fav);
                }
            });
    }
    {
        let au = artists_ui.clone();
        wire_row_flag!(detail, on_set_row_rating, state, "artists::set_rating",
            library::ratings::set_rating, collect_track_ids,
            captures: [weak, au],
            after: |id_vec, rating| {
                for id in &id_vec {
                    au.flip_detail_rating(*id, rating);
                    artists_ui_mod::apply_detail_row_rating(&weak, *id, rating);
                }
            });
    }

    {
        let weak = weak.clone();
        let au = artists_ui.clone();
        detail.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            artists_ui_mod::handle_select_row(&ui, &au, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        let au = artists_ui.clone();
        detail.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            artists_ui_mod::clear_selection(&ui, &au);
        });
    }

    {
        let s = state.clone();
        let au = artists_ui.clone();
        let weak = weak.clone();
        detail.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<ArtistDetail>();
            let (new_field, new_dir) = crate::ui::callbacks::next_sort(
                g.get_sort_field().as_str(),
                g.get_sort_dir().as_str(),
                &field,
            );
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            artists_ui_mod::resort_detail(&ui, &au);
            crate::ui::callbacks::persist_view_sort(
                &s,
                view_id::ARTIST_DETAIL,
                new_field,
                new_dir,
            );
        });
    }

    {
        let s = state.clone();
        let weak = weak.clone();
        detail.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<ArtistDetail>().snapshot_visible();
            let s = s.clone();
            spawn_logged_sync!(s, "artists::toggle_column",
                library::settings::update_view_columns(&s, "artist_detail".to_string(), columns));
        });
    }

    // filter-changed: re-walk the cached tracks + albums through the
    // new needle and push filtered Slint models. In-memory walk, no
    // DB round-trip. Mirrors `Favorites.on_filter_changed` (which
    // additionally re-filters the Most Played + Artist strips —
    // Artist Detail has no carousels, just tracks + albums).
    {
        let au = artists_ui.clone();
        let weak = weak.clone();
        detail.on_filter_changed(move |text| {
            artists_ui_mod::set_filter(&au, text.as_str());
            artists_ui_mod::apply_filtered_detail(&weak, &au);
        });
    }

    // toggle-albums-collapsed: flip the per-section flag synchronously
    // so the UI repaints this frame, then persist to
    // `views.json`'s `artist_albums_collapsed` so the next launch
    // restores the same state. Collapsing also drops the borrowed
    // grid-tier cover LRU (`AlbumsUi.grid_covers`): the
    // `if !albums-collapsed` gate has unmounted the scroller, so its
    // 448 px covers are no longer visible or queried via
    // `request-album-cover`. Safe because while Artist Detail is open
    // the Albums tab is not the active section — `grid_covers` holds
    // only this strip's covers. Re-expanding re-decodes them lazily.
    {
        let s = state.clone();
        let weak = weak.clone();
        let au_albums = albums_ui.clone();
        detail.on_toggle_albums_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<ArtistDetail>();
            let new_state = !g.get_albums_collapsed();
            g.set_albums_collapsed(new_state);
            if new_state {
                let au_albums = au_albums.clone();
                s.runtime.spawn_blocking(move || au_albums.release_grid_covers());
            }
            let s = s.clone();
            spawn_logged_sync!(s, "artists::set_albums_collapsed",
                library::settings::set_artist_albums_collapsed(&s, new_state));
        });
    }
}
