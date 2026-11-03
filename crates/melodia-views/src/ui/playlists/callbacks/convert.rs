//! Converting a smart playlist to a regular one, from its card menu or its page. It converts at
//! once and offers the undo that brings the rules back, asking first only where Settings turned
//! undo off for playlists, the rules then having no way back.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};

use super::spawn_refresh_after_edit;
use crate::ui::callbacks::DialogClaim;
use crate::ui::playlists::PlaylistsUi;
use crate::ui::undo::{Removal, UndoWindow};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::playlist::DroppedRules;
use melodia_core::entities::smart_criteria::Draw;
use melodia_core::entities::undo::{RemovalSize, UndoKind};
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, Dialog, Playlists};

/// The card entry and the page's pill both arrive at `convert-to-regular`, and the confirmation
/// at `convert-to-regular-confirmed`.
pub(super) fn wire(cx: ViewCtx<'_>, playlists_ui: &Arc<PlaylistsUi>) {
    let converter = Converter {
        state: cx.state.clone(),
        playlists_ui: playlists_ui.clone(),
        undo: cx.undo.clone(),
        weak: cx.app.as_weak(),
    };
    let playlists = cx.app.global::<Playlists>();
    {
        let converter = converter.clone();
        playlists.on_convert_to_regular(move |id| converter.request(id));
    }
    playlists.on_convert_to_regular_confirmed(move |id| converter.for_good(i64::from(id)));
}

/// The rules a convert dropped, which its undo writes back.
pub struct PlaylistConvertUndo {
    dropped: DroppedRules,
    playlists_ui: Arc<PlaylistsUi>,
}

impl PlaylistConvertUndo {
    pub fn name(&self) -> SharedString {
        SharedString::from(self.dropped.name.as_str())
    }

    /// Makes the playlist smart again, then refreshes as an edit does.
    pub async fn restore(self, state: &AppState, weak: &Weak<AppWindow>) -> Result<(), AppError> {
        library::smart_playlists::restore_rules(&state.db, &self.dropped).await?;
        let id = self.dropped.playlist.id;
        spawn_refresh_after_edit(state, &self.playlists_ui, weak, id, "playlists::undo_convert");
        Ok(())
    }
}

/// What a convert reaches. Every path runs on the UI thread, where the undo slot lives.
#[derive(Clone)]
struct Converter {
    state: AppState,
    playlists_ui: Arc<PlaylistsUi>,
    undo: Rc<UndoWindow>,
    weak: Weak<AppWindow>,
}

impl Converter {
    fn request(&self, id: i32) {
        if !self.undo.covers(UndoKind::Playlists, RemovalSize::Whole) {
            self.confirm_first(id);
            return;
        }
        let draw = self.playlists_ui.draw_on_show(i64::from(id));
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            let Some(dropped) = this.convert(i64::from(id), draw).await else { return };
            let playlists_ui = this.playlists_ui.clone();
            this.undo.offer(Removal::PlaylistRules(PlaylistConvertUndo { dropped, playlists_ui }));
        }));
        if let Err(e) = spawned {
            log::warn!("playlists::convert spawn_local: {}", describe(&e));
        }
    }

    /// A tick later, a `Dialog` write inside the click tripping Slint's recursion guard. Its
    /// accept arrives at [`Self::for_good`].
    fn confirm_first(&self, id: i32) {
        let Some(claim) = DialogClaim::take_from(&self.weak) else { return };
        let raised = self.weak.upgrade_in_event_loop(move |ui| {
            if claim.holds(&ui) {
                ui.global::<Dialog>().invoke_open_convert_playlist(id);
            }
        });
        if let Err(e) = raised {
            log::warn!("playlists::convert confirm: {}", describe(&e));
        }
    }

    /// Converts with no notice, the confirmation having said there is no undo. The dialog is
    /// modal, so the draw on show is still the one the request saw.
    fn for_good(&self, id: i64) {
        let draw = self.playlists_ui.draw_on_show(id);
        let this = self.clone();
        let spawned = slint::spawn_local(Compat::new(async move {
            this.convert(id, draw).await;
        }));
        if let Err(e) = spawned {
            log::warn!("playlists::convert_for_good spawn_local: {}", describe(&e));
        }
    }

    /// The rules it dropped, `None` where nothing changed: the playlist is gone, was already
    /// regular, or the write failed.
    async fn convert(&self, id: i64, draw: Draw) -> Option<DroppedRules> {
        match library::smart_playlists::convert_to_regular(&self.state.db, id, draw).await {
            Ok(Some(dropped)) => {
                log::info!("playlists::convert({id})");
                spawn_refresh_after_edit(
                    &self.state,
                    &self.playlists_ui,
                    &self.weak,
                    id,
                    "playlists::convert",
                );
                Some(dropped)
            }
            Ok(None) => None,
            Err(e) => {
                log::warn!("playlists::convert({id}): {}", describe(&e));
                None
            }
        }
    }
}
