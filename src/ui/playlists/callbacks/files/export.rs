//! The Export pill: fetch playlists into the selectable picker, commit the
//! selection to a folder of `.m3u8` files, and the picker's selection
//! plumbing (single-row toggle + "Select all").

use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::{refresh_export_selection_meta, set_all_picks, toggle_pick};
use crate::library;
use crate::state::AppState;
use crate::ui::file_dialog;
use crate::ui::shell::notifications::{
    NotificationParams, NotificationsUi, RowText, TOAST_AUTO_DISMISS_MS,
};
use crate::ui::util::count_as_i32;
use crate::{
    AppWindow, Dialog, PlaylistExportPickRow as UiPlaylistExportPickRow, Playlists, Settings,
};

pub(super) fn wire(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

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
                let dialog = file_dialog::parented(&weak, "Export Playlists To Folder");
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
                            NotificationParams::plain(
                                variant,
                                settings.invoke_playlist_export_title(count_as_i32(res.exported)),
                                settings.invoke_playlist_export_message(SharedString::from(
                                    res.folder.as_str(),
                                )),
                            ),
                            TOAST_AUTO_DISMISS_MS,
                        );
                    }
                    Ok(_) => {
                        show_export_failed(&ui, &notifications);
                    }
                    Err(e) => {
                        log::warn!("export_playlists_to_folder: {e}");
                        show_export_failed(&ui, &notifications);
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
            toggle_pick(
                &dlg.get_export_pick_rows(),
                id,
                |r: &UiPlaylistExportPickRow| r.id,
                |_| true,
                |r| r.selected = !r.selected,
            );
            refresh_export_selection_meta(&dlg);
        });
    }

    // set-all-export-picks: set every row's `selected` to `sel`.
    {
        let weak = weak.clone();
        playlists.on_set_all_export_picks(move |sel| {
            let Some(ui) = weak.upgrade() else { return };
            let dlg = ui.global::<Dialog>();
            set_all_picks(
                &dlg.get_export_pick_rows(),
                sel,
                |_: &UiPlaylistExportPickRow| true,
                |r| r.selected,
                |r, v| r.selected = v,
            );
            refresh_export_selection_meta(&dlg);
        });
    }
}

/// Nothing was written. The two ways that happens — an `Ok` reporting zero
/// exports, and an outright failure — say the same thing to the user and differ
/// only in whether there is an error worth logging.
fn show_export_failed(ui: &AppWindow, notifications: &NotificationsUi) {
    notifications.show_localized(ui, "error", "", |ui| {
        let g = ui.global::<Settings>();
        RowText::plain(
            g.invoke_playlist_export_failed_title(),
            g.invoke_playlist_export_failed_message(),
        )
    });
}
