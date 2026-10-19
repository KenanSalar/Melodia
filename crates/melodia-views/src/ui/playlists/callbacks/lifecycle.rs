//! Playlists section lifecycle: the `section-active-changed` enter/leave
//! handler (release + reload) and the `library_changed` and `stats_changed`
//! subscribers that keep the grid + open detail fresh.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::card_grid;
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::grid_prewarm;
use crate::ui::list_selection;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::playlists::grid::PlaylistsGrid;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::signal::on_signal;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::ui::track_detail::{self, TrackDetail, restore};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, PlaylistDetail, Playlists};

/// Wire the Playlists section-lifecycle callbacks. See
/// [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    playlists_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Playlists));
    wire_section_gate(ui, state, playlists_ui);
    install_library_refresher(ui, state, playlists_ui);
    install_stats_refresher(ui, state, playlists_ui);
}

/// Mirrors the Albums implementation — on leave, release the grid and the
/// detail's Slint state (UI thread) then the covers and the detail artwork
/// off-thread; on return, [`reenter`].
fn wire_section_gate(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let pu = playlists_ui.clone();
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_section_active_changed(move |active| {
        pu.set_section_active(active);
        if !active {
            pu.mark_dirty();
        }
        if !active && let Some(ui) = weak.upgrade() {
            // Rewound on the same tick as the grid it numbers;
            // `Albums.total-count`'s declaration argues the sentinel.
            ui.global::<Playlists>().set_total_count(UNFETCHED_COUNT);
            card_grid::release::<PlaylistsGrid>(&ui, &pu);

            let d = ui.global::<PlaylistDetail>();
            release_detail_hero_images(&ui, &d);
            track_detail::release_rows(&*pu, &d);
            list_selection::clear_row_selection(&d);
            // The card grid's set goes back with the models it describes;
            // `card-selection.slint` argues why it cannot outlive the leave.
            ui.global::<CardSelection>().invoke_clear();
        }
        if active {
            restore::resume_deferred(&weak, &s, &pu, playlists_ui_mod::restore_detail);
            if let Some(ui) = weak.upgrade() {
                reenter(&ui, &s, &pu);
            }
        } else {
            let pu = pu.clone();
            s.runtime.spawn_blocking(move || pu.release_section_state());
        }
    });
}

/// The section came back on screen: a reload if a leave released the grid,
/// else just a prewarm of its covers. The detail the release emptied is
/// reopened beside the reload.
fn reenter(ui: &AppWindow, s: &AppState, pu: &Arc<PlaylistsUi>) {
    if !pu.take_dirty() {
        let pu = pu.clone();
        s.runtime.spawn_blocking(move || pu.prewarm_visible_covers());
        return;
    }
    playlists_ui_mod::reload_grid(ui, s, pu);
    let open_id = pu.detail_playlist_id();
    if open_id < 0 {
        return;
    }
    let s = s.clone();
    let pu = pu.clone();
    let weak = ui.as_weak();
    s.runtime.clone().spawn(async move {
        let reopened = playlists_ui_mod::open_playlist(
            &s,
            &pu,
            weak.clone(),
            open_id,
            melodia_ui::NavEnterFrom::Right,
        )
        .await;
        let Err(e) = reopened else { return };
        log::warn!("playlists::section_enter open_playlist({open_id}): {}", describe(&e));
        pu.clear_detail();
        // Handed back warm, as a failed restore's grid is.
        grid_prewarm::prewarm_off_thread(&pu, PlaylistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            let g = ui.global::<PlaylistDetail>();
            g.set_playlist_id(-1);
            release_detail_hero_images(&ui, &g);
        });
    });
}

/// Scans, the watcher and a smart playlist's rule edit bump `library_changed`,
/// every other playlist write refreshing through `refresh_after_write`. Mirror
/// the Albums pattern: reload the grid and refresh an open detail.
fn install_library_refresher(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let installed =
        on_signal(&state.library_changed, ui.as_weak(), "playlists-library-changed", move |ui| {
            // Skip the in-place refresh when the section is hidden, but
            // mark the grid dirty so the next section-enter reloads.
            // Without the `mark_dirty`, a `library_changed` arriving while
            // the section was never visited (e.g. the first scan after a
            // fresh-DB launch) would be lost. Mirrors the same gate in
            // `albums/callbacks/lifecycle.rs`.
            if !pu.section_active() {
                pu.mark_dirty();
                return;
            }
            playlists_ui_mod::reload_grid(ui, &s, &pu);
            let open_id = pu.detail_playlist_id();
            if open_id >= 0 {
                let s = s.clone();
                let pu = pu.clone();
                let weak = ui.as_weak();
                spawn_logged!(
                    s,
                    "playlists::library_changed_detail",
                    playlists_ui_mod::refresh_detail(&s, &pu, weak, open_id)
                );
            }
        });
    if let Err(e) = installed {
        log::warn!("Playlists won't follow library changes: {}", describe(&e));
    }
}

/// Play-count / last-played flushes bump `stats_changed` (NOT
/// `library_changed`, by design, to avoid churning every view on each played
/// song). Only smart playlists whose rules depend on play stats ("most
/// played", "recently played", "never played") can change, so only those are
/// recounted — a static-rule ("Genre is Rock") smart playlist is neither woken
/// nor recounted here (it can't have moved).
fn install_stats_refresher(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let installed =
        on_signal(&state.stats_changed, ui.as_weak(), "playlists-stats-changed", move |ui| {
            // Hidden section: the section-enter path reloads on return.
            if pu.section_active() {
                playlists_ui_mod::refresh_play_stats(ui, &s, &pu);
            }
        });
    if let Err(e) = installed {
        log::warn!("Smart playlists won't follow play counts: {}", describe(&e));
    }
}
