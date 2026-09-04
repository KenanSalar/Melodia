//! The directory grid's own wiring: what a column change, a load-more and a logo lookup do.
//!
//! The card *actions* are not here — they are the same three on every tab, and live in
//! [`super::stations`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::grid_prewarm;
use crate::ui::radio::{RadioTab, RadioUi, browse, kept, mounted_tab};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        // A resize re-chunks the same stations; nothing is re-fetched, the column count being a
        // property of the window rather than of the query. One count for three grids, so this
        // asks the mounted tab rather than being gated on Browse in the `.slint`.
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_| {
            let Some(ui) = weak.upgrade() else { return };
            match mounted_tab(&ui.global::<Radio>()) {
                RadioTab::Browse => browse::apply(&ui, &ru),
                tab => kept::apply(&ui, &ru, tab),
            }
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_load_more(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse::load_more(&ui, &s, &ru);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_retry(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse::retry(&ui, &s, &ru);
        });
    }

    {
        // The card's lazy logo lookup. `grid_cover` is the branch every grid takes: cache-only
        // while the generation is `0`, scheduling past it, and never decoding on this thread.
        let ru = radio_ui.clone();
        g.on_request_logo(move |artwork_path, generation| {
            grid_prewarm::grid_cover(&ru.covers, &artwork_path, generation)
        });
    }
}
