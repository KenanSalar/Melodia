//! Import and export of the kept station list.
//!
//! Wired from `main()` rather than from the slice's `install`, because the completion toasts need
//! the notifications stack and that does not exist yet at install time — the same constraint
//! `ui::playlists::wire_files` carries, and the same shape.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::file_dialog;
use crate::ui::radio::{RadioUi, kept};
use crate::ui::shell::notifications::{NotificationParams, NotificationsUi, TOAST_AUTO_DISMISS_MS};
use crate::ui::util::count_as_i32;
use crate::{AppWindow, Radio, Settings};

/// The extensions a station list arrives in. `.pls` is the one most stations are handed out as,
/// which is why the importer reads it at all.
const IMPORT_EXTENSIONS: [&str; 3] = ["m3u8", "m3u", "pls"];

/// Suggested filename for an export. Not localized: it is a filename, and the extension is what
/// makes the file openable elsewhere.
const EXPORT_FILE_NAME: &str = "stations.m3u8";

pub fn wire(
    ui: &AppWindow,
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        g.on_import_stations(move || {
            let (s, ru, weak, notifications) =
                (s.clone(), ru.clone(), weak.clone(), notifications.clone());
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = file_dialog::parented(&weak, "Import Stations")
                    .add_filter("Station lists", &IMPORT_EXTENSIONS);
                let Some(handles) = dialog.pick_files().await else {
                    return;
                };

                let mut imported: u32 = 0;
                let mut skipped: u32 = 0;
                let mut failures: u32 = 0;
                for handle in &handles {
                    match library::radio_files::import_stations_from_file(&s, handle.path()).await {
                        Ok(result) => {
                            imported = imported.saturating_add(result.imported);
                            skipped = skipped.saturating_add(result.skipped);
                        }
                        Err(e) => {
                            failures = failures.saturating_add(1);
                            log::warn!(
                                "radio: import {}: {}",
                                handle.path().display(),
                                crate::services::describe(&e)
                            );
                        }
                    }
                }

                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                // Nothing added and something refused to parse is the only outright failure; a
                // file of stations already kept is a successful no-op and says so.
                if imported == 0 && failures > 0 {
                    notifications.show(NotificationParams::plain(
                        "error",
                        settings.invoke_station_import_failed_title(),
                        settings.invoke_station_import_failed_message(),
                    ));
                    return;
                }
                let variant = if failures > 0 { "warning" } else { "success" };
                notifications.show_auto_dismiss(
                    NotificationParams::plain(
                        variant,
                        settings.invoke_station_import_title(count_as_i32(imported)),
                        settings.invoke_station_import_message(count_as_i32(skipped)),
                    ),
                    TOAST_AUTO_DISMISS_MS,
                );
                kept::refresh(&ui, &s, &ru);
            }));
        });
    }

    {
        let s = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        g.on_export_stations(move || {
            let (s, weak, notifications) = (s.clone(), weak.clone(), notifications.clone());
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog =
                    file_dialog::parented(&weak, "Export Stations").set_file_name(EXPORT_FILE_NAME);
                let Some(target) = dialog.save_file().await else {
                    return;
                };
                let path = target.path().to_path_buf();
                let outcome = library::radio_files::export_stations(&s, &path).await;

                let Some(ui) = weak.upgrade() else { return };
                let settings = ui.global::<Settings>();
                match outcome {
                    Ok(exported) => {
                        notifications.show_auto_dismiss(
                            NotificationParams::plain(
                                "success",
                                settings.invoke_station_export_title(count_as_i32(exported)),
                                settings.invoke_station_export_message(SharedString::from(
                                    path.display().to_string(),
                                )),
                            ),
                            TOAST_AUTO_DISMISS_MS,
                        );
                    }
                    Err(e) => {
                        log::warn!("radio: export: {}", crate::services::describe(&e));
                        notifications.show(NotificationParams::plain(
                            "error",
                            settings.invoke_station_export_failed_title(),
                            settings.invoke_station_export_failed_message(),
                        ));
                    }
                }
            }));
        });
    }
}
