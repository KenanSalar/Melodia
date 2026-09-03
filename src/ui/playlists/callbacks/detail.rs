//! `PlaylistDetail.*` callbacks: close, play / shuffle / play-row, queue
//! actions, favorite toggle, row selection, in-memory sort, column
//! toggle, drag-reorder, the edit-artwork picker open, and track removal.

use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::{spawn_blocking_logged, spawn_logged, wire_row_flag};
use crate::ui::callbacks::{collect_track_ids, play_row_start, spawn_play_then_shuffle};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, Dialog, PlaylistDetail};

/// Wire the `PlaylistDetail` callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let detail = ui.global::<PlaylistDetail>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<PlaylistDetail>();

            // No `mark_drill_back` here, where its three siblings have one:
            // theirs precedes a `return_to_section` that flips the nav index,
            // and this detail records no origin at all. The body that mounts
            // in its place takes a fixed `below` and reads the global not at
            // all — see `ui::nav_transition`.
            g.set_playlist_id(-1);
            // The hero Images are *not* dropped here. This id is what the band's
            // whole hero half is a ternary over, so releasing on the same tick
            // leaves it collapsing a placeholder — `MyLibrary.hero-collapsed`
            // owns that teardown now, and the band fires it once the morph is
            // done. See `callbacks::my_library::release_collapsed_hero`.
            playlists_ui_mod::clear_detail(&pu);
            // `clear_detail` only reaches the Rust needle; leaving the Slint
            // half set would have the two disagree until the next open, on the
            // property `reorder-enabled` reads.
            g.set_filter(SharedString::new());

            let pu_swap = pu.clone();
            s.runtime.spawn_blocking(move || {
                pu_swap.release_detail_artwork();
                pu_swap.prewarm_visible_covers();
            });

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::PLAYLIST_DETAIL,
                    None,
                ) {
                    log::warn!("playlists::close_detail persist: {e}");
                }
            });

            // Record the post-close state — see the matching call in
            // `albums/detail.rs::on_close_detail` for the rationale.
            crate::ui::nav_history::record_current(&ui);
        });
    }

    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        detail.on_shuffle_all(move || {
            spawn_play_then_shuffle(&s, "playlists::shuffle_all", pu.detail_track_ids());
        });
    }

    // play-row: double-click loads the playlist into the queue and starts on
    // the clicked track.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        detail.on_play_row(move |track_id, idx| {
            let ids = pu.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(
                s,
                "playlists::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }

    {
        let s = state.clone();
        detail.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(
                s,
                "playlists::play_next",
                library::queue::queue_play_next_many(&s, id_vec)
            );
        });
    }

    {
        let s = state.clone();
        detail.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(
                s,
                "playlists::add_to_queue",
                library::queue::queue_add_tracks(&s, id_vec)
            );
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each affected row (rating never changes list membership, so a
    // surgical per-row patch suffices).
    {
        let pu = playlists_ui.clone();
        wire_row_flag!(detail, on_toggle_row_favorite, state, "playlists::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, pu],
        after: |id_vec, fav| {
            for id in &id_vec {
                pu.flip_detail_favorite(*id, fav);
                playlists_ui_mod::apply_detail_row_favorite(&weak, *id, fav);
            }
        });
    }
    {
        let pu = playlists_ui.clone();
        wire_row_flag!(detail, on_set_row_rating, state, "playlists::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, pu],
        after: |id_vec, rating| {
            for id in &id_vec {
                pu.flip_detail_rating(*id, rating);
                playlists_ui_mod::apply_detail_row_rating(&weak, *id, rating);
            }
        });
    }

    {
        let weak = weak.clone();
        let pu = playlists_ui.clone();
        detail.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            playlists_ui_mod::handle_select_row(&ui, &pu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        let pu = playlists_ui.clone();
        detail.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            playlists_ui_mod::clear_selection(&ui, &pu);
        });
    }

    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<PlaylistDetail>();
            // Third click lands back on the curated order, which is the only
            // way back to it — no header cell asks for `"position"`.
            let (new_field, new_dir) = crate::ui::callbacks::next_sort_with_natural(
                g.get_sort_field().as_str(),
                g.get_sort_dir().as_str(),
                &field,
                Some(playlists_ui_mod::POSITION_FIELD),
            );
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            playlists_ui_mod::resort_detail(&ui, &pu);
            crate::ui::callbacks::persist_view_sort(
                &s,
                view_id::PLAYLIST_DETAIL,
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
            let columns = ui.global::<PlaylistDetail>().snapshot_visible();
            let s_disk = s.clone();
            spawn_blocking_logged!(
                s,
                "playlists::toggle_column",
                library::settings::update_view_columns(
                    &s_disk,
                    view_id::PLAYLIST_DETAIL.to_owned(),
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
        let pu = playlists_ui.clone();
        detail.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            playlists_ui_mod::set_filter(&pu, text.as_str());
            playlists_ui_mod::apply_filtered_detail(&ui, &pu);
        });
    }

    // reorder: drag-released a row. Optimistically permute the visible
    // Vec + position-order cache, then call `library::playlists::reorder_playlist`.
    // Rollback on DB error.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_reorder(move |from, to| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(from_u) = usize::try_from(from) else {
                return;
            };
            let Ok(to_u) = usize::try_from(to) else {
                return;
            };
            if from_u == to_u {
                return;
            }
            let playlist_id = pu.detail_playlist_id();
            if playlist_id < 0 {
                return;
            }
            let Some(snapshot) = playlists_ui_mod::apply_optimistic_reorder(&ui, &pu, from_u, to_u)
            else {
                return;
            };
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playlists::reorder_playlist(&s, playlist_id, from, to).await
                {
                    log::warn!("playlists::reorder: {e}");
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        playlists_ui_mod::rollback_reorder(&ui, &pu, snapshot);
                    });
                }
            });
        });
    }

    // request-edit-artwork: populate the mosaic candidates from the
    // playlist's own track artworks and open the picker dialog. Seeds
    // `current-artwork` from `PlaylistDetail.cover` (already decoded
    // for the open detail view) so the dialog opens on a preview of
    // the saved state, and resets `mosaic-touched` so the dispatcher
    // treats an immediate Apply as a no-op.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_request_edit_artwork(move || {
            let id = pu.detail_playlist_id();
            if id < 0 {
                return;
            }
            let s = s.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let candidates = library::playlists::get_playlist_artwork_paths(&s, id)
                    .await
                    .unwrap_or_default();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let dlg = ui.global::<Dialog>();
                    let current_cover = ui.global::<PlaylistDetail>().get_cover();
                    dlg.set_title(SharedString::from("Edit Artwork"));
                    dlg.set_message(SharedString::from(""));
                    dlg.set_confirm_label(SharedString::from("Apply"));
                    dlg.set_cancel_label(SharedString::from("Cancel"));
                    dlg.set_destructive(false);
                    dlg.set_kind(SharedString::from("edit-playlist-artwork"));
                    dlg.set_target_id(i32::try_from(id).unwrap_or(-1));
                    dlg.set_input_text(SharedString::from(""));
                    dlg.set_mosaic_selection(ModelRc::new(VecModel::from(
                        Vec::<SharedString>::new(),
                    )));
                    dlg.set_mosaic_touched(false);
                    dlg.set_current_artwork(current_cover);
                    let cand_rows: Vec<SharedString> =
                        candidates.into_iter().map(SharedString::from).collect();
                    dlg.set_mosaic_candidates(ModelRc::new(VecModel::from(cand_rows)));
                    dlg.set_open(true);
                });
            });
        });
    }

    // Rename / Delete dialog opens are populated inline in
    // `my-library/tab-pills.slint` (Slint-only — see the long comment
    // in `super::dialog`). No Rust handler needed here; the Accept-side
    // commit lives in the `rename-playlist` / `delete-playlist`
    // branches in `super::dialog`.

    // remove-selected: batch remove from the open playlist.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_remove_selected(move || {
            let Some(ui) = weak.upgrade() else { return };
            let id = pu.detail_playlist_id();
            if id < 0 {
                return;
            }
            let g = ui.global::<PlaylistDetail>();
            let ids: Vec<i64> = g.get_selected_ids().iter().map(i64::from).collect();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playlists::remove_tracks_from_playlist_batch(&s, id, ids).await
                {
                    log::warn!("playlists::remove_selected({id}): {e}");
                    return;
                }
                let pu_ui = pu.clone();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    playlists_ui_mod::clear_selection(&ui, &pu_ui);
                });
                if let Err(e) = playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), id).await {
                    log::warn!("playlists::remove_selected refresh: {e}");
                }
                if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak).await {
                    log::warn!("playlists::remove_selected refetch grid: {e}");
                }
            });
        });
    }

    // remove-track: row-context-menu removal. Single-row mode sends a
    // 1-element array; multi-select sends the entire selection. The
    // batch path goes through `remove_tracks_from_playlist_batch` so
    // every shape collapses to one DB round-trip.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        detail.on_remove_track(move |track_ids| {
            let id_vec = collect_track_ids(&track_ids);
            let playlist_id = pu.detail_playlist_id();
            if playlist_id < 0 || id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playlists::remove_tracks_from_playlist_batch(&s, playlist_id, id_vec)
                        .await
                {
                    log::warn!("playlists::remove_track: {e}");
                    return;
                }
                if let Err(e) =
                    playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), playlist_id).await
                {
                    log::warn!("playlists::remove_track refresh: {e}");
                }
                if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak).await {
                    log::warn!("playlists::remove_track refetch grid: {e}");
                }
            });
        });
    }
}
