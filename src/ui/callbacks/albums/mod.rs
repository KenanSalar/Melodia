//! `Albums.*` / `AlbumDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the album-card grid (cover lookup, filter / sort, drill-in).
//! * [`detail`] — the open-album detail view (play, queue, favorite, sort).
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.

mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::AppWindow;
use crate::state::AppState;
use crate::ui::albums::AlbumsUi;

/// Wire every `Albums.*` / `AlbumDetail.*` callback to its `library::*`
/// counterpart and the `albums_ui` shared state, plus a
/// `library_changed_tx` subscriber that re-fetches the grid (and refreshes
/// an open detail) on watcher / scan / folder events. Call once after
/// `wire_all` and after `albums::install_albums_models`.
pub fn wire_albums(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    grid::wire(ui, state, albums_ui);
    detail::wire(ui, state, albums_ui);
    lifecycle::wire(ui, state, albums_ui);
}
