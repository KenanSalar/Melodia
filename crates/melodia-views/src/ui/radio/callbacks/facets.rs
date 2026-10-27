//! The filter chips' wiring; a pick is a fresh query, the filter being part of the request.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::radio::{RadioUi, facets, suggest};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_request_facet(move |kind| {
            let Some(ui) = weak.upgrade() else { return };
            facets::request(&ui, &s, &ru, kind);
        });
    }

    {
        // No `FilterThrottle`: this narrows a list in memory, with no request to coalesce.
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_facet_filter_changed(move |needle| {
            let Some(ui) = weak.upgrade() else { return };
            facets::filter(&ui, &ru, &needle);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_facet_picked(move |kind, name, code| {
            let Some(ui) = weak.upgrade() else { return };
            facets::pick(&ui, &s, &ru, kind, &name, &code);
        });
    }

    {
        // Clearing is a pick of nothing.
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_clear_facet(move |kind| {
            let Some(ui) = weak.upgrade() else { return };
            facets::pick(&ui, &s, &ru, kind, "", "");
        });
    }

    {
        // Not `facets::pick`: the scope and the emptied box must reach the query as one edit.
        let s = state.clone();
        let ru = radio_ui.clone();
        g.on_suggestion_picked(move |kind, name, code| {
            let Some(ui) = weak.upgrade() else { return };
            suggest::apply(&ui, &s, &ru, kind, &name, &code);
        });
    }
}
