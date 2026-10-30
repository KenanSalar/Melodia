//! Taking tracks out of Favorites at once, with an undo, for every removal `FavoriteActions`
//! carries.

use std::collections::HashSet;
use std::future::Future;
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, Model};

use super::{RowFlagMirrors, collect_track_ids};
use crate::ui::undo::{Removal, UndoWindow};
use melodia_app::library::{self, entity_tracks::EntityKind};
use melodia_app::state::AppState;
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, FavoriteActions};

/// The tracks a run of removals un-hearted. Ids only, as a queue cut is, so a hold costs what the
/// selection did and takes no cap. A set, so a track re-hearted and removed again inside the
/// window counts once.
pub struct FavoriteRemovalUndo {
    track_ids: HashSet<i64>,
    mirrors: RowFlagMirrors,
}

impl FavoriteRemovalUndo {
    pub fn absorb(&mut self, newer: Self) {
        self.track_ids.extend(newer.track_ids);
    }

    pub fn count(&self) -> usize {
        self.track_ids.len()
    }

    /// Re-hearts every track that went, one deleted since matching no row.
    pub async fn restore(self, state: &AppState) -> Result<(), AppError> {
        let ids: Vec<i64> = self.track_ids.into_iter().collect();
        library::favorites::set_favorite(state, ids.clone(), true).await?;
        self.mirrors.favorites(&ids, true);
        Ok(())
    }
}

pub fn wire(ui: &AppWindow, state: &AppState, undo: &Rc<UndoWindow>, mirrors: &RowFlagMirrors) {
    let remover =
        Unfavoriter { state: state.clone(), undo: undo.clone(), mirrors: mirrors.clone() };
    let actions = ui.global::<FavoriteActions>();
    {
        let remover = remover.clone();
        actions.on_remove_tracks(move |ids| remover.remove(collect_track_ids(&ids)));
    }
    actions.on_remove_entities(move |kind, ids| {
        let Some(kind) = EntityKind::from_token(kind.as_str()) else {
            log::warn!("favorites::remove: unknown card kind {kind}");
            return;
        };
        remover.remove_entities(kind, ids.iter().map(i64::from).collect());
    });
}

/// What a removal reaches. Every path runs on the UI thread, where the undo slot lives.
#[derive(Clone)]
struct Unfavoriter {
    state: AppState,
    undo: Rc<UndoWindow>,
    mirrors: RowFlagMirrors,
}

impl Unfavoriter {
    fn remove(&self, track_ids: Vec<i64>) {
        let this = self.clone();
        spawn_on_ui(async move { this.take_out(track_ids).await });
    }

    /// A card's tracks, of which only the hearted ones go.
    fn remove_entities(&self, kind: EntityKind, entity_ids: Vec<i64>) {
        let this = self.clone();
        spawn_on_ui(async move {
            match library::entity_tracks::track_ids_for(&this.state.db, kind, &entity_ids).await {
                Ok(track_ids) => this.take_out(track_ids).await,
                Err(e) => log::warn!("favorites::remove: {}", describe(&e)),
            }
        });
    }

    async fn take_out(&self, track_ids: Vec<i64>) {
        let cleared = match library::favorites::unfavorite(&self.state, track_ids).await {
            Ok(cleared) => cleared,
            Err(e) => {
                log::warn!("favorites::remove: {}", describe(&e));
                return;
            }
        };
        if cleared.is_empty() {
            return;
        }
        self.mirrors.favorites(&cleared, false);
        let mirrors = self.mirrors.clone();
        let track_ids = cleared.into_iter().collect();
        self.undo.offer(Removal::Favorites(FavoriteRemovalUndo { track_ids, mirrors }));
    }
}

fn spawn_on_ui(removal: impl Future<Output = ()> + 'static) {
    if let Err(e) = slint::spawn_local(Compat::new(removal)) {
        log::warn!("favorites::remove spawn_local: {}", describe(&e));
    }
}
