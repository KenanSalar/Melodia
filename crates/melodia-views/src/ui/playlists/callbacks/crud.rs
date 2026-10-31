//! Creating, renaming and deleting playlists. Create and rename commit from the `Dialog.accepted`
//! dispatcher in `globals/dialog.slint`, their opens being inline Slint: crossing into Rust to
//! write `Dialog` properties *synchronously* from a click handler trips Slint's "Recursion
//! detected" property guard (`i_slint_core::properties`). A delete runs at once and offers its
//! undo, raising a confirmation from a later tick only where it can't be undone.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use super::{optional_text, refresh_after_edit};
use crate::ui::callbacks::DialogClaim;
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::track_detail::TrackDetail;
use crate::ui::track_list_view::view_id;
use crate::ui::undo::{Removal, UndoWindow, playlists_deleted};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::library;
use melodia_app::library::playlists::DeletedPlaylists;
use melodia_app::state::AppState;
use melodia_core::entities::undo::{RemovalSize, UNDO_HOLD_CAP, UndoKind};
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, CardSelection, Dialog, NoUndoReason, PlaylistDetail, Playlists};

pub(super) fn wire(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let ViewCtx { app: ui, state, .. } = cx;
    wire_create(ui, state, playlists_ui);
    wire_rename(ui, state, playlists_ui);
    wire_delete(cx, playlists_ui);
}

/// What a run of deletes took, put back together.
pub struct PlaylistDeleteUndo {
    deleted: DeletedPlaylists,
    playlists_ui: Arc<PlaylistsUi>,
}

impl PlaylistDeleteUndo {
    pub fn absorb(&mut self, newer: Self) {
        self.deleted.absorb(newer.deleted);
    }

    pub fn count(&self) -> usize {
        self.deleted.playlist_count()
    }

    pub fn items(&self) -> usize {
        self.deleted.item_count()
    }

    pub fn name(&self) -> SharedString {
        SharedString::from(self.deleted.newest_name())
    }

    /// Writes the playlists back and reloads the grid. The detail a delete closed stays closed,
    /// navigation not being part of what went.
    pub async fn restore(self, state: &AppState, weak: &Weak<AppWindow>) -> Result<(), AppError> {
        library::playlists::restore_playlists(&state.db, self.deleted).await?;
        playlists_ui_mod::refresh_after_write(state, &self.playlists_ui, weak);
        Ok(())
    }
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

/// The trash, the card menu and the detail's pill all arrive at `delete-playlists`, and the
/// confirmation a delete past the cap raises at `delete-playlists-confirmed`.
fn wire_delete(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let deleter = Deleter {
        state: cx.state.clone(),
        playlists_ui: playlists_ui.clone(),
        undo: cx.undo.clone(),
        weak: cx.app.as_weak(),
    };
    let playlists = cx.app.global::<Playlists>();
    {
        let deleter = deleter.clone();
        playlists.on_delete_playlists(move |ids| deleter.request(ids.iter().collect()));
    }
    playlists.on_delete_playlists_confirmed(move |ids| {
        deleter.for_good(ids.iter().map(i64::from).collect());
    });
}

/// What a delete reaches. Both of its paths run on the UI thread, where the undo slot lives.
#[derive(Clone)]
struct Deleter {
    state: AppState,
    playlists_ui: Arc<PlaylistsUi>,
    undo: Rc<UndoWindow>,
    weak: Weak<AppWindow>,
}

impl Deleter {
    /// Deletes at once and offers the undo, or asks first where Settings turned undo off for it or
    /// the playlists hold more tracks than an undo may.
    fn request(&self, ids: Vec<i32>) {
        if ids.is_empty() {
            return;
        }
        let id_vec: Vec<i64> = ids.iter().copied().map(i64::from).collect();
        let Some(claim) = DialogClaim::take_from(&self.weak) else { return };
        let covered = self.undo.covers(UndoKind::Playlists, RemovalSize::Items(ids.len()));
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            if !covered {
                this.confirm_first(claim, ids, NoUndoReason::TurnedOff);
                return;
            }
            let counted =
                library::playlists::playlist_item_count(&this.state.db, id_vec.clone()).await;
            match counted {
                Ok(items) if items > UNDO_HOLD_CAP => {
                    this.confirm_first(claim, ids, NoUndoReason::TooLarge);
                }
                Ok(_) => this.delete_held(id_vec).await,
                Err(e) => log::warn!("playlists::delete count: {}", describe(&e)),
            }
        }));
        if let Err(e) = spawned {
            log::warn!("playlists::delete spawn_local: {}", describe(&e));
        }
    }

    async fn delete_held(&self, ids: Vec<i64>) {
        let open_was_deleted = close_detail_if_among(&self.weak, &self.playlists_ui, &ids);
        let deleted = library::playlists::delete_playlists_held(&self.state.db, ids).await;
        self.finish(open_was_deleted);
        match deleted {
            Ok(Some(deleted)) => {
                log::info!("playlists::delete: {} playlist(s)", deleted.playlist_count());
                let playlists_ui = self.playlists_ui.clone();
                self.undo.offer(Removal::Playlists(PlaylistDeleteUndo { deleted, playlists_ui }));
            }
            Ok(None) => {}
            Err(e) => log::warn!("playlists::delete: {}", describe(&e)),
        }
    }

    /// Raises the confirmation, unless another dialog claimed the global since the click. Its
    /// accept arrives at [`Self::for_good`]. The dialog gets a model of its own: the menu's ids can
    /// be the card selection's model, which every pick and clear rewrites in place
    /// (`card_selection::publish`).
    fn confirm_first(&self, claim: DialogClaim, ids: Vec<i32>, reason: NoUndoReason) {
        let Some(ui) = self.weak.upgrade() else { return };
        if !claim.holds(&ui) {
            return;
        }
        let dialog = ui.global::<Dialog>();
        match ids.as_slice() {
            &[id] => dialog.invoke_open_delete_playlist(id, reason),
            _ => dialog.invoke_open_delete_playlists(ModelRc::new(VecModel::from(ids)), reason),
        }
    }

    /// Deletes for good, then raises a notice with no button where the confirmation was the cap's.
    /// The dialog is modal, so Settings answers here as it did at the request.
    fn for_good(&self, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        let open_was_deleted = close_detail_if_among(&self.weak, &self.playlists_ui, &ids);
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            let deleted = library::playlists::delete_playlists(&this.state.db, ids).await;
            this.finish(open_was_deleted);
            let names = match deleted {
                Ok(names) => names,
                Err(e) => {
                    log::warn!("playlists::delete_for_good: {}", describe(&e));
                    return;
                }
            };
            let Some(name) = names.first() else { return };
            log::info!("playlists::delete_for_good: {} playlist(s)", names.len());
            let title = playlists_deleted(names.len(), SharedString::from(name.as_str()));
            this.undo.notify_over_cap(UndoKind::Playlists, names.len(), title);
        }));
        if let Err(e) = spawned {
            log::warn!("playlists::delete_for_good spawn_local: {}", describe(&e));
        }
    }

    /// Runs after every delete, a failed one included: the detail is closed by then, and the
    /// refetch keeps the grid honest about what survived.
    fn finish(&self, open_was_deleted: bool) {
        if open_was_deleted {
            // So a restart doesn't reopen it.
            let s = self.state.clone();
            self.state.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_last_detail_id(&s.paths, view_id::PLAYLIST_DETAIL, None)
                {
                    log::warn!("playlists::delete clear last_detail_id: {}", describe(&e));
                }
            });
        }
        // A deleted id can still be sitting in the card selection, where it would keep being
        // counted by the card menu and asked for by the next batch action.
        if let Some(ui) = self.weak.upgrade() {
            ui.global::<CardSelection>().invoke_clear();
        }
        playlists_ui_mod::refresh_after_write(&self.state, &self.playlists_ui, &self.weak);
    }
}

/// Closes the open detail when its playlist is among `ids`, before the write, so no frame shows a
/// deleted playlist. Reports whether it was.
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
