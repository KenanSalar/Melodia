//! The Recently Played section's enter and leave, its refresh subscriber and its first fetch.

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::detail_view::{release_hero_slots, release_shared_hero};
use crate::ui::list_selection;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::NAV_RECENTLY_PLAYED;
use crate::ui::recently_played::{
    self as recently_played_ui_mod, RecentlyPlayedTab, RecentlyPlayedUi,
};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, Nav, RecentlyPlayed, TrackListRow as UiTrackListRow};

pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // Seeded here, the gate firing only on a later difference.
    rp_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_RECENTLY_PLAYED);
    {
        let ru = rp_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            ru.set_section_active(active);
            if !active {
                // Before the release is spawned, so a re-enter's `take_dirty()` can't miss it.
                ru.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // The banner's slots hold their buffers past the LRU, so they go back here.
                let g = ui.global::<RecentlyPlayed>();
                release_hero_slots(&g);
                ru.forget_mosaic();
                release_shared_hero(&ui);
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "recently_played: clear tracks");
                recently_played_ui_mod::release_grid(&ui, &ru);
                list_selection::clear_row_selection(&g);
                // The card selection goes back with the models (`card-selection.slint`).
                ui.global::<CardSelection>().invoke_clear();
            }
            let ru = ru.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if ru.take_dirty() {
                        kick_full_refresh(&s, &ru, &weak).await;
                    }
                });
            } else {
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || ru.release_section_state());
            }
        });
    }

    // `stats_changed` follows a play-count flush, which writes both tabs' ordering keys. Its own
    // loop because it selects over both channels, where `ui::signal::on_signal` takes one.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed.subscribe();
        let mut stats_rx = state.stats_changed.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            loop {
                tokio::select! {
                    changed = library_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    changed = stats_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
                if !ru.section_active() {
                    ru.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &ru, &weak).await;
            }
        }));
    }

    // A boot onto this page gets no edge, so it fetches here; one elsewhere fetches on its first
    // enter.
    rp_ui.mark_dirty();
    if rp_ui.section_active() {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        state.runtime.clone().spawn(async move {
            if ru.take_dirty() {
                kick_full_refresh(&s, &ru, &weak).await;
            }
        });
    }
}

/// Refreshes the recency list, which the band reads on both tabs, and the grid only while it is
/// mounted, a tick skipped on Songs going on [`RecentlyPlayedUi::mark_grid_dirty`].
async fn kick_full_refresh(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &slint::Weak<AppWindow>,
) {
    if rp_ui.active_tab() == RecentlyPlayedTab::MostPlayed {
        // Consumed by the load it schedules, so a boot onto the grid loads once.
        rp_ui.take_grid_dirty();
        let s = state.clone();
        let ru = Arc::clone(rp_ui);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            recently_played_ui_mod::reload_grid(&ui, &s, &ru);
        });
    } else {
        rp_ui.mark_grid_dirty();
    }
    if let Err(e) = recently_played_ui_mod::refresh_tracks(state, rp_ui, weak).await {
        log::warn!("recently_played::refresh_tracks: {}", describe(&e));
    }
}
