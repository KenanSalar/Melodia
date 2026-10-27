//! `Artists.*` / `ArtistDetail.*` callbacks.

mod cross_tab;
mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Wires every `Artists.*` / `ArtistDetail.*` callback, on `albums::callbacks::wire`'s terms,
/// `albums_ui` being the strip's open-album hand-off.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    artists_ui: &Arc<ArtistsUi>,
    albums_ui: &Arc<AlbumsUi>,
) {
    grid::wire(ui, state, view_state, artists_ui);
    detail::wire(ui, state, artists_ui);
    cross_tab::wire(ui, state, albums_ui);
    lifecycle::wire(ui, state, artists_ui);
}
