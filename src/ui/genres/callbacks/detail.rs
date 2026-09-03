//! `GenreDetail.*` callbacks: close, play / shuffle / play-row, queue
//! actions, favorite toggle, row selection, in-memory sort, column toggle,
//! filter. Genres have no artwork, so close-detail releases the cached track
//! list + selection and the hero colour set — but no `(cover, blur)` pair.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::{spawn_blocking_logged, spawn_logged, wire_row_flag};
use crate::ui::callbacks::{collect_track_ids, play_row_start, spawn_play_then_shuffle};
use crate::ui::callbacks::{next_sort, persist_view_sort};
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::my_library::return_to_section;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, GenreDetail};

/// Wire the `GenreDetail` callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let detail = ui.global::<GenreDetail>();
    let weak = ui.as_weak();

    // close-detail: the header's back button. Flip back to the grid
    // and drop the cached detail state. Clears the Genres entry in
    // `views.json`'s `last_detail_ids` so the next launch lands on the
    // grid (not the just-closed genre). `clear_detail` drops the
    // cached `Vec<TrackListRow>` + selection `HashSet`; the trim that
    // follows hands the now-empty arena pages back to glibc (mirrors
    // the `release_detail_artwork()` + trim that fires on Album /
    // Artist close-detail — there's no `(cover, blur)` pair to clear
    // here, but a long detail track list can still hold tens of
    // `SharedString`s per row, so the bulk-free + trim is worth doing).
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        detail.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<GenreDetail>();
            // View-transition direction: `Left` = returning from a detail.
            // The cross-section close alone samples it — the `selected-index`
            // write below. See `albums/detail.rs`'s note for why the same-page
            // back reads nothing.
            crate::ui::nav_transition::mark_drill_back(&ui);

            // If another *section* opened this detail, return to it in the same
            // UI-thread tick as the `genre-id` reset so Slint reroutes straight
            // there. A drill from a sibling tab records no origin — see
            // `albums/detail.rs`'s note and `cross_tab_nav::origin_stamp`.
            let origin = g.get_origin_nav_index();
            if origin >= 0 {
                return_to_section(&ui, origin);
                g.set_origin_nav_index(-1);
            }

            g.set_genre_id(-1);
            // The shared colour set and chip row are *not* handed back here.
            // This id is what the band's whole hero half is a ternary over, so
            // resetting on the same tick leaves it collapsing a placeholder over
            // a gradient that is no longer this genre's —
            // `MyLibrary.hero-collapsed` owns that teardown now, and the band
            // fires it once the morph is done. See
            // `callbacks::my_library::release_collapsed_hero`.
            genres_ui_mod::clear_detail(&gu);

            let gu_trim = gu.clone();
            s.runtime.spawn_blocking(move || gu_trim.release_caches());

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_last_detail_id(&s_disk, view_id::GENRE_DETAIL, None)
                {
                    log::warn!("genres::close_detail persist: {e}");
                }
            });

            // Record the post-close state — see the matching call in
            // `albums/detail.rs::on_close_detail` for the rationale.
            crate::ui::nav_history::record_current(&ui);
        });
    }

    {
        let s = state.clone();
        let gu = genres_ui.clone();
        detail.on_shuffle_genre(move || {
            spawn_play_then_shuffle(&s, "genres::shuffle_genre", gu.detail_track_ids());
        });
    }

    // play-row: double-click loads every *visible* track into the queue and
    // starts on the clicked one — when a search filter is active that is the
    // filtered subset, not the whole genre.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        detail.on_play_row(move |track_id, idx| {
            let ids = gu.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(
                s,
                "genres::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }

    {
        let s = state.clone();
        detail.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "genres::play_next", library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        detail.on_add_to_queue(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "genres::add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each affected row (no list re-fetch — scroll position holds and
    // there's no flash). Single-row and multi-select both arrive as `[int]`;
    // rating never changes list membership.
    {
        let gu = genres_ui.clone();
        wire_row_flag!(detail, on_toggle_row_favorite, state, "genres::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, gu],
        after: |id_vec, fav| {
            for id in &id_vec {
                gu.flip_detail_favorite(*id, fav);
                genres_ui_mod::apply_detail_row_favorite(&weak, *id, fav);
            }
        });
    }
    {
        let gu = genres_ui.clone();
        wire_row_flag!(detail, on_set_row_rating, state, "genres::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, gu],
        after: |id_vec, rating| {
            for id in &id_vec {
                gu.flip_detail_rating(*id, rating);
                genres_ui_mod::apply_detail_row_rating(&weak, *id, rating);
            }
        });
    }

    // select-row / clear-selection: modifier-aware selection,
    // mirroring the Tracks / Albums views. The new selected set is
    // computed in Rust.
    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::handle_select_row(&ui, &gu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::clear_selection(&ui, &gu);
        });
    }

    // request-sort: clicking a track-table header. Same field flips
    // dir; a new field resets to ascending. Genre detail sorts
    // in-memory; the new sort is persisted (shared by every genre).
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        detail.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<GenreDetail>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            genres_ui_mod::resort_detail(&ui, &gu);
            persist_view_sort(&s, view_id::GENRE_DETAIL, new_field, new_dir);
        });
    }

    // toggle-column: the popup already flipped the matching `show-*`
    // flag for instant feedback. Persist the new visible-column list
    // under the `"genre_detail"` settings key.
    {
        let s = state.clone();
        let weak = weak.clone();
        detail.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<GenreDetail>().snapshot_visible();
            let s = s.clone();
            spawn_blocking_logged!(
                s,
                "genres::toggle_column",
                library::settings::update_view_columns(
                    &s,
                    view_id::GENRE_DETAIL.to_owned(),
                    columns
                )
            );
        });
    }

    // filter-changed: re-walk the cached tracks through the new needle
    // and push a filtered Slint model. In-memory walk, no DB round-trip.
    // Mirrors `ArtistDetail.on_filter_changed`.
    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::set_filter(&gu, text.as_str());
            genres_ui_mod::apply_filtered_detail(&ui, &gu);
        });
    }
}
