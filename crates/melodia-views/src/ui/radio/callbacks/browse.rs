//! The directory grid's own wiring; the card actions, the same on every tab, are
//! [`super::stations`]'s.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::radio::{RadioTab, RadioUi, browse, covers, kept, mounted_tab};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Radio};

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        // A resize re-chunks the same stations without a fetch. One count serves three grids, so
        // this asks the mounted tab.
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
        let ru = radio_ui.clone();
        g.on_request_logo(move |artwork_path, generation| {
            covers::logo_cover(&ru, &artwork_path, generation)
        });
    }
}
