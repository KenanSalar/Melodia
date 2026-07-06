//! The Add-to-Playlist picker's multi-select plumbing (toggle + "Select
//! all", with fully-contained playlists unselectable) and the add-tracks
//! commit fired by the accept dispatcher.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, SharedString};

use super::{
    TOAST_AUTO_DISMISS_MS, add_pick_disabled, clamp_usize, refresh_add_selection_meta,
    set_all_picks, toggle_pick,
};
use crate::library;
use crate::state::AppState;
use crate::ui::notifications::{NotificationParams, NotificationsUi};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{AppWindow, PlaylistPickRow as UiPlaylistPickRow, Playlists, Settings};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // toggle-add-pick: flip one enabled row's `selected`, then recompute the
    // header's select-all + count. Disabled (fully-contained) rows no-op.
    {
        let weak = weak.clone();
        playlists.on_toggle_add_pick(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<crate::Dialog>();
            let pick_total = dlg.get_pick_total_tracks();
            toggle_pick(
                &dlg.get_playlist_pick_rows(),
                id,
                |r: &UiPlaylistPickRow| r.id,
                |r| !add_pick_disabled(r.contained_count, pick_total),
                |r| r.selected = !r.selected,
            );
            refresh_add_selection_meta(&dlg);
        });
    }

    // set-all-add-picks: set every *enabled* row's `selected` to `sel`
    // (fully-contained rows stay unselectable).
    {
        let weak = weak.clone();
        playlists.on_set_all_add_picks(move |sel| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<crate::Dialog>();
            let pick_total = dlg.get_pick_total_tracks();
            set_all_picks(
                &dlg.get_playlist_pick_rows(),
                sel,
                |r: &UiPlaylistPickRow| !add_pick_disabled(r.contained_count, pick_total),
                |r| r.selected,
                |r, v| r.selected = v,
            );
            refresh_add_selection_meta(&dlg);
        });
    }

    // add-tracks-to-selected: fired by the accept dispatcher. Read the
    // selected (enabled) playlist ids + pending tracks synchronously (the
    // dialog closes + clears its model right after this returns), then add the
    // tracks into each selected playlist, refresh, and toast a summary. Runs
    // on the UI thread (`spawn_local` + `Compat`) because the
    // `Rc<NotificationsUi>` isn't `Send`.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        playlists.on_add_tracks_to_selected(move || {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<crate::Dialog>();
            let pick_total = dlg.get_pick_total_tracks();
            let pids: Vec<i64> = dlg
                .get_playlist_pick_rows()
                .iter()
                .filter(|r| r.selected && !add_pick_disabled(r.contained_count, pick_total))
                .map(|r| i64::from(r.id))
                .collect();
            let track_ids: Vec<i64> =
                dlg.get_pending_track_ids().iter().map(i64::from).collect();
            if pids.is_empty() || track_ids.is_empty() {
                return;
            }

            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            let notifications = notifications.clone();
            let _ = slint::spawn_local(Compat::new(async move {
                let requested = pids.len();
                let track_count = track_ids.len();
                let mut ok: usize = 0;
                for pid in &pids {
                    match library::playlists::add_to_playlist(&s, *pid, track_ids.clone()).await
                    {
                        Ok(()) => ok += 1,
                        Err(e) => log::warn!("playlists::add_tracks_to_selected({pid}): {e}"),
                    }
                }

                if ok > 0 {
                    if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await {
                        log::warn!("playlists::add_tracks_to_selected refetch grid: {e}");
                    }
                    let detail_id = pu.detail_playlist_id();
                    if pids.contains(&detail_id)
                        && let Err(e) =
                            playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), detail_id)
                                .await
                    {
                        log::warn!("playlists::add_tracks_to_selected refresh detail: {e}");
                    }
                }

                // Total failure (rare — a DB error on every playlist) is
                // logged above; surface the success/partial result as a toast.
                if ok == 0 {
                    return;
                }
                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                let variant = if ok < requested { "warning" } else { "success" };
                notifications.show_auto_dismiss(
                    NotificationParams {
                        variant: variant.into(),
                        title: settings.invoke_add_to_playlist_title(clamp_usize(ok)),
                        message: settings
                            .invoke_add_to_playlist_message(clamp_usize(track_count)),
                        action_label: SharedString::default(),
                        action_kind: SharedString::default(),
                    },
                    TOAST_AUTO_DISMISS_MS,
                );
            }));
        });
    }
}
