//! Playlist import/export (Extended-M3U8) callbacks — the Import / Export
//! header pills on the Playlists view — plus the Add-to-Playlist picker's
//! multi-select toggles + commit (they share the Export picker's
//! model-mutation + selection-meta pattern, and the commit needs the
//! `Rc<NotificationsUi>` for its completion toast).
//!
//! All native dialogs run on the UI thread via
//! `slint::spawn_local(Compat::new(...))` (`Compat` supplies a tokio reactor
//! so the awaited sqlx calls work); after each `.await` the future resumes on
//! the UI thread, so pushing toasts through the `Rc<NotificationsUi>` and
//! reading/writing `Dialog.*` is safe without an extra event-loop hop.
//!
//! Wired separately from [`super::wire_playlists`] (in `main.rs`, after the
//! notifications stack exists) because these handlers need the
//! `Rc<NotificationsUi>` — see [`wire`].

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::library;
use crate::state::AppState;
use crate::ui::notifications::{NotificationParams, NotificationsUi};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{
    AppWindow, Dialog, PlaylistExportPickRow as UiPlaylistExportPickRow,
    PlaylistPickRow as UiPlaylistPickRow, Playlists, Settings,
};

/// Transient import/export confirmation toasts auto-dismiss after this long.
/// Error toasts stay sticky so a failure isn't missed.
const TOAST_AUTO_DISMISS_MS: u32 = 3000;

/// Wire the `Playlists.*` import/export callbacks. Call once after both the
/// `playlists_ui` handle and the notifications stack exist.
pub fn wire(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // import-playlists: native multi-file picker → import each file into a
    // new playlist → refresh the grid → summary toast.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        playlists.on_import_playlists(move || {
            let s = s.clone();
            let pu = pu.clone();
            let weak = weak.clone();
            let notifications = notifications.clone();
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = {
                    let mut d = rfd::AsyncFileDialog::new()
                        .set_title("Import Playlists")
                        .add_filter("Playlists", &["m3u8", "m3u"]);
                    if let Some(ui) = weak.upgrade() {
                        d = d.set_parent(&ui.window().window_handle());
                    }
                    d
                };
                let Some(handles) = dialog.pick_files().await else {
                    return;
                };
                if handles.is_empty() {
                    return;
                }

                // Aggregate across every picked file.
                let mut imported: u32 = 0; // playlists actually created
                let mut tracks: u32 = 0; // matched (path + hash)
                let mut missing: u32 = 0;
                let mut failures: u32 = 0;
                for handle in &handles {
                    match library::playlist_files::import_playlist_from_file(&s, handle.path())
                        .await
                    {
                        Ok(r) => {
                            imported = imported.saturating_add(1);
                            tracks =
                                tracks.saturating_add(r.matched_by_path + r.matched_by_hash);
                            missing = missing.saturating_add(r.missing);
                        }
                        Err(e) => {
                            failures = failures.saturating_add(1);
                            log::warn!(
                                "import_playlist_from_file {}: {e}",
                                handle.path().display()
                            );
                        }
                    }
                }

                if imported > 0
                    && let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await
                {
                    log::warn!("fetch_grid after playlist import: {e}");
                }

                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                if imported == 0 {
                    notifications.show(NotificationParams {
                        variant: "error".into(),
                        title: settings.invoke_playlist_import_failed_title(),
                        message: settings.invoke_playlist_import_failed_message(),
                        action_label: SharedString::default(),
                        action_kind: SharedString::default(),
                    });
                } else {
                    let variant = if missing > 0 || failures > 0 {
                        "warning"
                    } else {
                        "success"
                    };
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: variant.into(),
                            title: settings.invoke_playlist_import_title(clamp_u32(imported)),
                            message: settings.invoke_playlist_import_message(
                                clamp_u32(tracks),
                                clamp_u32(missing),
                            ),
                            action_label: SharedString::default(),
                            action_kind: SharedString::default(),
                        },
                        TOAST_AUTO_DISMISS_MS,
                    );
                }
            }));
        });
    }

    // request-export-playlists: fetch all playlists, fill the picker model
    // (none selected — the user opts in), then open the dialog (chrome was
    // set inline in Slint).
    {
        let s = state.clone();
        let weak = weak.clone();
        playlists.on_request_export_playlists(move || {
            let s = s.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let stats = library::playlists::get_playlists(&s).await.unwrap_or_else(|e| {
                    log::warn!("request_export_playlists get_playlists: {e}");
                    Vec::new()
                });
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let rows: Vec<UiPlaylistExportPickRow> = stats
                        .into_iter()
                        .filter_map(|p| {
                            let id = i32::try_from(p.id).ok().or_else(|| {
                                log::warn!(
                                    "request_export_playlists: playlist id {} overflows i32 — skipping",
                                    p.id,
                                );
                                None
                            })?;
                            Some(UiPlaylistExportPickRow {
                                id,
                                name: SharedString::from(p.name.as_str()),
                                artwork_path: SharedString::from(
                                    p.thumbnail_path.as_deref().unwrap_or(""),
                                ),
                                track_count: p.track_count,
                                // Start with nothing selected — the user opts in
                                // per playlist (or via "Select all").
                                selected: false,
                            })
                        })
                        .collect();
                    let dlg = ui.global::<Dialog>();
                    dlg.set_export_pick_rows(ModelRc::new(VecModel::from(rows)));
                    dlg.set_export_select_all(false);
                    dlg.set_export_selected_count(0);
                    dlg.set_open(true);
                });
            });
        });
    }

    // export-selected-playlists: fired by the accept dispatcher. Read the
    // selected ids synchronously (the dialog closes + clears its model right
    // after this returns), then pick a folder and write the files.
    {
        let s = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        playlists.on_export_selected_playlists(move || {
            let Some(ui) = weak.upgrade() else { return };
            let ids: Vec<i64> = ui
                .global::<Dialog>()
                .get_export_pick_rows()
                .iter()
                .filter(|r| r.selected)
                .map(|r| i64::from(r.id))
                .collect();
            if ids.is_empty() {
                return;
            }

            let s = s.clone();
            let weak = weak.clone();
            let notifications = notifications.clone();
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = {
                    let mut d = rfd::AsyncFileDialog::new()
                        .set_title("Export Playlists To Folder");
                    if let Some(ui) = weak.upgrade() {
                        d = d.set_parent(&ui.window().window_handle());
                    }
                    d
                };
                let Some(folder) = dialog.pick_folder().await else {
                    return;
                };
                let folder_path = folder.path().to_path_buf();

                let result =
                    library::playlist_files::export_playlists_to_folder(&s, &ids, &folder_path)
                        .await;

                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                match result {
                    Ok(res) if res.exported > 0 => {
                        let variant = if res.failed.is_empty() {
                            "success"
                        } else {
                            "warning"
                        };
                        notifications.show_auto_dismiss(
                            NotificationParams {
                                variant: variant.into(),
                                title: settings
                                    .invoke_playlist_export_title(clamp_u32(res.exported)),
                                message: settings.invoke_playlist_export_message(
                                    SharedString::from(res.folder.as_str()),
                                ),
                                action_label: SharedString::default(),
                                action_kind: SharedString::default(),
                            },
                            TOAST_AUTO_DISMISS_MS,
                        );
                    }
                    Ok(_) => {
                        notifications.show(NotificationParams {
                            variant: "error".into(),
                            title: settings.invoke_playlist_export_failed_title(),
                            message: settings.invoke_playlist_export_failed_message(),
                            action_label: SharedString::default(),
                            action_kind: SharedString::default(),
                        });
                    }
                    Err(e) => {
                        log::warn!("export_playlists_to_folder: {e}");
                        notifications.show(NotificationParams {
                            variant: "error".into(),
                            title: settings.invoke_playlist_export_failed_title(),
                            message: settings.invoke_playlist_export_failed_message(),
                            action_label: SharedString::default(),
                            action_kind: SharedString::default(),
                        });
                    }
                }
            }));
        });
    }

    // toggle-export-pick: flip one row's `selected`, then recompute the
    // header's select-all + count.
    {
        let weak = weak.clone();
        playlists.on_toggle_export_pick(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            let model = dlg.get_export_pick_rows();
            if let Some(vm) = model
                .as_any()
                .downcast_ref::<VecModel<UiPlaylistExportPickRow>>()
            {
                for i in 0..vm.row_count() {
                    if let Some(mut row) = vm.row_data(i)
                        && row.id == id
                    {
                        row.selected = !row.selected;
                        vm.set_row_data(i, row);
                        break;
                    }
                }
            }
            refresh_export_selection_meta(&dlg);
        });
    }

    // set-all-export-picks: set every row's `selected` to `sel`.
    {
        let weak = weak.clone();
        playlists.on_set_all_export_picks(move |sel| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            let model = dlg.get_export_pick_rows();
            if let Some(vm) = model
                .as_any()
                .downcast_ref::<VecModel<UiPlaylistExportPickRow>>()
            {
                for i in 0..vm.row_count() {
                    if let Some(mut row) = vm.row_data(i)
                        && row.selected != sel
                    {
                        row.selected = sel;
                        vm.set_row_data(i, row);
                    }
                }
            }
            refresh_export_selection_meta(&dlg);
        });
    }

    // toggle-add-pick: flip one enabled row's `selected`, then recompute the
    // header's select-all + count. Disabled (fully-contained) rows no-op.
    {
        let weak = weak.clone();
        playlists.on_toggle_add_pick(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            let pick_total = dlg.get_pick_total_tracks();
            let model = dlg.get_playlist_pick_rows();
            if let Some(vm) = model
                .as_any()
                .downcast_ref::<VecModel<UiPlaylistPickRow>>()
            {
                for i in 0..vm.row_count() {
                    if let Some(mut row) = vm.row_data(i)
                        && row.id == id
                    {
                        if !add_pick_disabled(row.contained_count, pick_total) {
                            row.selected = !row.selected;
                            vm.set_row_data(i, row);
                        }
                        break;
                    }
                }
            }
            refresh_add_selection_meta(&dlg);
        });
    }

    // set-all-add-picks: set every *enabled* row's `selected` to `sel`
    // (fully-contained rows stay unselectable).
    {
        let weak = weak.clone();
        playlists.on_set_all_add_picks(move |sel| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            let pick_total = dlg.get_pick_total_tracks();
            let model = dlg.get_playlist_pick_rows();
            if let Some(vm) = model
                .as_any()
                .downcast_ref::<VecModel<UiPlaylistPickRow>>()
            {
                for i in 0..vm.row_count() {
                    if let Some(mut row) = vm.row_data(i)
                        && !add_pick_disabled(row.contained_count, pick_total)
                        && row.selected != sel
                    {
                        row.selected = sel;
                        vm.set_row_data(i, row);
                    }
                }
            }
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
            let dlg = ui.global::<Dialog>();
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

/// A picker row is disabled (unselectable) when every pending track is already
/// in that playlist. Mirrors the Slint-side `disabled` expression.
fn add_pick_disabled(contained_count: i32, pick_total: i32) -> bool {
    pick_total > 0 && contained_count >= pick_total
}

/// Recompute `Dialog.add-selected-count` and `Dialog.add-select-all` from the
/// current Add-to-Playlist picker model, counting only enabled (not fully-
/// contained) rows so "Select all" reflects "all selectable rows selected".
fn refresh_add_selection_meta(dlg: &Dialog) {
    let pick_total = dlg.get_pick_total_tracks();
    let model = dlg.get_playlist_pick_rows();
    let mut enabled: usize = 0;
    let mut selected: usize = 0;
    for r in model.iter() {
        if add_pick_disabled(r.contained_count, pick_total) {
            continue;
        }
        enabled += 1;
        if r.selected {
            selected += 1;
        }
    }
    dlg.set_add_selected_count(clamp_usize(selected));
    dlg.set_add_select_all(enabled > 0 && selected == enabled);
}

/// Recompute `Dialog.export-selected-count` and `Dialog.export-select-all`
/// from the current picker model.
fn refresh_export_selection_meta(dlg: &Dialog) {
    let model = dlg.get_export_pick_rows();
    let total = model.row_count();
    let selected = model.iter().filter(|r| r.selected).count();
    dlg.set_export_selected_count(clamp_usize(selected));
    dlg.set_export_select_all(total > 0 && selected == total);
}

fn clamp_u32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn clamp_usize(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}
