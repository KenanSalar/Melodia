//! Playlists section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan / CRUD events.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, VecModel};

use crate::state::AppState;
use crate::ui::callbacks::macros::{release_detail_hero_images, spawn_logged};
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::{
    AppWindow, Nav, PlaylistDetail, PlaylistGridRow as UiPlaylistGridRow, Playlists,
    TrackListRow as UiTrackListRow,
};

/// Wire the Playlists section-lifecycle callbacks. See
/// [`super::wire_playlists`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let playlists = ui.global::<Playlists>();
    let weak = ui.as_weak();

    // section-active-changed: mirrors the Albums implementation —
    // on leave, wipe the Slint models (UI thread) then release the
    // Rust-side caches off-thread; on return, full re-fetch if dirty
    // else just prewarm visible covers.
    playlists_ui.set_section_active(ui.global::<Nav>().get_selected_index() == 7);
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
                let m = g.get_grid_rows();
                if let Some(vm) = m.as_any().downcast_ref::<VecModel<UiPlaylistGridRow>>() {
                    vm.set_vec(Vec::new());
                }

                let d = ui.global::<PlaylistDetail>();
                release_detail_hero_images!(d);
                let tm = d.get_tracks();
                if let Some(vm) = tm.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
                    vm.set_vec(Vec::new());
                }
                let sm = d.get_selected_ids();
                if let Some(vm) = sm.as_any().downcast_ref::<VecModel<i32>>() {
                    vm.set_vec(Vec::new());
                }
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
                        if let Err(e) =
                            playlists_ui_mod::fetch_grid(&s, &pu, weak.clone()).await
                        {
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
                            log::warn!(
                                "playlists::section_enter open_playlist({open_id}): {e}"
                            );
                            playlists_ui_mod::clear_detail(&pu);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                let g = ui.global::<PlaylistDetail>();
                                g.set_playlist_id(-1);
                                release_detail_hero_images!(g);
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
                // be lost. Mirrors the same gate in `wire_albums`.
                if !pu.section_active() {
                    pu.mark_dirty();
                    continue;
                }
                let open_id = pu.detail_playlist_id();
                {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "playlists::library_changed",
                        playlists_ui_mod::fetch_grid(&s, &pu, weak));
                }
                if open_id >= 0 {
                    let s = s.clone();
                    let pu = pu.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "playlists::library_changed_detail",
                        playlists_ui_mod::refresh_detail(&s, &pu, weak, open_id));
                }
            }
        }));
    }
}
