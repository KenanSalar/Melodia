//! Playlist import/export (Extended-M3U8) callbacks — the Import / Export
//! header pills on the Playlists view.
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
use crate::{AppWindow, Dialog, PlaylistExportPickRow as UiPlaylistExportPickRow, Playlists, Settings};

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
    // (all selected), then open the dialog (chrome was set inline in Slint).
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
