//! The Import pill: native multi-file picker → import each file into a new
//! playlist → refresh the grid → summary toast.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::file_dialog;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::shell::notifications::{
    NotificationParams, NotificationsUi, RowText, TOAST_AUTO_DISMISS_MS,
};
use crate::ui::util::count_as_i32;
use crate::{AppWindow, Playlists, Settings};
use melodia_app::library;
use melodia_app::state::AppState;

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
            let dialog = file_dialog::parented(&weak, "Import Playlists")
                .add_filter("Playlists", &["m3u8", "m3u"]);
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
                match library::playlist_files::import_playlist_from_file(&s, handle.path()).await {
                    Ok(r) => {
                        imported = imported.saturating_add(1);
                        tracks = tracks.saturating_add(r.matched_by_path + r.matched_by_hash);
                        missing = missing.saturating_add(r.missing);
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        log::warn!("import_playlist_from_file {}: {e}", handle.path().display());
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
                notifications.show_localized(&ui, "error", "", |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_playlist_import_failed_title(),
                        g.invoke_playlist_import_failed_message(),
                    )
                });
            } else {
                let variant = if missing > 0 || failures > 0 {
                    "warning"
                } else {
                    "success"
                };
                notifications.show_auto_dismiss(
                    NotificationParams::plain(
                        variant,
                        settings.invoke_playlist_import_title(count_as_i32(imported)),
                        settings.invoke_playlist_import_message(
                            count_as_i32(tracks),
                            count_as_i32(missing),
                        ),
                    ),
                    TOAST_AUTO_DISMISS_MS,
                );
            }
        }));
    });
}
