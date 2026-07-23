//! `Genres.*` / `GenreDetail.*` callbacks, split by concern:
//!
//! * [`grid`] — the genre-tile grid (client-side filter / sort, drill-in).
//! * [`detail`] — the open-genre detail view (play, queue, favorite, sort).
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.
//!
//! Mirror of `callbacks/albums` minus everything cover-related: genres have
//! no intrinsic artwork (see the `Genres` global comment in
//! `ui/globals.slint`), so there's no `request-cover` handler, no grid-cover
//! release / prewarm, no `(cover, blur)` pair to clear on detail close.

mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::AppWindow;
use crate::state::AppState;
use crate::ui::genres::GenresUi;

/// Wire every `Genres.*` / `GenreDetail.*` callback to its `library::*`
/// counterpart and the `genres_ui` shared state, plus a `library_changed_tx`
/// subscriber that re-fetches the grid (and refreshes an open detail) on
/// watcher / scan / folder events. Call once after `wire_all` and after
/// `genres::install_genres_models`.
pub fn wire_genres(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    grid::wire(ui, state, genres_ui);
    detail::wire(ui, state, genres_ui);
    lifecycle::wire(ui, state, genres_ui);
}
