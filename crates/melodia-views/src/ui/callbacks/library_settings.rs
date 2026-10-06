//! `LibrarySettings.*` callbacks: native folder picker, remove, rescan, cancel.

use async_compat::Compat;
use slint::ComponentHandle;

use super::macros::spawn_logged;
use crate::ui::file_dialog;
use crate::ui::settings::library_settings as lib_settings_ui;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, LibrarySettings};

/// Wire the `LibrarySettings.*` callbacks. Pairs with
/// `ui::settings::library_settings::install`, which handles the push side (folder list
/// hydration + scan-progress subscriber). Call once after `wire_all`.
///
/// Add-folder runs the native `rfd` picker on the UI thread (via
/// `slint::spawn_local`) and then drives the DB work through `Compat` so the
/// async sqlx calls have a tokio reactor available. Validation errors are
/// surfaced via the modal `Dialog` overlay.
pub fn wire_library_settings(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<LibrarySettings>();
    let weak = ui.as_weak();

    // Add Folder: native picker → add_folder → kick off a scan on success;
    // on validation/other error pop the Dialog overlay.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_add_folder(move || {
            let s = s.clone();
            let weak = weak.clone();
            // `spawn_local` runs on the UI thread (which on Linux is also
            // the only thread the GTK/portal-backed rfd dialog can be
            // safely invoked from). `Compat` provides a tokio reactor so
            // the awaited sqlx calls work.
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = file_dialog::parented(&weak, "Select Music Folder");
                let Some(handle) = dialog.pick_folder().await else {
                    return;
                };
                let path_str = handle.path().to_string_lossy().into_owned();

                match library::settings::add_folder(&s, path_str).await {
                    // The bump inside `add_folder` already drove the folder-list
                    // subscriber, so the new row is on screen by the time the scan starts.
                    Ok(folder) => library::scan::start(&s, folder.id),
                    Err(AppError::Validation(msg)) => {
                        lib_settings_ui::show_error(&weak, "Cannot add folder", msg);
                    }
                    Err(e) => {
                        lib_settings_ui::show_error(&weak, "Cannot add folder", e.to_string());
                    }
                }
            }));
        });
    }

    // Remove folder: the row's confirm dialog routes the accept here. The
    // `library_changed` bump inside `remove_folder` drives both the
    // folder-list subscriber and the Tracks view's request_refresh, so
    // the row vanishes and the cascade-deleted tracks disappear without
    // any explicit re-fetch here.
    {
        let s = state.clone();
        g.on_remove_folder(move |id| {
            let s = s.clone();
            let id = i64::from(id);
            spawn_logged!(s, "remove_folder", library::settings::remove_folder(&s, id));
        });
    }

    // Rescan: the scan-progress channel drives the bar, and the
    // `library_changed` bump at the end of the scan refreshes the folder
    // list so `last_scanned` updates.
    {
        let s = state.clone();
        g.on_rescan_folder(move |id| library::scan::start(&s, i64::from(id)));
    }

    {
        let s = state.clone();
        g.on_cancel_scan(move || library::scan::cancel(&s));
    }
}
