//! The Export pill: fetch playlists into the selectable picker, commit the
//! selection to one named `.m3u8` or a zip of them, and the picker's
//! selection plumbing (single-row toggle + "Select all").

use std::path::PathBuf;
use std::rc::Rc;

use async_compat::Compat;
use chrono::Local;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use super::{refresh_export_selection_meta, set_all_picks, toggle_pick};
use crate::ui::file_dialog;
use crate::ui::shell::notifications::{
    NotificationParams, NotificationsUi, RowText, TOAST_AUTO_DISMISS_MS,
};
use crate::ui::util::count_as_i32;
use melodia_app::library;
use melodia_app::library::playlist_files::{self, ArchiveExport};
use melodia_app::state::AppState;
use melodia_core::error::AppError;
use melodia_ui::{
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
                let mut stats = library::playlists::get_playlists(&s).await.unwrap_or_else(|e| {
                    log::warn!("request_export_playlists get_playlists: {e}");
                    Vec::new()
                });
                // A smart playlist exports what its rules match, which its stored count isn't.
                let smart: Vec<usize> =
                    stats.iter().enumerate().filter(|(_, p)| p.is_smart).map(|(i, _)| i).collect();
                library::smart_playlists::recount(&s, &mut stats, &smart).await;
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
    // ticked rows synchronously (the dialog closes + clears its model right
    // after this returns), then ask where to write them.
    {
        let s = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        playlists.on_export_selected_playlists(move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(selection) = Selection::ticked(&ui.global::<Dialog>()) else {
                return;
            };
            let _ = slint::spawn_local(Compat::new(export(
                s.clone(),
                weak.clone(),
                notifications.clone(),
                selection,
            )));
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

/// What the ticked rows ask for. One playlist is a bare `.m3u8` any player opens, the way a station
/// list is; several are one zip of them, so either answer is a single file the user names.
enum Selection {
    One { id: i64, name: String },
    Many(Vec<i64>),
}

impl Selection {
    /// `None` when nothing is ticked.
    fn ticked(dlg: &Dialog) -> Option<Self> {
        let rows: Vec<UiPlaylistExportPickRow> =
            dlg.get_export_pick_rows().iter().filter(|r| r.selected).collect();
        match rows.as_slice() {
            [] => None,
            [row] => Some(Self::One { id: i64::from(row.id), name: row.name.to_string() }),
            _ => Some(Self::Many(rows.iter().map(|r| i64::from(r.id)).collect())),
        }
    }

    /// Asks where to save, prefilled for what is being exported. `None` is a cancel.
    async fn ask_destination(&self, weak: &Weak<AppWindow>) -> Option<PathBuf> {
        let dialog = match self {
            Self::One { name, .. } => file_dialog::parented(weak, "Export Playlist")
                .set_file_name(playlist_files::suggested_file_name(name))
                .add_filter("Playlists", &playlist_files::PLAYLIST_EXTENSIONS),
            Self::Many(_) => file_dialog::parented(weak, "Export Playlists")
                .set_file_name(playlist_files::suggested_archive_name(Local::now()))
                .add_filter("Playlist archives", &[playlist_files::ARCHIVE_EXTENSION]),
        };
        dialog.save_file().await.map(|target| target.path().to_path_buf())
    }

    /// Runs on the runtime: awaited from the UI thread, every playlist's rows would be decoded and
    /// serialized there between awaits.
    async fn write(self, state: AppState, dest: PathBuf) -> Result<Written, AppError> {
        let runtime = state.runtime.clone();
        runtime
            .spawn(async move {
                match self {
                    Self::One { id, .. } => {
                        playlist_files::export_playlist_to_file(&state, id, &dest)
                            .await
                            .map(|()| Written::Playlists { count: 1, partial: false })
                    }
                    Self::Many(ids) => {
                        playlist_files::export_playlists_to_archive(&state, &ids, &dest)
                            .await
                            .map(Written::from)
                    }
                }
            })
            .await
            .map_err(AppError::io_source)?
    }
}

/// What reached the disk, in the terms the toast states it.
enum Written {
    Playlists {
        count: u32,
        /// Some ticked playlists were left out beside the ones written.
        partial: bool,
    },
    /// Nothing did: the playlists hold more than an archive import reads back.
    TooLarge,
}

impl From<ArchiveExport> for Written {
    fn from(export: ArchiveExport) -> Self {
        match export {
            ArchiveExport::Exported(result) => {
                Self::Playlists { count: result.exported, partial: result.failed > 0 }
            }
            ArchiveExport::TooLarge => Self::TooLarge,
        }
    }
}

async fn export(
    state: AppState,
    weak: Weak<AppWindow>,
    notifications: Rc<NotificationsUi>,
    selection: Selection,
) {
    let Some(dest) = selection.ask_destination(&weak).await else {
        return;
    };
    let result = selection.write(state, dest.clone()).await;

    let Some(ui) = weak.upgrade() else { return };
    match result {
        Ok(Written::Playlists { count, partial }) if count > 0 => {
            let settings = ui.global::<Settings>();
            let variant = if partial { "warning" } else { "success" };
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    variant,
                    settings.invoke_playlist_export_title(count_as_i32(count)),
                    settings.invoke_playlist_export_message(SharedString::from(
                        dest.display().to_string(),
                    )),
                ),
                TOAST_AUTO_DISMISS_MS,
            );
        }
        Ok(Written::Playlists { .. }) => show_export_failed(&ui, &notifications, Unwritten::Failed),
        Ok(Written::TooLarge) => show_export_failed(&ui, &notifications, Unwritten::TooLarge),
        Err(e) => {
            log::warn!("playlists: export: {}", melodia_core::error::describe(&e));
            show_export_failed(&ui, &notifications, Unwritten::Failed);
        }
    }
}

/// Why nothing reached the disk, in the terms the toast states it.
#[derive(Clone, Copy)]
enum Unwritten {
    /// An outright failure, or an `Ok` exporting nothing: the same to the user, differing only in
    /// whether there is an error worth logging.
    Failed,
    /// Named, since exporting fewer at a time is the way out.
    TooLarge,
}

fn show_export_failed(ui: &AppWindow, notifications: &NotificationsUi, reason: Unwritten) {
    notifications.show_localized(ui, "error", "", move |ui| {
        let g = ui.global::<Settings>();
        let message = match reason {
            Unwritten::Failed => g.invoke_playlist_export_failed_message(),
            Unwritten::TooLarge => g.invoke_playlist_export_too_large_message(),
        };
        RowText::plain(g.invoke_playlist_export_failed_title(), message)
    });
}
