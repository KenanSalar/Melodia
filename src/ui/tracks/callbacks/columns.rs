//! Column-visibility persistence for the Songs tab.

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_blocking_logged;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, Tracks};

/// toggle-column: the popup has already flipped the matching `show-*` flag for instant
/// visual feedback. We persist the new visible-column list to `views.json`'s
/// `view_columns["tracks"]` so the choice survives restarts.
///
/// The Slint flag is the source of truth here — we read it through
/// `TrackListColumnState::snapshot_visible` so the on-disk shape stays consistent with the
/// shutdown-snapshot path.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_toggle_column(move |_id| {
        let Some(ui) = weak.upgrade() else { return };
        let columns = ui.global::<Tracks>().snapshot_visible();
        let s = s.clone();
        spawn_blocking_logged!(s, "tracks::toggle_column",
            library::settings::update_view_columns(
                &s, view_id::TRACKS.to_owned(), columns));
    });
}
