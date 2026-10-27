//! `Albums.*` / `AlbumDetail.*` callbacks.

mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::ui::albums::AlbumsUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Wires every `Albums.*` / `AlbumDetail.*` callback. Only [`super::install`] calls it, so the
/// models are always in first; `wire_all` has to have run before both.
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
