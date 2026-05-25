//! `Playlists.*` / `PlaylistDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the playlist-card grid (cover lookup, filter, drill-in,
//!   the Rename-overlay name / description lookups).
//! * [`detail`] — the open-playlist detail view (play, queue, favorite,
//!   sort, drag-reorder, edit-artwork open, track removal).
//! * [`dialog`] — the create / rename / delete / mosaic-artwork CRUD
//!   commits, the add-to-playlist picker, and mosaic-candidate toggling.
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.

mod detail;
mod dialog;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::AppWindow;
use crate::state::AppState;
use crate::ui::playlists::PlaylistsUi;

/// Wire every `Playlists.*` / `PlaylistDetail.*` callback to its
/// `library::*` counterpart and the `playlists_ui` shared state, plus a
/// `library_changed_tx` subscriber that re-fetches the grid (and refreshes
/// an open detail) on watcher / scan / CRUD events. Call once after
/// `wire_all` and after `playlists::install_playlists_models`.
pub fn wire_playlists(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    grid::wire(ui, state, playlists_ui);
    detail::wire(ui, state, playlists_ui);
    dialog::wire(ui, state, playlists_ui);
    lifecycle::wire(ui, state, playlists_ui);
}
