//! Import and export of the kept station list.
//!
//! Wired from boot's `install_ui` rather than from the slice's `install`, because the completion
//! toasts need the notifications stack and that does not exist yet at install time — the same
//! constraint `ui::playlists::wire_files` carries, and the same shape.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString};

use crate::ui::file_dialog;
use crate::ui::radio::{RadioUi, kept};
use crate::ui::shell::notifications::{Completion, NotificationsUi, RowText};
use crate::ui::util::count_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Radio, Settings};

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
    wire_import_stations(ui, state, radio_ui, notifications);
    wire_export_stations(ui, state, notifications);
}

fn wire_import_stations(
    ui: &AppWindow,
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    notifications: &Rc<NotificationsUi>,
) {
    let s = state.clone();
    let ru = radio_ui.clone();
    let weak = ui.as_weak();
    let notifications = notifications.clone();
    ui.global::<Radio>().on_import_stations(move || {
        let (s, ru, weak, notifications) =
            (s.clone(), ru.clone(), weak.clone(), notifications.clone());
        let _ = slint::spawn_local(Compat::new(async move {
            let dialog = file_dialog::parented(&weak, "Import Stations")
                .add_filter("Station lists", &IMPORT_EXTENSIONS);
            let Some(handles) = dialog.pick_files().await else {
                return;
            };
            // A pick that returned nothing is a cancel by another name, and reporting
            // "0 stations added" over it reads as a failed import.
            if handles.is_empty() {
                return;
            }
            let paths: Vec<PathBuf> = handles.iter().map(|h| h.path().to_path_buf()).collect();
            let total = library::radio_files::import_stations_from_files(&s.db, &paths).await;

            let Some(ui) = weak.upgrade() else { return };
            let settings = ui.global::<Settings>();
            // Nothing added and something refused to parse is the only outright failure; a
            // file of stations already kept is a successful no-op and says so.
            if total.stations.imported == 0 && total.failed > 0 {
                notifications.show_failure(&ui, |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_station_import_failed_title(),
                        g.invoke_station_import_failed_message(),
                    )
                });
                return;
            }
            notifications.show_completion(
                Completion::partial_if(total.failed > 0),
                settings.invoke_station_import_title(count_as_i32(total.stations.imported)),
                settings.invoke_station_import_message(count_as_i32(total.stations.skipped)),
            );
            kept::refresh(&ui, &s, &ru);
        }));
    });
}

fn wire_export_stations(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let s = state.clone();
    let weak = ui.as_weak();
    let notifications = notifications.clone();
    ui.global::<Radio>().on_export_stations(move || {
        let (s, weak, notifications) = (s.clone(), weak.clone(), notifications.clone());
        let _ = slint::spawn_local(Compat::new(async move {
            // Same filter as the import, so a save name retyped without an extension can't
            // land somewhere the import picker then refuses to show.
            let dialog = file_dialog::parented(&weak, "Export Stations")
                .set_file_name(EXPORT_FILE_NAME)
                .add_filter("Station lists", &IMPORT_EXTENSIONS);
            let Some(target) = dialog.save_file().await else {
                return;
            };
            let path = target.path().to_path_buf();
            let outcome = library::radio_files::export_stations(&s.db, &path).await;

            let Some(ui) = weak.upgrade() else { return };
            let settings = ui.global::<Settings>();
            match outcome {
                Ok(exported) => {
                    notifications.show_completion(
                        Completion::Complete,
                        settings.invoke_station_export_title(count_as_i32(exported)),
                        settings.invoke_station_export_message(SharedString::from(
                            path.display().to_string(),
                        )),
                    );
                }
                Err(e) => {
                    log::warn!("radio: export: {}", melodia_core::error::describe(&e));
                    notifications.show_failure(&ui, |ui| {
                        let g = ui.global::<Settings>();
                        RowText::plain(
                            g.invoke_station_export_failed_title(),
                            g.invoke_station_export_failed_message(),
                        )
                    });
                }
            }
        }));
    });
}
