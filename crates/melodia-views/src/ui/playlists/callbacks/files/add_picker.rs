//! The Add-to-Playlist picker: its opener, the multi-select plumbing (toggle +
//! "Select all", with fully-contained playlists unselectable) and the add-tracks
//! commit fired by the accept dispatcher.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::{add_pickable, refresh_add_selection_meta, set_all_picks, toggle_pick};
use crate::ui::callbacks::DialogClaim;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::playlists::callbacks::refresh_after_edit;
use crate::ui::shell::notifications::{Completion, NotificationsUi};
use crate::ui::util::{len_as_i32, opt_shared};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, PlaylistPickRow as UiPlaylistPickRow, Playlists, Settings};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    wire_request_add_to_playlist(ui, state);
    wire_toggle_add_pick(ui);
    wire_set_all_add_picks(ui);
    wire_add_tracks_to_selected(ui, state, playlists_ui, notifications);
}

/// The track row's "Add to Playlist" entry (single-row or multi-select). Runs
/// two short SELECTs in parallel (full playlist list + per-playlist overlap
/// counts for the selection), builds `PlaylistPickRow`s, then opens the dialog
/// already populated. `exclude-playlist-id >= 0` is the open Playlist Detail id
/// from the row, so the current playlist doesn't appear as a target.
fn wire_request_add_to_playlist(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_request_add_to_playlist(move |ids, exclude_id| {
        let Some(claim) = DialogClaim::take_from(&weak) else { return };
        let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
        if id_vec.is_empty() {
            return;
        }
        let exclude = i64::from(exclude_id);
        let s = s.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            let (playlists_res, counts_res) = tokio::join!(
                library::playlists::get_playlists(&s.db),
                library::playlists::count_tracks_in_playlists_for_selection(&s.db, id_vec.clone()),
            );
            let playlist_stats = playlists_res.unwrap_or_else(|e| {
                log::warn!("playlists::request_add_to_playlist get_playlists: {}", describe(&e));
                Vec::new()
            });
            let counts = counts_res.unwrap_or_else(|e| {
                log::warn!("playlists::request_add_to_playlist counts: {}", describe(&e));
                HashMap::new()
            });
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if !claim.holds(&ui) {
                    return;
                }
                let rows = pick_rows(playlist_stats, &counts, exclude);
                let dlg = ui.global::<Dialog>();
                dlg.set_playlist_pick_rows(ModelRc::new(VecModel::from(rows)));
                dlg.set_add_select_all(false);
                dlg.set_add_selected_count(0);
                dlg.set_open(true);
            });
        });
    });
}

/// One picker row per playlist the selection can be added to, each counting how
/// much of the selection it already holds.
fn pick_rows(
    playlist_stats: Vec<PlaylistStats>,
    counts: &HashMap<i64, i64>,
    exclude: i64,
) -> Vec<UiPlaylistPickRow> {
    playlist_stats
        .into_iter()
        .filter(|p| p.id != exclude)
        // Smart playlists derive membership from rules — adding
        // tracks would write orphan `playlist_items` rows that
        // never surface. Exclude them as targets (same gate as
        // reorder / remove / file-drop).
        .filter(|p| !p.is_smart)
        .filter_map(|p| {
            // Skip rows whose i64 playlist id can't fit in the
            // Slint-side i32 (`PlaylistPickRow.id`). The picker's
            // toggle / commit route back into Rust by that id
            // (`Playlists.toggle-add-pick` / `add-tracks-to-selected`)
            // — surfacing a row with a clamped id would mis-target.
            let id = i32::try_from(p.id).ok().or_else(|| {
                log::warn!(
                    "playlists::request_add_to_playlist: playlist id {} overflows i32 — skipping",
                    p.id,
                );
                None
            })?;
            let contained = i32::try_from(*counts.get(&p.id).unwrap_or(&0)).unwrap_or(i32::MAX);
            Some(UiPlaylistPickRow {
                id,
                name: SharedString::from(p.name.as_str()),
                artwork_path: opt_shared(p.thumbnail_path.as_deref()),
                contained_count: contained,
                // Multi-select: start with nothing ticked; the
                // user opts in per playlist (or via "Select all").
                selected: false,
            })
        })
        .collect()
}

/// Flip one enabled row's `selected`, then recompute the header's select-all +
/// count. Disabled (fully-contained) rows no-op.
fn wire_toggle_add_pick(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_toggle_add_pick(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        let pick_total = dlg.get_pick_total_tracks();
        toggle_pick(
            &dlg.get_playlist_pick_rows(),
            id,
            |r: &UiPlaylistPickRow| r.id,
            |r| add_pickable(r, pick_total),
            |r| r.selected = !r.selected,
        );
        refresh_add_selection_meta(&dlg);
    });
}

/// Set every *enabled* row's `selected` to `sel` (fully-contained rows stay
/// unselectable).
fn wire_set_all_add_picks(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_set_all_add_picks(move |sel| {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        let pick_total = dlg.get_pick_total_tracks();
        set_all_picks(
            &dlg.get_playlist_pick_rows(),
            sel,
            |r: &UiPlaylistPickRow| add_pickable(r, pick_total),
            |r| r.selected,
            |r, v| r.selected = v,
        );
        refresh_add_selection_meta(&dlg);
    });
}

/// Fired by the accept dispatcher. Read the selected (enabled) playlist ids +
/// pending tracks synchronously (the dialog closes + clears its model right
/// after this returns), then add the tracks into each selected playlist,
/// refresh, and toast a summary. Runs on the UI thread (`spawn_local` +
/// `Compat`) because the `Rc<NotificationsUi>` isn't `Send`.
fn wire_add_tracks_to_selected(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    let notifications = notifications.clone();
    ui.global::<Playlists>().on_add_tracks_to_selected(move || {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        let pick_total = dlg.get_pick_total_tracks();
        let pids: Vec<i64> = dlg
            .get_playlist_pick_rows()
            .iter()
            .filter(|r| r.selected && add_pickable(r, pick_total))
            .map(|r| i64::from(r.id))
            .collect();
        let track_ids: Vec<i64> = dlg.get_pending_track_ids().iter().map(i64::from).collect();
        if pids.is_empty() || track_ids.is_empty() {
            return;
        }

        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            let added = add_to_each(&s, &pids, &track_ids).await;
            if added == 0 {
                // Total failure (rare — a DB error on every playlist) is logged per playlist.
                return;
            }
            refresh_after_edit(&s, &pu, weak.clone(), &pids, "playlists::add_tracks_to_selected")
                .await;

            let Some(ui) = weak.upgrade() else { return };
            let settings = ui.global::<Settings>();
            notifications.show_completion(
                Completion::partial_if(added < pids.len()),
                settings.invoke_add_to_playlist_title(len_as_i32(added)),
                settings.invoke_add_to_playlist_message(len_as_i32(track_ids.len())),
            );
        }));
    });
}

/// Add `track_ids` to every playlist in `pids`, returning how many took them.
async fn add_to_each(state: &AppState, pids: &[i64], track_ids: &[i64]) -> usize {
    let mut added: usize = 0;
    for pid in pids {
        match library::playlists::add_to_playlist(&state.db, *pid, track_ids.to_vec()).await {
            Ok(()) => added += 1,
            Err(e) => {
                log::warn!("playlists::add_tracks_to_selected({pid}): {}", describe(&e));
            }
        }
    }
    added
}
