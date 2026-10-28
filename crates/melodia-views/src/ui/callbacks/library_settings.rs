//! `LibrarySettings.*` callbacks: native folder picker, remove, rescan, cancel.

use async_compat::Compat;
use slint::ComponentHandle;

use super::macros::spawn_logged;
use crate::ui::file_dialog;
use crate::ui::settings::library_settings as lib_settings_ui;
use melodia_app::library;
use melodia_app::library::scan::OnStop;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, LibrarySettings};

/// Wire the `LibrarySettings.*` callbacks, the pull half of
/// `ui::settings::library_settings::install`. Call once after `wire_all`.
pub fn wire_library_settings(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<LibrarySettings>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_add_folder(move || {
            let s = s.clone();
            let weak = weak.clone();
            // The UI thread, the only one the portal-backed picker may run on on Linux.
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = file_dialog::parented(&weak, "Select Music Folder");
                let Some(handle) = dialog.pick_folder().await else {
                    return;
                };
                let path_str = handle.path().to_string_lossy().into_owned();
                lib_settings_ui::add_folder_and_scan(&s, &weak, path_str).await;
            }));
        });
    }

    // No re-fetch here: `remove_folder`'s `library_changed` bump repaints the folder list and
    // every view the cascade-deleted tracks sat in.
    {
        let s = state.clone();
        g.on_remove_folder(move |id| {
            let s = s.clone();
            let id = i64::from(id);
            spawn_logged!(s, "remove_folder", library::settings::remove_folder(&s, id));
        });
    }

    // The scan's return to idle refreshes `last_scanned`, a rescan that changed nothing bumping
    // no `library_changed`.
    {
        let s = state.clone();
        g.on_rescan_folder(move |id| library::scan::start(&s, i64::from(id), OnStop::Keep));
    }

    {
        let s = state.clone();
        g.on_cancel_scan(move || library::scan::cancel(&s));
    }
}
