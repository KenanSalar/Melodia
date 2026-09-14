//! The Import pill: native multi-file picker → import each playlist file, or
//! each playlist in an archive, into a new playlist → refresh the grid →
//! summary toast.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::file_dialog;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::shell::notifications::{Completion, NotificationsUi, RowText};
use crate::ui::util::count_as_i32;
use melodia_app::library::playlist_files::{self, ImportFileResult};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Playlists, Settings};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    let s = state.clone();
    let pu = playlists_ui.clone();
    let notifications = notifications.clone();
    playlists.on_import_playlists(move || {
        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            // One filter rather than two, which a picker offers as alternatives and so hides
            // whichever isn't selected.
            let extensions: Vec<&str> = playlist_files::PLAYLIST_EXTENSIONS
                .into_iter()
                .chain([playlist_files::ARCHIVE_EXTENSION])
                .collect();
            let dialog = file_dialog::parented(&weak, "Import Playlists")
                .add_filter("Playlists", &extensions);
            let Some(handles) = dialog.pick_files().await else {
                return;
            };
            if handles.is_empty() {
                return;
            }

            let paths: Vec<PathBuf> =
                handles.iter().map(|handle| handle.path().to_path_buf()).collect();

            // Off the UI thread: every playlist's parse and track matching happens in there.
            let import_state = s.clone();
            let result = s
                .runtime
                .spawn(async move {
                    playlist_files::import_playlists_from_files(&import_state, &paths).await
                })
                .await
                .unwrap_or_else(|e| {
                    log::warn!("playlist import: task ended early: {e}");
                    ImportFileResult::default()
                });

            if result.imported > 0
                && let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await
            {
                log::warn!("fetch_grid after playlist import: {e}");
            }

            let Some(ui) = weak.upgrade() else { return };
            if result.imported == 0 {
                notifications.show_failure(&ui, |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_playlist_import_failed_title(),
                        g.invoke_playlist_import_failed_message(),
                    )
                });
            } else {
                let settings = ui.global::<Settings>();
                notifications.show_completion(
                    Completion::partial_if(result.missing > 0 || result.failed > 0),
                    settings.invoke_playlist_import_title(count_as_i32(result.imported)),
                    settings.invoke_playlist_import_message(
                        count_as_i32(result.matched),
                        count_as_i32(result.missing),
                    ),
                );
            }
        }));
    });
}
