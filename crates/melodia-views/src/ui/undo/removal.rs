//! What the undo slot holds: one arm per surface, each wrapping the token its surface hands over.
//! Each arm's restore is its own surface's, so no surface names another's.

use slint::{ComponentHandle, SharedString};

use crate::ui::equalizer::ResetUndo;
use crate::ui::playlists::PlaylistCutUndo;
use crate::ui::search::RecentClearUndo;
use crate::ui::shell::notifications::RowText;
use crate::ui::util::len_as_i32;
use melodia_app::state::AppState;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, UndoText};

/// Renders the notice in the language active now.
pub(super) type Recipe = Box<dyn Fn(&AppWindow) -> RowText>;

pub enum Removal {
    Equalizer(ResetUndo),
    RecentSearches(RecentClearUndo),
    PlaylistTracks(PlaylistCutUndo),
}

impl Removal {
    /// Takes `next` into this hold when both took from the same place, handing it back otherwise.
    /// A whole-curve hold keeps the older curve: undoing a run newest first ends there anyway.
    pub(super) fn absorb(&mut self, next: Removal) -> Option<Removal> {
        match (self, next) {
            (Removal::Equalizer(_), Removal::Equalizer(_)) => None,
            (Removal::RecentSearches(held), Removal::RecentSearches(next)) => {
                held.absorb(next);
                None
            }
            (Removal::PlaylistTracks(held), Removal::PlaylistTracks(next))
                if held.same_playlist(&next) =>
            {
                held.absorb(next);
                None
            }
            (_, next) => Some(next),
        }
    }

    pub(super) fn recipe(&self) -> Recipe {
        match self {
            Removal::Equalizer(_) => titled(|ui| ui.global::<UndoText>().invoke_equalizer_reset()),
            Removal::RecentSearches(_) => {
                titled(|ui| ui.global::<UndoText>().invoke_recent_searches_cleared())
            }
            Removal::PlaylistTracks(cut) => titled(tracks_removed_from(cut.count(), cut.name())),
        }
    }

    /// Only a restore that writes to the library answers its failure; the others report their own
    /// or can't fail.
    pub(super) async fn restore(
        self,
        state: &AppState,
        ui: &slint::Weak<AppWindow>,
    ) -> Result<(), AppError> {
        match self {
            Removal::Equalizer(reset) => {
                reset.restore();
                Ok(())
            }
            Removal::RecentSearches(cleared) => {
                cleared.restore(state, ui);
                Ok(())
            }
            Removal::PlaylistTracks(cut) => cut.restore(state, ui).await,
        }
    }
}

/// "3 tracks removed from Road Trip", whether the notice can undo it or not.
pub fn tracks_removed_from(
    count: usize,
    playlist: SharedString,
) -> impl Fn(&AppWindow) -> SharedString + 'static {
    let count = len_as_i32(count);
    move |ui| ui.global::<UndoText>().invoke_tracks_removed_from_playlist(count, playlist.clone())
}

/// A notice whose title says it all.
fn titled(title: impl Fn(&AppWindow) -> SharedString + 'static) -> Recipe {
    Box::new(move |ui: &AppWindow| RowText {
        title: title(ui),
        message: SharedString::default(),
        action_label: ui.global::<UndoText>().invoke_undo_label(),
    })
}
