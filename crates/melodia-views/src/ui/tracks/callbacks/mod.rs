//! `Tracks.*` callbacks. Songs is one surface, so [`tracklist`] is the whole view, with no
//! `grid` / `detail` pair beside it.

mod lifecycle;
mod tracklist;

use std::sync::Arc;

use crate::ui::tracks::TracksUi;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::AppWindow;

/// Wires every `Tracks.*` callback, on `albums::callbacks::wire`'s terms.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    tracks_ui: &Arc<TracksUi>,
) {
    lifecycle::wire(ui, state, tracks_ui);
    tracklist::wire(ui, state, view_state, tracks_ui);
}
