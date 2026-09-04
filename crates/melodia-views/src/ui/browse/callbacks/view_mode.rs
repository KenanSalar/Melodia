//! The list-versus-cards toggle, the re-chunk behind it, and the private cover tier the
//! card grid draws from.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::spawn_blocking_logged;
use crate::ui::tab_bar::should_announce_warm;
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
            g.set_covers_generation(0);
            browse_ui_mod::rebuild_cards(&ui, &bu);

            let bu_work = bu.clone();
            let weak_bump = weak.clone();
            if mode == browse_ui_mod::BrowseViewMode::Card {
                s.runtime.spawn(async move {
                    let bu_prewarm = bu_work.clone();
                    // A `JoinError` is the same "we don't know" as a prewarm that handed
                    // its buffers back.
                    // `Some(Card)` is "we decoded for the card tier and still hold it" —
                    // the shape `should_announce_warm` takes, with the mode standing in
                    // for the tab a grid page would pass.
                    let warmed =
                        tokio::task::spawn_blocking(move || bu_prewarm.prewarm_card_covers())
                            .await
                            .unwrap_or(false)
                            .then_some(browse_ui_mod::BrowseViewMode::Card);
                    let _ = weak_bump.upgrade_in_event_loop(move |ui| {
                        // Both shadows are written on this thread, so this is the same
                        // re-check the prewarm made, against anything that landed after
                        // it returned.
                        if should_announce_warm(
                            warmed,
                            bu_work.section_active(),
                            bu_work.view_mode(),
                        ) {
                            let g = ui.global::<Browse>();
                            g.set_covers_generation(g.get_covers_generation() + 1);
                        }
                    });
                });
            } else {
                let bu_release = bu.clone();
                s.runtime.spawn_blocking(move || bu_release.release_grid_covers());
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

    // request-card-cover: one card's thumbnail off Browse's own grid tier, decoded only
    // once `covers-generation` says the tier is warm.
    {
        let bu = browse_ui.clone();
        g.on_request_card_cover(move |path, generation| bu.grid_cover(&path, generation));
    }
}
