//! Every `Radio.*` callback.
//!
//! Split by what each group answers to: the page itself here, the directory grid in [`browse`],
//! the station page in [`detail`], the filter chips in [`facets`], the kept tabs in [`kept`], the
//! row actions every tab shares in [`stations`], and the section's own arrivals and departures in
//! [`lifecycle`].
//!
//! [`files`] is the exception and wires from `main()`, its toasts needing a stack that does not
//! exist at install time.

mod browse;
mod detail;
mod facets;
pub(super) mod files;
mod kept;
mod lifecycle;
mod stations;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::ui::callbacks::index_persist::IndexPersist;
use crate::ui::view_tag;
use crate::{AppWindow, Radio};

use super::{RadioTab, RadioUi, filter, tab_from_index};

/// Write the active tab to `views.json` on the blocking pool. The Slint property is already
/// correct by the time any caller gets here, so this is pure catch-up.
///
/// Ordered through `persist`, the seat write's sibling: a bounce queues a value per pick and two
/// blocking tasks have none of their own, so a reversed pair restores the tab the user only
/// passed through — and the seat that tab reseats decides which station page comes back with it.
///
/// UI thread only, for the publish.
fn persist_tab(state: &AppState, persist: &Arc<IndexPersist>, tab: i32) {
    persist.publish(tab);
    let s = state.clone();
    let persist = Arc::clone(persist);
    state.runtime.spawn_blocking(move || {
        persist.write_if_current(tab, || {
            if let Err(e) = library::settings::set_radio_tab(&s, tab) {
                log::warn!("radio::set_radio_tab: {e}");
            }
        });
    });
}

pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    radio_ui: &Arc<RadioUi>,
) {
    let g = ui.global::<Radio>();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = ui.as_weak();
        let persist = Arc::new(IndexPersist::new(g.get_tab_idx()));
        g.on_tab_changed(move |tab| {
            let Some(ui) = weak.upgrade() else { return };
            // A tab pick moves no nav index, so `nav_history::record_current` never hears about
            // it — the two curated pages log for the same reason. The bar has already written
            // `tab-idx`, so the tag reads the tab being entered.
            view_tag::log_current(&ui);
            // The needle belongs to the tab, not to the page, so the box follows the mount.
            filter::sync_box(&ui, &ru);
            let mounted = tab_from_index(&ui.global::<Radio>(), tab);
            // **The station page belongs to the tab the same way**, and this is the one move that
            // can leave one behind. Synchronously and on this tick: `tab-idx` has already moved,
            // so a hop would let the view's own `changed` trackers read a `detail-open` that is
            // briefly neither tab's answer. (`super::detail` is the slice's state module; the
            // sibling `detail` here is its wiring.)
            super::detail::reseat(&ui, &ru, mounted);
            // Which station `views.json` names follows the mount too — with a page per tab, "the
            // last one opened" stopped being "the one the restored tab is showing".
            super::detail::persist_seat(&s, &ui, &ru);
            // A pick paints from cache and asks for nothing: the fetch is the section enter's.
            // Browse keeps its own rows and its own warm tier across a pick, so only the two
            // local tabs have anything to do here.
            if mounted != RadioTab::Browse {
                // `super::kept` is the slice's state module; the sibling `kept` here is its
                // wiring.
                super::kept::on_tab_entered(&ui, &s, &ru, mounted);
            }
            persist_tab(&s, &persist, tab);
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
    detail::wire(ui, state, radio_ui);
    facets::wire(ui, state, radio_ui);
    stations::wire(ui, state, radio_ui);
    kept::wire(ui, state, view_state, radio_ui);
}
