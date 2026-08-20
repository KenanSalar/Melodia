//! The filter chips: which list a picker shows, and what a pick does to the query.
//!
//! A pick is a fresh query rather than a narrowing of what is loaded — the filter is part of the
//! request, so there is nothing on screen to re-derive it from.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::radio::{RadioUi, facets};
use crate::{AppWindow, Radio};

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
        // Not behind a `FilterThrottle`: this narrows a list already in memory, so there is no
        // request to coalesce and the settled-value hop would only make the picker feel slow.
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
        // Clearing is a pick of nothing rather than its own path: "no country" and "this
        // country" are the same edit with different values.
        let s = state.clone();
        let ru = radio_ui.clone();
        g.on_clear_facet(move |kind| {
            let Some(ui) = weak.upgrade() else { return };
            facets::pick(&ui, &s, &ru, kind, "", "");
        });
    }
}
