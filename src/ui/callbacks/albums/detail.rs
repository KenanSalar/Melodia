//! `AlbumDetail.*` callbacks: close, play / shuffle / play-row, queue
//! actions, favorite toggle, row selection, in-memory sort, column toggle.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::{collect_track_ids, play_row_start, spawn_play_then_shuffle};
use crate::ui::callbacks::macros::{
    release_detail_hero_images, spawn_logged, spawn_logged_sync, wire_row_flag,
};
use crate::ui::my_library::restore_origin;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AlbumDetail, AppWindow};

/// Wire the `AlbumDetail` callbacks. See [`super::wire_albums`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let detail = ui.global::<AlbumDetail>();
    let weak = ui.as_weak();

    // close-detail: the header's back button. Flip back to the grid and
    // drop the cached detail state. Clears the Albums entry in
    // `views.json`'s `last_detail_ids` so the next launch lands on the grid
    // (not the just-closed album). The detail-tier `(cover, blur)` cache
    // is released off-thread (the detail view is unmounted by the flip),
    // and the grid cover cache is re-warmed so visible cards are cache
    // hits when the grid mounts — mirrors the section-exit/re-enter pair.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        detail.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<AlbumDetail>();

            // View-transition direction: `Left` = returning from a detail.
            // Set before any property write that flips the `if` branch —
            // for cross-tab origin restores that's the `selected-index`
            // write below; for same-tab back it's the `album-id = -1`
            // write a few lines down. One up-front set covers both.
            crate::ui::nav_transition::mark_drill_back(&ui);

            // If cross-tab nav opened this detail (currently only
            // `wire_artists::on_open_album`), restore where the user came
            // from in the same UI-thread tick as the `album-id` reset so
            // the Slint conditional reroutes straight to the origin's
            // detail view without an Albums-grid frame.
            // `ArtistDetail.artist-id` was preserved through the cross-tab
            // episode, so `tab-idx == tab-artists && artist-id >= 0`
            // mounts `ArtistDetailView` immediately. **The origin is a
            // pair**: five views share nav index 3, so restoring the index
            // alone would be a no-op leaving the Albums tab mounted.
            let origin = g.get_origin_nav_index();
            let origin_was_cross_tab = origin >= 0;
            if origin_was_cross_tab {
                restore_origin(&ui, origin, g.get_origin_tab());
                g.set_origin_nav_index(-1);
                g.set_origin_tab(-1);
            }

            g.set_album_id(-1);
            // Drop the hero Image properties so their `SharedPixelBuffer`s
            // release the Arc the LRU is about to clear too.
            release_detail_hero_images!(ui, g);
            albums_ui_mod::clear_detail(&au);

            let au_swap = au.clone();
            s.runtime.spawn_blocking(move || {
                au_swap.release_detail_artwork();
                // Skip the Albums-grid prewarm on the cross-tab back path:
                // the grid isn't going to mount (Slint swings to
                // `ArtistDetailView`), and `AlbumsUi.grid_covers` is
                // shared with the Artist Detail's Albums sub-section —
                // prewarming would evict that sub-section's still-needed
                // thumbnails in favor of grid covers the user won't see.
                if !origin_was_cross_tab {
                    au_swap.prewarm_visible_covers();
                }
            });

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::ALBUM_DETAIL,
                    None,
                ) {
                    log::warn!("albums::close_detail persist: {e}");
                }
            });

            // Record the post-close state (cross-tab origin restore may
            // have flipped `Nav.selected-index` above, so read it here
            // rather than assuming Albums). No-op while a replay is in
            // flight — the replay invokes this very callback to drive
            // back/forward, and the suppress gate stops it from
            // re-recording the entry we already walked to.
            crate::ui::nav_history::record_current(&s, &ui);
        });
    }

    {
        let s = state.clone();
        let au = albums_ui.clone();
        detail.on_shuffle_album(move || {
            spawn_play_then_shuffle(&s, "albums::shuffle_album", au.detail_track_ids());
        });
    }

    // play-row: double-click loads the album into the queue and starts on the
    // clicked track.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        detail.on_play_row(move |track_id, idx| {
            let ids = au.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "albums::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }

    {
        let s = state.clone();
        detail.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "albums::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        detail.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "albums::add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each affected row (no list re-fetch — scroll position holds and
    // there's no flash). Single-row and multi-select both arrive as `[int]`;
    // rating never changes list membership.
    {
        let au = albums_ui.clone();
        wire_row_flag!(detail, on_toggle_row_favorite, state, "albums::set_favorite",
            library::favorites::set_favorite, collect_track_ids,
            captures: [weak, au],
            after: |id_vec, fav| {
                for id in &id_vec {
                    au.flip_detail_favorite(*id, fav);
                    albums_ui_mod::apply_detail_row_favorite(&weak, *id, fav);
                }
            });
    }
    {
        let au = albums_ui.clone();
        wire_row_flag!(detail, on_set_row_rating, state, "albums::set_rating",
            library::ratings::set_rating, collect_track_ids,
            captures: [weak, au],
            after: |id_vec, rating| {
                for id in &id_vec {
                    au.flip_detail_rating(*id, rating);
                    albums_ui_mod::apply_detail_row_rating(&weak, *id, rating);
                }
            });
    }

    // select-row / clear-selection: modifier-aware selection, mirroring the
    // Tracks view. The new selected set is computed in Rust.
    {
        let weak = weak.clone();
        let au = albums_ui.clone();
        detail.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            albums_ui_mod::handle_select_row(&ui, &au, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        let au = albums_ui.clone();
        detail.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            albums_ui_mod::clear_selection(&ui, &au);
        });
    }

    // request-sort: clicking a track-table header. Same field flips dir; a
    // new field resets to ascending. Album detail sorts in-memory; the new
    // sort is persisted (shared by every album, restored across restarts).
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        detail.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<AlbumDetail>();
            let (new_field, new_dir) = crate::ui::callbacks::next_sort(
                g.get_sort_field().as_str(),
                g.get_sort_dir().as_str(),
                &field,
            );
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            albums_ui_mod::resort_detail(&ui, &au);
            crate::ui::callbacks::persist_view_sort(
                &s,
                view_id::ALBUM_DETAIL,
                new_field,
                new_dir,
            );
        });
    }

    // toggle-column: the popup already flipped the matching `show-*` flag
    // for instant feedback. Persist the new visible-column list under the
    // `"album_detail"` settings key — separate from Tracks / Browse.
    {
        let s = state.clone();
        let weak = weak.clone();
        detail.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<AlbumDetail>().snapshot_visible();
            let s = s.clone();
            spawn_logged_sync!(s, "albums::toggle_column",
                library::settings::update_view_columns(&s, "album_detail".to_string(), columns));
        });
    }

    // filter-changed: re-walk the cached tracks through the new needle
    // and push a filtered Slint model. In-memory walk, no DB round-trip.
    // Mirrors `ArtistDetail.on_filter_changed`.
    {
        let weak = weak.clone();
        let au = albums_ui.clone();
        detail.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            albums_ui_mod::set_filter(&au, text.as_str());
            albums_ui_mod::apply_filtered_detail(&ui, &au);
        });
    }
}
