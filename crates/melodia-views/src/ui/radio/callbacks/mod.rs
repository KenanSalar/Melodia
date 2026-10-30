//! Every `Radio.*` callback, split by what each group answers to; [`files`] wires from boot.

mod browse;
mod detail;
mod facets;
pub(super) mod files;
mod kept;
mod lifecycle;
mod removal;
mod stations;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::view_ctx::ViewCtx;
use crate::ui::view_tag;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::Radio;

use super::{RadioTab, RadioUi, filter, tab_from_index};

pub(crate) use removal::StationRemovalUndo;

/// Writes the active tab to `views.json`, ordered through `persist`: a reversed pair from a bounce
/// would restore the tab the user only passed through, and that tab's station page with it.
fn persist_tab(state: &AppState, persist: &Arc<IndexPersist>, tab: i32) {
    persist.publish(tab);
    let s = state.clone();
    let persist = Arc::clone(persist);
    state.runtime.spawn_blocking(move || {
        persist.write_if_current(tab, || {
            if let Err(e) = library::settings::set_radio_tab(&s.paths, tab) {
                log::warn!("radio::set_radio_tab: {}", describe(&e));
            }
        });
    });
}

pub(super) fn wire(cx: ViewCtx<'_>, radio_ui: &Arc<RadioUi>) {
    let ViewCtx { app: ui, state, view_state, .. } = cx;
    let g = ui.global::<Radio>();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = ui.as_weak();
        let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            // A tab pick moves no nav index, so `record_current` never logs it; the bar has
            // already written `tab-idx`, so the tag reads the tab entered.
            view_tag::log_current(&ui);
            // The needle belongs to the tab, not to the page, so the box follows the mount.
            filter::sync_box(&ui, &ru);
            let mounted = tab_from_index(&ui.global::<Radio>(), tab);
            // The station page follows the mount too, on this tick. `super::detail` and
            // `super::kept` are the slice's state modules, not the sibling wiring.
            super::detail::reseat(&ui, &ru, mounted);
            super::detail::persist_seat(&s, &ui, &ru);
            // A pick paints from cache; Browse keeps its rows and tier across one.
            if mounted != RadioTab::Browse {
                super::kept::on_tab_entered(&ui, &s, &ru, mounted);
            }
            persist_tab(&s, &persist, tab);
        });
    }

    {
        // Behind the view's `FilterThrottle`: every settled burst costs a directory request.
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
    detail::wire(ui, state, radio_ui);
    facets::wire(ui, state, radio_ui);
    stations::wire(ui, state, radio_ui);
    kept::wire(ui, state, view_state, radio_ui);
    removal::wire(cx, radio_ui);
}
