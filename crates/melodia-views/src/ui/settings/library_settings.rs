//! The Library Folders card: the scan bar, and the folder list, a fresh `VecModel` per refresh
//! since a handful of folders needs no diff.

use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::ui::util::{clamp_i64_to_i32, count_as_i32};
use melodia_app::library;
use melodia_app::library::scan::OnStop;
use melodia_app::state::{AppState, ScanProgressTick};
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, Dialog, FolderListRow, LibrarySettings, ScanPhase};

pub fn install(ui: &AppWindow, state: &AppState) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();

    let initial: Rc<VecModel<FolderListRow>> = Rc::new(VecModel::default());
    ui.global::<LibrarySettings>().set_folders(ModelRc::from(initial));

    {
        let weak = weak.clone();
        let s = state.clone();
        state.runtime.spawn(async move {
            refresh_folders(weak, s).await;
        });
    }

    // The scan bar, and the folder list again as a run ends, when `last_scanned` moves: a run
    // that changed nothing bumps no `library_changed` to say so.
    {
        let mut rx = state.scan.subscribe();
        let weak = weak.clone();
        let s = state.clone();
        slint::spawn_local(Compat::new(async move {
            // Painted before the first wait: the launch scan can be publishing before
            // this subscribes, and `subscribe` marks that value seen.
            let mut snapshot = rx.borrow_and_update().clone();
            loop {
                match weak.upgrade() {
                    Some(ui) => {
                        paint_scan_progress(&ui.global::<LibrarySettings>(), snapshot.as_ref());
                    }
                    None => break,
                }
                if rx.changed().await.is_err() {
                    break;
                }
                snapshot.clone_from(&rx.borrow());
                // Only a run letting go of the bar sends `None`.
                if snapshot.is_none() {
                    s.runtime.spawn(refresh_folders(weak.clone(), s.clone()));
                }
            }
            log::debug!("ui::settings::library_settings scan-progress subscriber stopped");
        }))?;
    }

    // A folder added, removed or absorbed. Its own loop rather than `ui::signal::on_signal`,
    // whose callback can't await: awaiting the refetch folds a burst of bumps into one pass.
    {
        let mut rx = state.library_changed.subscribe();
        let weak = weak.clone();
        let s = state.clone();
        slint::spawn_local(Compat::new(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                refresh_folders(weak.clone(), s.clone()).await;
            }
            log::debug!("ui::settings::library_settings library-changed subscriber stopped");
        }))?;
    }

    Ok(())
}

/// Publishes the folder list onto `LibrarySettings.folders`. Any thread.
pub async fn refresh_folders(ui: Weak<AppWindow>, state: AppState) {
    let folders = match library::settings::get_folders(&state.db).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("library_settings::refresh_folders: {}", describe(&e));
            return;
        }
    };

    let rows: Vec<FolderListRow> = folders
        .iter()
        .map(|f| FolderListRow {
            id: clamp_i64_to_i32(f.id),
            path: SharedString::from(f.path.as_str()),
            last_scanned: SharedString::from(format_last_scanned(f.last_scanned.as_deref())),
        })
        .collect();

    let _ = ui.upgrade_in_event_loop(move |ui| {
        let model: Rc<VecModel<FolderListRow>> = Rc::new(VecModel::from(rows));
        ui.global::<LibrarySettings>().set_folders(ModelRc::from(model));
    });
}

/// Add `path` to the library and scan it, raising the Dialog with the reason a folder was
/// refused. The Library tab's picker and the welcome card's Music folder offer both land here.
pub async fn add_folder_and_scan(state: &AppState, ui: &Weak<AppWindow>, path: String) {
    match library::settings::add_folder(state, path).await {
        // The bump inside `add_folder` already drove the folder-list subscriber, so the new row
        // is on screen by the time the scan starts.
        Ok(folder) => library::scan::start(state, folder.id, OnStop::Withdraw),
        Err(AppError::Validation { msg, .. }) => show_error(ui, "Cannot add folder", msg),
        Err(AppError::Io(io_error)) => show_error(ui, "Cannot add folder", describe(&io_error)),
        Err(e) => show_error(ui, "Cannot add folder", describe(&e)),
    }
}

/// Raises the modal Dialog with `title` and `message`. Any thread.
pub fn show_error(ui_weak: &Weak<AppWindow>, title: impl Into<String>, message: String) {
    let title = title.into();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<Dialog>();
        g.set_title(SharedString::from(title.as_str()));
        g.set_message(SharedString::from(message.as_str()));
        g.set_open(true);
    });
}

/// A folder's stored `last_scanned` as the line under its row, empty for `None`, which hides it.
pub fn format_last_scanned(stamp: Option<&str>) -> String {
    let Some(stamp) = stamp else {
        return String::new();
    };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return String::new();
    };
    let now = chrono::Utc::now();
    let elapsed = now.signed_duration_since(parsed.with_timezone(&chrono::Utc));
    if elapsed.num_seconds() < 60 {
        return "scanned just now".to_owned();
    }
    let minutes = elapsed.num_minutes();
    if minutes < 60 {
        return format!("scanned {minutes} minute{} ago", plural(minutes));
    }
    let hours = elapsed.num_hours();
    if hours < 24 {
        return format!("scanned {hours} hour{} ago", plural(hours));
    }
    let days = elapsed.num_days();
    if days < 7 {
        return format!("scanned {days} day{} ago", plural(days));
    }
    format!("scanned {}", parsed.format("%Y-%m-%d"))
}

fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Shows `tick` on the scan bar, or puts the bar away when no scan is running.
fn paint_scan_progress(g: &LibrarySettings<'_>, tick: Option<&ScanProgressTick>) {
    let Some(tick) = tick else {
        g.set_scan_phase(ScanPhase::Idle);
        g.set_found_count(0);
        g.set_scanned_count(0);
        g.set_total_count(0);
        g.set_scanning_file(SharedString::default());
        return;
    };
    g.set_scan_phase(ui_phase(tick.phase));
    g.set_found_count(count_as_i32(tick.found));
    g.set_scanned_count(count_as_i32(tick.done));
    g.set_total_count(count_as_i32(tick.total));
    g.set_scanning_file(SharedString::from(tick.current_file.as_str()));
}

/// The global's spelling of a backend phase; its extra `Idle` is the empty channel.
fn ui_phase(phase: melodia_app::state::ScanPhase) -> ScanPhase {
    use melodia_app::state::ScanPhase as Backend;
    match phase {
        Backend::Discovering => ScanPhase::Discovering,
        Backend::Reading => ScanPhase::Reading,
        Backend::Finishing => ScanPhase::Finishing,
        Backend::Stopping => ScanPhase::Stopping,
    }
}

#[cfg(test)]
#[path = "tests/library_settings_tests.rs"]
mod tests;
