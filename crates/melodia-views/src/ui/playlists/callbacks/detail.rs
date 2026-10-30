//! `PlaylistDetail.*` callbacks: close-detail, the drags that move rows, the edit-artwork picker
//! open and track removal here, the rest through [`crate::ui::callbacks::track_detail`].

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};

use super::artwork::open_edit_artwork;
use crate::ui::callbacks::{DialogClaim, collect_track_ids};
use crate::ui::callbacks::{grid_tab, track_detail};
use crate::ui::drag;
use crate::ui::paged_rows::track_rows;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::undo::{Removal, tracks_removed_from};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::playlist::PlaylistItemsCut;
use melodia_core::entities::undo::{RemovalSize, UndoCapture, UndoKind};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::block_move;
use melodia_ui::{AppWindow, PlaylistDetail};

pub(super) fn wire(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let ViewCtx { app: ui, state, .. } = cx;
    track_detail::wire(state, playlists_ui);
    wire_close(ui, state, playlists_ui);
    wire_reorder(ui, state, playlists_ui);
    wire_edit_artwork(ui, state, playlists_ui);
    wire_remove_track(cx, playlists_ui);
}

/// What a run of removals from one playlist took, put back newest first.
pub struct PlaylistCutUndo {
    playlist_id: i64,
    cuts: Vec<PlaylistItemsCut>,
    /// As the detail header showed it at the latest removal.
    name: SharedString,
    playlists_ui: Arc<PlaylistsUi>,
}

impl PlaylistCutUndo {
    pub fn same_playlist(&self, other: &Self) -> bool {
        self.playlist_id == other.playlist_id
    }

    /// A later removal from the same playlist, whose rows are indexed as this one left it.
    pub fn absorb(&mut self, newer: Self) {
        self.cuts.extend(newer.cuts);
        self.name = newer.name;
    }

    pub fn count(&self) -> usize {
        self.cuts.iter().map(|cut| cut.rows.len()).sum()
    }

    pub fn name(&self) -> SharedString {
        self.name.clone()
    }

    /// Writes the rows back, then refreshes as an edit does: the grid, and the detail only while
    /// it still shows this playlist.
    pub async fn restore(self, state: &AppState, weak: &Weak<AppWindow>) -> Result<(), AppError> {
        let Self { playlist_id, cuts, playlists_ui, .. } = self;
        library::playlists::restore_playlist_items(&state.db, cuts).await?;
        let (s, weak) = (state.clone(), weak.clone());
        state.runtime.spawn(async move {
            super::refresh_after_edit(&s, &playlists_ui, weak, &[playlist_id], "playlists::undo")
                .await;
        });
        Ok(())
    }
}

/// The header's back button. No `mark_drill_back` where its siblings have one: this detail
/// records no origin, so its close never moves the nav index and nothing reads the edge.
fn wire_close(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<PlaylistDetail>().on_close_detail(move || {
        let Some(ui) = weak.upgrade() else { return };
        track_detail::forget_closed(&ui, &s, &*pu, &ui.global::<PlaylistDetail>());
        let reloaded = grid_tab::reload_owed_grid(&ui, &s, &pu);

        let pu = pu.clone();
        s.runtime.spawn_blocking(move || {
            pu.release_detail_artwork();
            if !reloaded {
                pu.prewarm_visible_covers();
            }
        });
    });
}

/// The two row drags: one row, or every picked row gathered at a slot, and where the picks sit
/// for the press that lifts them.
fn wire_reorder(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let g = ui.global::<PlaylistDetail>();

    {
        let (s, pu, weak) = (state.clone(), playlists_ui.clone(), ui.as_weak());
        g.on_reorder(move |from, to| {
            let Some(ui) = weak.upgrade() else { return };
            let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
                return;
            };
            move_tracks(&ui, &s, &pu, &[from], to);
        });
    }

    {
        let (s, pu, weak) = (state.clone(), playlists_ui.clone(), ui.as_weak());
        g.on_move_picks(move |slot| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(slot) = usize::try_from(slot) else {
                return;
            };
            let picked = playlists_ui_mod::picked_rows(&ui);
            move_tracks(&ui, &s, &pu, &picked, block_move::landing(&picked, slot));
        });
    }

    let weak = ui.as_weak();
    g.on_picked_run(move || {
        let picked = weak.upgrade().map(|ui| playlists_ui_mod::picked_rows(&ui));
        drag::lifted_run(&picked.unwrap_or_default())
    });
}

/// Moves `rows` to `to` on screen, writes, then reads back the order the database holds, which
/// rolls back a failed write and corrects a refresh that landed mid-write. A write that lands
/// re-reads the header and the card, the automatic cover following whichever track now leads.
fn move_tracks(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    rows: &[usize],
    to: usize,
) {
    let playlist_id = playlists_ui.detail_playlist_id();
    if playlist_id < 0 {
        return;
    }
    let track_ids = playlists_ui_mod::track_ids_at(ui, rows);
    if !playlists_ui_mod::apply_optimistic_move(ui, playlists_ui, rows, to) {
        return;
    }
    let (s, pu, weak) = (state.clone(), playlists_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        let moved =
            library::playlists::move_playlist_tracks(&s.db, playlist_id, track_ids, to).await;
        track_rows::requery(&s, &pu);
        if let Err(e) = moved {
            log::warn!("playlists::move: {}", describe(&e));
            return;
        }
        if let Err(e) = playlists_ui_mod::refresh_header(&s, &pu, weak.clone(), playlist_id).await {
            log::warn!("playlists::move refresh: {}", describe(&e));
        }
        playlists_ui_mod::refresh_after_write(&s, &pu, &weak);
    });
}

/// Opens the Edit Artwork picker on `PlaylistDetail.cover`, already decoded for the open detail.
/// Rename and Delete need no handler here: `my-library/tab-pills.slint` fills their dialogs
/// inline, and `super::crud` commits them.
fn wire_edit_artwork(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<PlaylistDetail>().on_request_edit_artwork(move || {
        if pu.detail_playlist_id() < 0 {
            return;
        }
        let Some(claim) = DialogClaim::take_from(&weak) else { return };
        let Some(ui) = weak.upgrade() else { return };
        let playlist = ui.global::<PlaylistDetail>().get_playlist();
        open_edit_artwork(&s, &weak, claim, &playlist, pu.draw(), |ui| {
            ui.global::<PlaylistDetail>().get_cover()
        });
    });
}

/// The row menu's removal, one row or a selection, as one batched write that offers its undo where
/// Settings covers it. Once it lands the rows and their picks leave on that tick, filtered or not,
/// and `refresh_detail` reloads the rest. On the UI thread, where the undo slot lives.
fn wire_remove_track(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let s = cx.state.clone();
    let pu = playlists_ui.clone();
    let undo = cx.undo.clone();
    let weak = cx.app.as_weak();
    cx.app.global::<PlaylistDetail>().on_remove_track(move |track_ids| {
        let id_vec = collect_track_ids(&track_ids);
        let playlist_id = pu.detail_playlist_id();
        if playlist_id < 0 || id_vec.is_empty() {
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        let name = ui.global::<PlaylistDetail>().get_playlist().name;
        let covered = undo.covers(UndoKind::Playlists, RemovalSize::Items(id_vec.len()));
        let (s, pu, undo, weak) = (s.clone(), pu.clone(), undo.clone(), weak.clone());
        let spawned = slint::spawn_local(Compat::new(async move {
            let removed = if covered {
                library::playlists::remove_tracks_from_playlist_batch(
                    &s.db,
                    playlist_id,
                    id_vec.clone(),
                )
                .await
            } else {
                // Undo off: nothing held, so nothing to tell.
                library::playlists::remove_tracks_from_playlist_for_good(
                    &s.db,
                    playlist_id,
                    id_vec.clone(),
                )
                .await
                .map(|_| UndoCapture::Nothing)
            };
            let removed = match removed {
                Ok(removed) => removed,
                Err(e) => {
                    log::warn!("playlists::remove_track: {}", describe(&e));
                    return;
                }
            };
            track_rows::remove_rows(&*pu, id_vec);
            refresh_after_removal(&s, &pu, &weak, playlist_id);
            match removed {
                UndoCapture::Nothing => {}
                UndoCapture::Held(cut) => {
                    let cuts = vec![cut];
                    let hold = PlaylistCutUndo { playlist_id, cuts, name, playlists_ui: pu };
                    undo.offer(Removal::PlaylistTracks(hold));
                }
                UndoCapture::OverCap { rows } => {
                    let title = tracks_removed_from(rows, name);
                    undo.notify_over_cap(UndoKind::Playlists, rows, title);
                }
            }
        }));
        if let Err(e) = spawned {
            log::warn!("playlists::remove_track spawn_local: {}", describe(&e));
        }
    });
}

/// On the runtime, where every other refresh of the detail runs.
fn refresh_after_removal(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: &Weak<AppWindow>,
    playlist_id: i64,
) {
    let (s, pu, weak) = (state.clone(), playlists_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        if let Err(e) = playlists_ui_mod::refresh_detail(&s, &pu, weak.clone(), playlist_id).await {
            log::warn!("playlists::remove_track refresh: {}", describe(&e));
        }
        playlists_ui_mod::refresh_after_write(&s, &pu, &weak);
    });
}
