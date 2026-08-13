//! Playlist import/export (Extended-M3U8) callbacks — two of the Playlists
//! tab's four action pills — plus the Add-to-Playlist picker's multi-select
//! toggles + commit, split by concern:
//!
//! * [`import`] — the Import pill (native multi-file picker → per-file
//!   import aggregation → grid refresh → summary toast).
//! * [`export`] — the Export pill (fill picker → folder picker → write
//!   files → toast) and the export picker's selection plumbing.
//! * [`add_picker`] — the Add-to-Playlist picker's selection plumbing and
//!   the add-tracks commit.
//!
//! All native dialogs run on the UI thread via
//! `slint::spawn_local(Compat::new(...))` (`Compat` supplies a tokio reactor
//! so the awaited sqlx calls work); after each `.await` the future resumes on
//! the UI thread, so pushing toasts through the `Rc<NotificationsUi>` and
//! reading/writing `Dialog.*` is safe without an extra event-loop hop.
//!
//! Wired separately from [`super::wire`] (in `main.rs`, after the
//! notifications stack exists) because these handlers need the
//! `Rc<NotificationsUi>` — see [`wire`].

mod add_picker;
mod export;
mod import;

use std::rc::Rc;
use std::sync::Arc;

use slint::{Model, ModelRc, VecModel};

use crate::state::AppState;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::shell::notifications::NotificationsUi;
use crate::{AppWindow, Dialog};

/// Wire the `Playlists.*` import/export callbacks. Call once after both the
/// `playlists_ui` handle and the notifications stack exist.
pub fn wire(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    notifications: &Rc<NotificationsUi>,
) {
    import::wire(ui, state, playlists_ui, notifications);
    export::wire(ui, state, notifications);
    add_picker::wire(ui, state, playlists_ui, notifications);
}

/// Find the picker row with `id` and flip it via `flip`, unless `enabled`
/// rejects the row. Shared walk for both pickers' single-row toggles — the
/// two row structs (`PlaylistExportPickRow`, `PlaylistPickRow`) are distinct
/// generated types, so field access goes through closures.
fn toggle_pick<R: Clone + 'static>(
    model: &ModelRc<R>,
    id: i32,
    id_of: impl Fn(&R) -> i32,
    enabled: impl Fn(&R) -> bool,
    flip: impl Fn(&mut R),
) {
    let Some(vm) = model.as_any().downcast_ref::<VecModel<R>>() else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        if id_of(&row) == id {
            if enabled(&row) {
                flip(&mut row);
                vm.set_row_data(i, row);
            }
            break;
        }
    }
}

/// Set every `enabled` row's `selected` to `sel`, writing back only rows that
/// actually change. Shared walk for both pickers' "Select all" headers.
fn set_all_picks<R: Clone + 'static>(
    model: &ModelRc<R>,
    sel: bool,
    enabled: impl Fn(&R) -> bool,
    is_selected: impl Fn(&R) -> bool,
    set_selected: impl Fn(&mut R, bool),
) {
    let Some(vm) = model.as_any().downcast_ref::<VecModel<R>>() else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        if enabled(&row) && is_selected(&row) != sel {
            set_selected(&mut row, sel);
            vm.set_row_data(i, row);
        }
    }
}

/// A picker row is disabled (unselectable) when every pending track is already
/// in that playlist. Mirrors the Slint-side `disabled` expression.
fn add_pick_disabled(contained_count: i32, pick_total: i32) -> bool {
    pick_total > 0 && contained_count >= pick_total
}

/// Recompute `Dialog.add-selected-count` and `Dialog.add-select-all` from the
/// current Add-to-Playlist picker model, counting only enabled (not fully-
/// contained) rows so "Select all" reflects "all selectable rows selected".
fn refresh_add_selection_meta(dlg: &Dialog) {
    let pick_total = dlg.get_pick_total_tracks();
    let model = dlg.get_playlist_pick_rows();
    let mut enabled: usize = 0;
    let mut selected: usize = 0;
    for r in model.iter() {
        if add_pick_disabled(r.contained_count, pick_total) {
            continue;
        }
        enabled += 1;
        if r.selected {
            selected += 1;
        }
    }
    dlg.set_add_selected_count(clamp_usize(selected));
    dlg.set_add_select_all(enabled > 0 && selected == enabled);
}

/// Recompute `Dialog.export-selected-count` and `Dialog.export-select-all`
/// from the current picker model.
fn refresh_export_selection_meta(dlg: &Dialog) {
    let model = dlg.get_export_pick_rows();
    let total = model.row_count();
    let selected = model.iter().filter(|r| r.selected).count();
    dlg.set_export_selected_count(clamp_usize(selected));
    dlg.set_export_select_all(total > 0 && selected == total);
}

fn clamp_u32(n: u32) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn clamp_usize(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}
