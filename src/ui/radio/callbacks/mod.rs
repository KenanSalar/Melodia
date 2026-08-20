//! Every `Radio.*` callback.
//!
//! Split by what each group answers to: the page itself here, the directory grid in [`browse`],
//! the filter chips in [`facets`], and the section's own arrivals and departures in [`lifecycle`].

mod browse;
mod facets;
mod lifecycle;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::view_tag;
use crate::{AppWindow, Radio};

use super::{RadioUi, filter};

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
        let ru = radio_ui.clone();
        let weak = ui.as_weak();
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            // A tab pick moves no nav index, so `nav_history::record_current` never hears about
            // it — the two curated pages log for the same reason. The bar has already written
            // `tab-idx`, so the tag reads the tab being entered.
            view_tag::log_current(&ui);
            // The needle belongs to the tab, not to the page, so the box follows the mount.
            filter::sync_box(&ui, &ru);
            persist_tab(&s, tab);
        });
    }

    {
        // Behind the view's `FilterThrottle`, so once per settled burst rather than per
        // keystroke — which matters more here than anywhere else in the tree, every burst
        // costing a directory request.
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = ui.as_weak();
        g.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            filter::dispatch(&ui, &s, &ru, &text);
        });
    }

    lifecycle::wire(ui, state, radio_ui);
    browse::wire(ui, state, radio_ui);
    facets::wire(ui, state, radio_ui);
}
