//! The create / rename / delete commits the `Dialog.accepted` dispatcher in
//! `globals/dialog.slint` routes to. Their opens are inline Slint: crossing into
//! Rust to write `Dialog` properties *synchronously* from a click handler trips
//! Slint's "Recursion detected" property guard (`i_slint_core::properties`).

use std::sync::Arc;

use slint::{ComponentHandle, Model, Weak};

use super::{optional_text, refresh_after_edit};
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::track_detail::TrackDetail;
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, PlaylistDetail, Playlists};

pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    wire_create(ui, state, playlists_ui);
    wire_rename(ui, state, playlists_ui);
    wire_delete(ui, state, playlists_ui);
}

/// The playlist and its pending tracks land together or not at all.
fn wire_create(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_create_playlist(move |name, description, pending| {
        let name_str = name.trim().to_owned();
        if name_str.is_empty() {
            return;
        }
        let description_opt = optional_text(&description);
        let pending_vec: Vec<i64> = pending.iter().map(i64::from).collect();
        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            match library::playlists::create_playlist(
                &s.db,
                name_str.clone(),
                description_opt,
                pending_vec,
            )
            .await
            {
                Ok(_) => {
                    playlists_ui_mod::refresh_after_write(&s, &pu, &weak);
                    log::info!("playlists::create_playlist: {name_str:?}");
                }
                Err(e) => {
                    log::warn!("playlists::create_playlist {name_str:?}: {}", describe(&e));
                }
            }
        });
    });
}

fn wire_rename(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_rename_playlist(move |id, new_name, description| {
        let id = i64::from(id);
        let name_str = new_name.trim().to_owned();
        if name_str.is_empty() {
            return;
        }
        // The dialog pre-fills the current description, so an empty one was cleared on purpose.
        let description_opt = optional_text(&description);
        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            match library::playlists::update_playlist(
                &s.db,
                id,
                name_str.clone(),
                description_opt,
                None,
            )
            .await
            {
                Ok(_) => {
                    refresh_after_edit(&s, &pu, weak, &[id], "playlists::rename").await;
                    log::info!("playlists::rename({id}): {name_str:?}");
                }
                Err(e) => {
                    log::warn!("playlists::rename({id}) {name_str:?}: {}", describe(&e));
                }
            }
        });
    });
}

/// `delete-playlist` is the dispatcher's single id, `delete-playlists` the card menu's batch.
fn wire_delete(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = ui.as_weak();
        playlists.on_delete_playlist(move |id| {
            spawn_delete(&s, &pu, &weak, vec![i64::from(id)], "playlists::delete");
        });
    }
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = ui.as_weak();
        playlists.on_delete_playlists(move |ids| {
            let ids: Vec<i64> = ids.iter().map(i64::from).collect();
            spawn_delete(&s, &pu, &weak, ids, "playlists::delete_many");
        });
    }
}

/// Deletes `ids`, closing the open detail first where it is among them, so no frame shows a
/// deleted playlist. A failed write is logged and the rest still runs: the detail is closed by
/// then, and the refetch keeps the grid honest about what survived.
fn spawn_delete(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: &Weak<AppWindow>,
    ids: Vec<i64>,
    label: &'static str,
) {
    if ids.is_empty() {
        return;
    }
    let open_was_deleted = close_detail_if_among(weak, playlists_ui, &ids);
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = weak.clone();
    state.runtime.spawn(async move {
        match library::playlists::delete_playlists(&s.db, &ids).await {
            Ok(()) => log::info!("{label}({ids:?})"),
            Err(e) => log::warn!("{label}({ids:?}): {}", describe(&e)),
        }
        if open_was_deleted {
            // So a restart doesn't reopen it.
            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk.paths,
                    view_id::PLAYLIST_DETAIL,
                    None,
                ) {
                    log::warn!("{label} clear last_detail_id: {}", describe(&e));
                }
            });
        }
        // A deleted id can still be sitting in the card selection, where it would keep
        // being counted by the card menu and asked for by the next batch action.
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<CardSelection>().invoke_clear();
        });
        playlists_ui_mod::refresh_after_write(&s, &pu, &weak);
    });
}

/// Close the open detail when its playlist is among `ids`, reporting whether it was.
fn close_detail_if_among(weak: &Weak<AppWindow>, playlists_ui: &PlaylistsUi, ids: &[i64]) -> bool {
    if !ids.contains(&playlists_ui.detail_playlist_id()) {
        return false;
    }
    if let Some(ui) = weak.upgrade() {
        let d = ui.global::<PlaylistDetail>();
        d.set_playlist_id(-1);
        release_detail_hero_images(&ui, &d);
        playlists_ui.clear_detail();
        // The grid reload after the delete pays whatever the detail left owed.
        playlists_ui.section().take_grid_owed();
    }
    true
}
