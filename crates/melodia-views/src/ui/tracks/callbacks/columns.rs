//! Column-visibility persistence for the Songs tab.

use slint::ComponentHandle;

use crate::ui::track_list_view;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Tracks};

/// toggle-column: the popup has already flipped the matching `show-*` flag for instant
/// visual feedback, so what is left is persisting the new visible set to `views.json`.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_toggle_column(move |_id| {
        let Some(ui) = weak.upgrade() else { return };
        track_list_view::persist_visible(&s, &ui.global::<Tracks>());
    });
}
