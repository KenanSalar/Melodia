//! Every `Radio.*` callback.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::view_tag;
use crate::{AppWindow, Radio};

use super::RadioUi;

/// Write the active tab to `views.json` on the blocking pool. The Slint property is already
/// correct by the time any caller gets here, so this is pure catch-up.
fn persist_tab(state: &AppState, tab: i32) {
    let s = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_radio_tab(&s, tab) {
            log::warn!("radio::set_radio_tab: {e}");
        }
    });
}

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();

    {
        let s = state.clone();
        let weak = ui.as_weak();
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            // A tab pick moves no nav index, so `nav_history::record_current` never hears about
            // it — the two curated pages log for the same reason. The bar has already written
            // `tab-idx`, so the tag reads the tab being entered.
            view_tag::log_current(&ui);
            persist_tab(&s, tab);
        });
    }

    {
        let ru = radio_ui.clone();
        // Mirrors the flag and nothing else: a leave owes `mark_dirty` for exactly what it hands
        // back, and this page holds nothing to hand back yet.
        g.on_section_active_changed(move |active| ru.section.set_active(active));
    }
}
