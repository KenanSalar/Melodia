//! The Extended M3U8 import and export pills and the Add-to-Playlist picker, wired apart from
//! [`super::wire`] for the `Rc<NotificationsUi>` their toasts need.
//!
//! Each flow runs on the UI thread under `slint::spawn_local(Compat::new(…))`, so it resumes
//! there after every `.await` and can toast and touch `Dialog.*` with no extra hop. Import and
//! export await their file work on the runtime instead, which parses or serializes every playlist.

mod add_picker;
mod export;
mod import;

use std::rc::Rc;
use std::sync::Arc;

use slint::{Model, ModelRc, VecModel};

use crate::ui::playlists::PlaylistsUi;
use crate::ui::shell::notifications::NotificationsUi;
use crate::ui::util::len_as_i32;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Dialog, PlaylistPickRow};

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

/// Flips the picker row with `id` unless `enabled` rejects it. Both pickers' toggles walk through
/// here, their two row structs being distinct generated types reached through closures.
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

/// Sets every enabled row's `selected` to `sel`, writing back only the rows that change. Both
/// pickers' Select all.
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

/// Whether a picker row can be selected: not when every pending track is already
/// in that playlist. The negation of the Slint-side `disabled` expression.
fn add_pickable(row: &PlaylistPickRow, pick_total: i32) -> bool {
    !(pick_total > 0 && row.contained_count >= pick_total)
}

/// Recounts the Add-to-Playlist picker's header off its enabled rows alone, so Select all means
/// every selectable row.
fn refresh_add_selection_meta(dlg: &Dialog) {
    let pick_total = dlg.get_pick_total_tracks();
    let model = dlg.get_playlist_pick_rows();
    let mut enabled: usize = 0;
    let mut selected: usize = 0;
    for r in model.iter() {
        if !add_pickable(&r, pick_total) {
            continue;
        }
        enabled += 1;
        if r.selected {
            selected += 1;
        }
    }
    dlg.set_add_selected_count(len_as_i32(selected));
    dlg.set_add_select_all(enabled > 0 && selected == enabled);
}

fn refresh_export_selection_meta(dlg: &Dialog) {
    let model = dlg.get_export_pick_rows();
    let total = model.row_count();
    let selected = model.iter().filter(|r| r.selected).count();
    dlg.set_export_selected_count(len_as_i32(selected));
    dlg.set_export_select_all(total > 0 && selected == total);
}
