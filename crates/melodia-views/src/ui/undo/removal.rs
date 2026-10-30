//! What the undo slot holds: one arm per surface, each wrapping the token its surface hands over.
//! Each arm's restore is its own surface's, so no surface names another's.

use slint::{ComponentHandle, SharedString};

use crate::ui::equalizer::ResetUndo;
use crate::ui::playlists::{PlaylistCutUndo, PlaylistDeleteUndo};
use crate::ui::queue_panel::{QueueClearUndo, QueueCutUndo, QueueReplaceUndo};
use crate::ui::radio::StationRemovalUndo;
use crate::ui::search::RecentClearUndo;
use crate::ui::shell::notifications::RowText;
use crate::ui::util::len_as_i32;
use melodia_app::state::AppState;
use melodia_core::entities::radio::KeptList;
use melodia_core::entities::undo::UNDO_HOLD_CAP;
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, UndoText};

/// Renders the notice in the language active now.
pub(super) type Recipe = Box<dyn Fn(&AppWindow) -> RowText>;

/// Renders a title alone, for a notice that can't undo.
pub(super) type Title = Box<dyn Fn(&AppWindow) -> SharedString>;

pub enum Removal {
    Equalizer(ResetUndo),
    RecentSearches(RecentClearUndo),
    PlaylistTracks(PlaylistCutUndo),
    Playlists(PlaylistDeleteUndo),
    RadioStations(StationRemovalUndo),
    QueueRows(QueueCutUndo),
    QueueCleared(QueueClearUndo),
    QueueReplaced(QueueReplaceUndo),
}

/// How a later removal lands on the one held.
pub(super) enum Absorbed {
    Merged,
    /// It took from somewhere else, so it replaces the hold.
    Distinct(Removal),
    /// Together they would hold more than [`UNDO_HOLD_CAP`] records, so none of the run can be
    /// undone: undoing only its newest part would leave the notice's count wrong.
    OverCap(Title),
}

impl Removal {
    /// Takes `next` into this hold when both took from the same place. A hold of a whole state, the
    /// curve or a replaced queue, keeps the older one: undoing a run newest first ends there anyway.
    pub(super) fn absorb(&mut self, next: Removal) -> Absorbed {
        match (self, next) {
            (Removal::Equalizer(_), Removal::Equalizer(_))
            | (Removal::QueueReplaced(_), Removal::QueueReplaced(_)) => Absorbed::Merged,
            (Removal::RecentSearches(held), Removal::RecentSearches(next)) => {
                held.absorb(next);
                Absorbed::Merged
            }
            (Removal::PlaylistTracks(held), Removal::PlaylistTracks(next))
                if held.same_playlist(&next) =>
            {
                let rows = held.count() + next.count();
                if rows > UNDO_HOLD_CAP {
                    return Absorbed::OverCap(Box::new(tracks_removed_from(rows, next.name())));
                }
                held.absorb(next);
                Absorbed::Merged
            }
            (Removal::Playlists(held), Removal::Playlists(next)) => {
                if held.items() + next.items() > UNDO_HOLD_CAP {
                    let count = held.count() + next.count();
                    return Absorbed::OverCap(Box::new(playlists_deleted(count, next.name())));
                }
                held.absorb(next);
                Absorbed::Merged
            }
            (Removal::RadioStations(held), Removal::RadioStations(next))
                if held.same_list(&next) =>
            {
                let count = held.count() + next.count();
                if count > UNDO_HOLD_CAP {
                    let title = stations_removed(next.list(), count, next.name());
                    return Absorbed::OverCap(Box::new(title));
                }
                held.absorb(next);
                Absorbed::Merged
            }
            // No cap check, which `QueueCut` and `take_cleared` argue.
            (Removal::QueueRows(held), Removal::QueueRows(next)) => {
                held.absorb(next);
                Absorbed::Merged
            }
            (Removal::QueueCleared(held), Removal::QueueCleared(next)) => {
                held.absorb(next);
                Absorbed::Merged
            }
            (_, next) => Absorbed::Distinct(next),
        }
    }

    pub(super) fn recipe(&self) -> Recipe {
        match self {
            Removal::Equalizer(_) => titled(|ui| ui.global::<UndoText>().invoke_equalizer_reset()),
            Removal::RecentSearches(_) => {
                titled(|ui| ui.global::<UndoText>().invoke_recent_searches_cleared())
            }
            Removal::PlaylistTracks(cut) => titled(tracks_removed_from(cut.count(), cut.name())),
            Removal::Playlists(deleted) => {
                titled(playlists_deleted(deleted.count(), deleted.name()))
            }
            Removal::RadioStations(removed) => {
                titled(stations_removed(removed.list(), removed.count(), removed.name()))
            }
            Removal::QueueRows(cut) => {
                let count = len_as_i32(cut.count());
                titled(move |ui| ui.global::<UndoText>().invoke_tracks_removed_from_queue(count))
            }
            Removal::QueueCleared(_) => titled(|ui| ui.global::<UndoText>().invoke_queue_cleared()),
            Removal::QueueReplaced(_) => {
                titled(|ui| ui.global::<UndoText>().invoke_queue_replaced())
            }
        }
    }

    /// Answers a failed write, for the one failure notice. The equalizer's and the queue's can't
    /// fail here: the first's settings write logs its own, and the queue's arms write nothing, a
    /// replace's resume reporting an unplayable track as any start does.
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
            Removal::RecentSearches(cleared) => cleared.restore(state, ui).await,
            Removal::PlaylistTracks(cut) => cut.restore(state, ui).await,
            Removal::Playlists(deleted) => deleted.restore(state, ui).await,
            Removal::RadioStations(removed) => removed.restore(state, ui).await,
            Removal::QueueRows(cut) => {
                cut.restore(state);
                Ok(())
            }
            Removal::QueueCleared(cleared) => {
                cleared.restore(state);
                Ok(())
            }
            Removal::QueueReplaced(replaced) => {
                replaced.restore(state);
                Ok(())
            }
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

/// "Road Trip deleted", or "3 playlists deleted", whether the notice can undo it or not.
pub fn playlists_deleted(
    count: usize,
    name: SharedString,
) -> impl Fn(&AppWindow) -> SharedString + 'static {
    let count = len_as_i32(count);
    move |ui| ui.global::<UndoText>().invoke_playlists_deleted(count, name.clone())
}

/// "Jazz FM removed from Favorites", or "3 stations removed from Recently Played", whether the
/// notice can undo it or not.
pub fn stations_removed(
    from: KeptList,
    count: usize,
    name: SharedString,
) -> impl Fn(&AppWindow) -> SharedString + 'static {
    let count = len_as_i32(count);
    move |ui| {
        let text = ui.global::<UndoText>();
        match from {
            KeptList::Favorites => text.invoke_stations_removed_from_favorites(count, name.clone()),
            KeptList::Recent => text.invoke_stations_removed_from_recent(count, name.clone()),
        }
    }
}

/// A notice whose title says it all.
fn titled(title: impl Fn(&AppWindow) -> SharedString + 'static) -> Recipe {
    Box::new(move |ui: &AppWindow| RowText {
        title: title(ui),
        message: SharedString::default(),
        action_label: ui.global::<UndoText>().invoke_undo_label(),
    })
}
