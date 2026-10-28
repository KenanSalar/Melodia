//! The Search section's leave: the models on the UI thread, then the tiers off it.

use std::sync::Arc;

use slint::ComponentHandle;

use super::NAV_SEARCH;
use crate::ui::search::{SearchUi, apply::teardown_models_on_leave};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Nav, Search};

pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // Seeded here, the gate firing only on a later difference.
    search_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_SEARCH);
    {
        let su = search_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            su.set_section_active(active);
            if !active && let Some(ui) = weak.upgrade() {
                teardown_models_on_leave(&ui);
            }
            if !active {
                let su = su.clone();
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || su.release_section_state());
            }
        });
    }
}
