//! The Import pill: native multi-file picker → import each file into a new
//! playlist → refresh the grid → summary toast.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use super::{TOAST_AUTO_DISMISS_MS, clamp_u32};
use crate::library;
use crate::state::AppState;
use crate::ui::notifications::{NotificationParams, NotificationsUi};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{AppWindow, Playlists, Settings};

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
                        tracks = tracks.saturating_add(r.matched_by_path + r.matched_by_hash);
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
                        message: settings
                            .invoke_playlist_import_message(clamp_u32(tracks), clamp_u32(missing)),
                        action_label: SharedString::default(),
                        action_kind: SharedString::default(),
                    },
                    TOAST_AUTO_DISMISS_MS,
                );
            }
        }));
    });
}
