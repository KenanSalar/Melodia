//! Library Folders settings glue: hydrates the `LibrarySettings` Slint global
//! from the DB, subscribes to scan-progress and library-changed channels, and
//! exposes a small helper for opening the modal `Dialog` overlay.
//!
//! The Slint `LibrarySettings.folders` model is rebuilt as a fresh
//! `Rc<VecModel<FolderListRow>>` on every refresh — folder lists are tiny
//! (a handful of entries) so virtualization isn't needed and the allocation
//! cost is negligible.

use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::ui::util::{clamp_i64_to_i32, count_as_i32};
use melodia_app::library;
use melodia_app::state::{AppState, ScanProgressTick};
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, FolderListRow, LibrarySettings, ScanPhase};

/// Wire up the `LibrarySettings` global: initial folder fetch +
/// scan-progress subscriber + library-changed re-fetch subscriber. Call once
/// during startup after `AppWindow::new()` and `AppState::init`.
pub fn install(ui: &AppWindow, state: &AppState) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();

    // Empty seed model so the Slint side never observes a default-constructed
    // `ModelRc` (which is a no-op model that ignores `set_vec` etc. — we'd
    // never see UI updates if we left it that way).
    let initial: Rc<VecModel<FolderListRow>> = Rc::new(VecModel::default());
    ui.global::<LibrarySettings>().set_folders(ModelRc::from(initial));

    // 1. Initial fetch.
    {
        let weak = weak.clone();
        let s = state.clone();
        state.runtime.spawn(async move {
            refresh_folders(weak, s).await;
        });
    }

    // 2. Scan-progress subscriber: live progress bar updates on the UI.
    {
        let mut rx = state.scan.subscribe();
        let weak = weak.clone();
        slint::spawn_local(Compat::new(async move {
            // Painted before the first wait: the first-launch scan can be publishing before
            // this subscribes, and `subscribe` marks that value seen.
            loop {
                let snapshot = rx.borrow_and_update().clone();
                match weak.upgrade() {
                    Some(ui) => {
                        paint_scan_progress(&ui.global::<LibrarySettings>(), snapshot.as_ref());
                    }
                    None => break,
                }
                if rx.changed().await.is_err() {
                    break;
                }
            }
            log::debug!("ui::settings::library_settings scan-progress subscriber stopped");
        }))?;
    }

    // 3. Library-changed subscriber: refetch folder list so `last_scanned`
    //    updates after each scan completion (own scans + watcher batches).
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

/// Fetch the folder list from the DB and publish it onto
/// `LibrarySettings.folders`. Safe to call from any thread; the actual UI
/// write hops onto the event loop.
pub async fn refresh_folders(ui: Weak<AppWindow>, state: AppState) {
    let folders = match library::settings::get_folders(&state).await {
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

/// Pop the modal Dialog overlay with the given title + message. Safe to call
/// from any thread; routes through the UI event loop.
pub fn show_error(ui_weak: &Weak<AppWindow>, title: impl Into<String>, message: String) {
    let title = title.into();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<Dialog>();
        g.set_title(SharedString::from(title.as_str()));
        g.set_message(SharedString::from(message.as_str()));
        g.set_open(true);
    });
}

/// RFC3339-stored timestamps (`folder.last_scanned`) → human display string
/// shown under each folder row. `None` collapses to an empty string, which the
/// row uses to hide the meta line.
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
