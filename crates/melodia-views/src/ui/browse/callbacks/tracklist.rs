//! The Browse file list's callbacks.

use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Global as _, Model, SharedString};

use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{play_displayed, wire_queue_actions, wire_toggle_column};
use crate::ui::callbacks::{
    collect_nonzero_track_ids, next_sort, persist_view_sort, persisted_sort,
};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Browse};

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    browse_ui: &Arc<BrowseUi>,
) {
    let g = ui.global::<Browse>();
    seed_sort(&g, view_state, browse_ui);
    wire_play_row(&g, state, browse_ui);
    // Disk-only rows (`id == 0`) can't be queued. The selection already keeps them out, so this
    // guards single-row mode.
    wire_queue_actions(&g, state, collect_nonzero_track_ids);
    wire_row_flags(ui, state, browse_ui);
    wire_sort(ui, state, browse_ui);
    wire_selection(ui, browse_ui);
    wire_card_selected(ui);
    wire_toggle_column(&g.as_weak(), state);
}

/// Seeds the persisted sort before the first navigation applies it.
fn seed_sort(g: &Browse<'_>, view_state: Option<&ViewStateData>, browse_ui: &BrowseUi) {
    if let Some(sort) = persisted_sort(view_state, view_id::BROWSE) {
        g.set_sort_field(SharedString::from(sort.field.as_str()));
        g.set_sort_dir(SharedString::from(sort.dir.as_str()));
        browse_ui.set_sort(sort.field.clone(), sort.dir.as_str().to_owned());
    }
}

/// Queues the folder's in-library files, starting on the clicked one. Disk-only rows are ignored,
/// and shift the row index off `current_in_library_ids`, the case `play_row_start`'s lookup by id
/// exists for.
fn wire_play_row(g: &Browse<'_>, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let s = state.clone();
    let bu = browse_ui.clone();
    g.on_play_row(move |track_id, idx| {
        if track_id == 0 {
            return;
        }
        play_displayed(&s, view_id::BROWSE, bu.current_in_library_ids(), track_id, idx);
    });
}

/// Patched in place, cache and row, so the scroll holds.
fn wire_row_flags(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "browse::set_favorite",
        library::favorites::set_favorite, collect_nonzero_track_ids,
        captures: [weak, bu],
        after: |id_vec, fav| {
            let ids: HashSet<i64> = id_vec.into_iter().collect();
            bu.flip_favorites(&ids, fav);
            browse_ui_mod::apply_rows_favorite(&weak, ids, fav);
        });
    }
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "browse::set_rating",
        library::ratings::set_rating, collect_nonzero_track_ids,
        captures: [weak, bu],
        after: |id_vec, rating| {
            let ids: HashSet<i64> = id_vec.into_iter().collect();
            bu.flip_ratings(&ids, rating);
            browse_ui_mod::apply_rows_rating(&weak, ids, rating);
        });
    }
}

fn wire_sort(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let s = state.clone();
    let bu = browse_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Browse>().on_request_sort(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Browse>();
        let (new_field, new_dir) =
            next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
        g.set_sort_field(SharedString::from(new_field.as_str()));
        g.set_sort_dir(SharedString::from(new_dir.as_str()));
        bu.set_sort(new_field.clone(), new_dir.as_str().to_owned());
        persist_view_sort(&s, view_id::BROWSE, new_field, new_dir);
        browse_ui_mod::resort_and_apply(&ui, &bu);
    });
}

/// Computed in Rust, a Slint expression being unable to iterate a model for a membership check.
fn wire_selection(ui: &AppWindow, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    {
        let weak = ui.as_weak();
        let bu = browse_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::handle_select_row(&ui, &bu, idx, id, shift, ctrl);
        });
    }
    {
        let weak = ui.as_weak();
        g.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::select_all(&ui);
        });
    }
    {
        let weak = ui.as_weak();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::clear_selection(&ui);
        });
    }
}

/// The card grid's per-card read of the same set. Its model is chunked into grid rows, so a
/// flag on the row would cost a `set_row_data` over every card in a row to move one; the
/// generation argument is what re-runs this `pure` binding instead.
fn wire_card_selected(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Browse>().on_is_card_selected(move |id, _generation| {
        let Some(ui) = weak.upgrade() else { return false };
        ui.global::<Browse>().get_selected_ids().iter().any(|selected| selected == id)
    });
}
