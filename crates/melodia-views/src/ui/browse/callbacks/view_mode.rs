//! The list-versus-cards toggle, the re-chunk behind it, and the cover lookup the card
//! grid draws through.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::spawn_blocking_logged;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Browse};

pub(super) fn wire(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // The cards are rebuilt from the cached listing without hopping the event loop: a posted
    // rebuild can lose to a redraw, which paints an empty grid.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_toggle_view_mode(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Browse>();
            let mode = bu.view_mode().toggled();
            let mode_idx = browse_ui_mod::mode_index(&g, mode);
            bu.set_view_mode(mode);
            g.set_view_mode(mode_idx);
            browse_ui_mod::rebuild_cards(&ui, &bu);

            // No fetch to hide a prewarm behind, so the cards mount on whatever the tier holds
            // and each miss schedules itself. Toggling away hands the pixels back.
            if mode == browse_ui_mod::BrowseViewMode::Card {
                let bu_prewarm = bu.clone();
                s.runtime.spawn_blocking(move || bu_prewarm.prewarm_card_covers());
            } else {
                s.runtime.spawn_blocking(crate::ui::grid_prewarm::hand_back_covers);
            }

            let s_disk = s.clone();
            spawn_blocking_logged!(
                s_disk,
                "browse::set_view_mode",
                library::settings::set_browse_view_mode(&s_disk.paths, mode_idx)
            );
        });
    }

    // Re-chunks the same cards, and does nothing while the list is mounted, `GridColumnsSync`
    // firing at mount whichever body is up.
    {
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::rebuild_cards(&ui, &bu);
        });
    }

    // `generation` is read for its effect on the binding, never its value.
    g.on_request_card_cover(|path, _generation| crate::ui::grid_prewarm::grid_cover(&path));
}
