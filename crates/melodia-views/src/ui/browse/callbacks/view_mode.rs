//! The list-versus-cards toggle, the re-chunk behind it, and the private cover tier the
//! card grid draws from.

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

    // toggle-view-mode: the pill means "switch", so Rust negates. The card model is
    // rebuilt from the cached listing rather than re-fetched, and **without hopping the
    // event loop** — `invoke_from_event_loop` posts even when called from the UI thread,
    // and a redraw winning that race paints an empty grid.
    //
    // A toggle is the one path with no fetch to await before the grid mounts, so it takes
    // the `covers-generation` pair: rewind to 0 so the mounting cards ask the tier
    // cache-only, warm a screenful off-thread, then bump — gated on the view still being
    // where the prewarm left it.
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

            // The toggle has no fetch to hide a prewarm behind, so the cards mount against
            // whatever the tier holds and each miss schedules itself; the shared notifier
            // brings them back. Toggling *away* hands the pixels back as proxies.
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
                library::settings::set_browse_view_mode(&s_disk, mode_idx)
            );
        });
    }

    // columns-changed: the grid re-flowed, so re-chunk the same cards into rows of the new
    // width. No fetch, no DB — and a no-op while the list is mounted, `GridColumnsSync`
    // firing at mount regardless of which body is up.
    {
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::rebuild_cards(&ui, &bu);
        });
    }

    // request-card-cover: one card's thumbnail off the shared grid tier. `generation` is read
    // for its effect on the binding, never its value.
    g.on_request_card_cover(|path, _generation| crate::ui::grid_prewarm::grid_cover(&path));
}
