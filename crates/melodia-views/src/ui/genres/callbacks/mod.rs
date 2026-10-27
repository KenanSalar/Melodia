//! `Genres.*` / `GenreDetail.*` callbacks: `albums/callbacks` less everything a cover needs.

mod detail;
mod grid;
mod lifecycle;

use std::sync::Arc;

use crate::ui::genres::GenresUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Wires every `Genres.*` / `GenreDetail.*` callback, on `albums::callbacks::wire`'s terms.
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
