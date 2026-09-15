//! Column-visibility persistence for the Browse file list.

use slint::ComponentHandle;

use crate::ui::track_list_view;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Browse};

/// toggle-column: the popup already flipped the matching `show-*` flag for instant visual
/// feedback. Persist the new visible-column list under Browse's own key, so Browse keeps a column
/// layout apart from the Songs tab.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Browse>().on_toggle_column(move |_id| {
        let Some(ui) = weak.upgrade() else { return };
        track_list_view::persist_visible(&s, &ui.global::<Browse>());
    });
}
