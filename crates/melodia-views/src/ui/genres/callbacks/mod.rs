//! `Genres.*` / `GenreDetail.*` callbacks: [`grid`] (the tile grid — client-side
//! filter / sort, drill-in), [`detail`] (play, queue, favorite, sort) and
//! [`lifecycle`] (section enter/leave caches plus the `library_changed` re-fetch
//! subscriber).
//!
//! A mirror of `albums/callbacks` minus everything cover-related — genres have no
//! intrinsic artwork — so there is no `request-cover` handler, no grid-cover release
//! or prewarm, and no `(cover, blur)` pair to clear on detail close.

mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::AppWindow;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::ui::genres::GenresUi;

/// Wire every `Genres.*` / `GenreDetail.*` callback to its `library::*` counterpart
/// and the `genres_ui` shared state, plus a `library_changed` subscriber that
/// re-fetches the grid and refreshes an open detail on watcher / scan / folder events.
///
/// Called by [`super::install`], which is what guarantees the models are in place first — a
/// pairing, rather than two statements a boot-file reorder could separate. `wire_all` still has to
/// have run before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    genres_ui: &Arc<GenresUi>,
) {
    grid::wire(ui, state, view_state, genres_ui);
    detail::wire(ui, state, genres_ui);
    lifecycle::wire(ui, state, genres_ui);
}
