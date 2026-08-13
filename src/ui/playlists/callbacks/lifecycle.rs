//! Playlists section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan / CRUD events.

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::callbacks::macros::{release_detail_hero_images, spawn_logged};
use crate::ui::model_diff::clear_vec_model;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::{
    AppWindow, PlaylistDetail, PlaylistGridRow as UiPlaylistGridRow, Playlists,
    TrackListRow as UiTrackListRow,
};

/// Wire the Playlists section-lifecycle callbacks. See
/// [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // section-active-changed: mirrors the Albums implementation —
    // on leave, wipe the Slint models (UI thread) then release the
    // Rust-side caches off-thread; on return, full re-fetch if dirty
    // else just prewarm visible covers.
    playlists_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Playlists));
    // See the matching seed in `albums/lifecycle.rs`: a boot pre-fetch for a
    // section that isn't on screen can't publish the shared hero globals, so
    // its first enter has to re-fetch rather than take the cheap path.
    if !playlists_ui.section_active() {
        playlists_ui.mark_dirty();
    }
    {
        let pu = playlists_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        playlists.on_section_active_changed(move |active| {
            pu.set_section_active(active);
            if !active {
                pu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                let g = ui.global::<Playlists>();
                // Rewound on the same tick as the model it numbers;
                // `Albums.total-count`'s declaration argues the sentinel.
                g.set_total_count(UNFETCHED_COUNT);
                clear_vec_model::<UiPlaylistGridRow>(&g.get_grid_rows(), "playlists: clear grid");

                let d = ui.global::<PlaylistDetail>();
                release_detail_hero_images!(ui, d);
                clear_vec_model::<UiTrackListRow>(
                    &d.get_tracks(),
                    "playlists: clear detail tracks",
                );
                clear_vec_model::<i32>(&d.get_selected_ids(), "playlists: clear detail selection");
                d.set_selection_anchor(-1);
            }
            let pu = pu.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if pu.take_dirty() {
                        let open_id = pu.detail_playlist_id();
                        if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await {
                            log::warn!("playlists::section_enter fetch_grid: {e}");
                        }
                        if open_id >= 0
                            && let Err(e) = playlists_ui_mod::open_playlist(
                                &s,
                                &pu,
                                weak.clone(),
                                open_id,
                                crate::NavEnterFrom::Right,
                            )
                            .await
                        {
                            log::warn!("playlists::section_enter open_playlist({open_id}): {e}");
                            playlists_ui_mod::clear_detail(&pu);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                let g = ui.global::<PlaylistDetail>();
                                g.set_playlist_id(-1);
                                release_detail_hero_images!(ui, g);
                            });
                        }
                    } else {
                        let pu = pu.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            pu.prewarm_visible_covers();
                        })
                        .await;
                    }
                });
            } else {
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || pu.release_section_state());
            }
        });
    }

    // library_changed subscriber — playlist mutations from elsewhere
    // (CRUD, scans, watcher) bump this counter. Mirror the Albums
    // pattern: re-fetch the grid (so new playlists appear and removed
    // ones disappear) and refresh an open detail.
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        let mut rx = state.library_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                // Skip the in-place refresh when the section is hidden, but
                // mark the cached data dirty so the next section-enter
                // re-fetches from scratch. Without the `mark_dirty`, a
                // `library_changed` arriving while the section was never
                // visited (e.g. the first scan after a fresh-DB launch) would
                // be lost. Mirrors the same gate in `albums/callbacks/lifecycle.rs`.
                if !pu.section_active() {
                    pu.mark_dirty();
                    continue;
                }
                let open_id = pu.detail_playlist_id();
                {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(
                        s,
                        "playlists::library_changed",
                        playlists_ui_mod::fetch_grid(&s, &pu, weak)
                    );
                }
                if open_id >= 0 {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(
                        s,
                        "playlists::library_changed_detail",
                        playlists_ui_mod::refresh_detail(&s, &pu, weak, open_id)
                    );
                }
            }
        }));
    }

    // stats_changed subscriber — play-count / last-played flushes bump this
    // channel (NOT library_changed, by design, to avoid churning every view on
    // each played song). Only smart playlists whose rules depend on play stats
    // ("most played", "recently played", "never played") can change, so this is
    // gated on the presence of a *stat-dependent* smart playlist and the refresh
    // recounts only that subset — a static-rule ("Genre is Rock") smart playlist
    // is neither woken nor recounted here (it can't have moved).
    {
        let s = state.clone();
        let pu = playlists_ui.clone();
        let weak = weak.clone();
        let mut rx = state.stats_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                // Hidden section: the library_changed / section-enter path
                // already re-fetches on return, so nothing to do here. And
                // nothing to do unless a smart playlist could actually have
                // moved (stricter than a plain any-smart-playlist check).
                if !pu.section_active() || !pu.has_stat_dependent_smart_playlists() {
                    continue;
                }
                let open_id = pu.detail_playlist_id();
                let open_smart = open_id >= 0 && pu.is_playlist_smart_stat_dependent(open_id);
                {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(
                        s,
                        "playlists::stats_changed",
                        playlists_ui_mod::fetch_grid_stats(&s, &pu, weak)
                    );
                }
                if open_smart {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(
                        s,
                        "playlists::stats_changed_detail",
                        playlists_ui_mod::refresh_detail(&s, &pu, weak, open_id)
                    );
                }
            }
        }));
    }
}
