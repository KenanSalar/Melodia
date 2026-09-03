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
use crate::ui::albums::AlbumsUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;

/// Wire every `Albums.*` / `AlbumDetail.*` callback to its `library::*`
/// counterpart and the `albums_ui` shared state, plus a
/// `library_changed` subscriber that re-fetches the grid (and refreshes
/// an open detail) on watcher / scan / folder events.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    albums_ui: &Arc<AlbumsUi>,
) {
    grid::wire(ui, state, view_state, albums_ui);
    detail::wire(ui, state, albums_ui);
    lifecycle::wire(ui, state, albums_ui);
}
