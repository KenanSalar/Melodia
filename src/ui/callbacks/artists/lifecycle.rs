//! Artists section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, VecModel};

use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::callbacks::macros::{release_detail_hero_images, spawn_logged};
use crate::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, ArtistGridRow as UiArtistGridRow, Artists,
    Nav, TrackListRow as UiTrackListRow,
};

use super::NAV_ARTISTS;

/// Wire the Artists section-lifecycle callbacks. See [`super::wire_artists`].
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    albums_ui: &Arc<AlbumsUi>,
) {
    let artists = ui.global::<Artists>();
    let weak = ui.as_weak();

    // section-active-changed: enter / leave the Artists section.
    //
    // On leave: synchronously wipe the Slint `grid-rows` model + every
    // `ArtistDetail.*` property holding heavy refs (cover/blur Images,
    // tracks + albums + selected-ids VecModels) on the UI thread so the
    // `SharedPixelBuffer` Arcs + `SharedString` allocations drop. Then
    // off-thread call `release_section_state` (Rust-side caches + grid
    // data + detail tracks + `malloc_trim`) plus `albums.release_grid_covers`
    // — Artist Detail's Albums sub-section borrows `AlbumsUi.grid_covers`,
    // so its lingering thumbnails are freed too.
    //
    // On return: full `fetch_grid` if data was wiped, else just prewarm
    // (initial enter after boot's pre-fetch). The detail re-fetch (if
    // `ArtistDetail.artist-id >= 0`) runs after the grid fetch so the
    // user lands back where they were.
    artists_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_ARTISTS);
    {
        let au = artists_ui.clone();
        let albums = albums_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        artists.on_section_active_changed(move |active| {
            au.set_section_active(active);
            if !active {
                // Land synchronously before the release task spawns — see
                // `ArtistsUi::data_dirty` for the race details.
                au.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                let g = ui.global::<Artists>();
                let m = g.get_grid_rows();
                if let Some(vm) = m.as_any().downcast_ref::<VecModel<UiArtistGridRow>>() {
                    vm.set_vec(Vec::new());
                }

                let d = ui.global::<ArtistDetail>();
                release_detail_hero_images!(d);
                let tm = d.get_tracks();
                if let Some(vm) = tm.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
                    vm.set_vec(Vec::new());
                }
                let am = d.get_albums();
                if let Some(vm) = am.as_any().downcast_ref::<VecModel<UiAlbumRow>>() {
                    vm.set_vec(Vec::new());
                }
                let sm = d.get_selected_ids();
                if let Some(vm) = sm.as_any().downcast_ref::<VecModel<i32>>() {
                    vm.set_vec(Vec::new());
                }
                d.set_selection_anchor(-1);
            }
            let au = au.clone();
            let albums = albums.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if au.take_dirty() {
                        let open_id = au.detail_artist_id();
                        if let Err(e) =
                            artists_ui_mod::fetch_grid(&s, &au, weak.clone()).await
                        {
                            log::warn!("artists::section_enter fetch_grid: {e}");
                        }
                        if open_id >= 0
                            && let Err(e) = artists_ui_mod::open_artist(
                                &s,
                                &au,
                                weak.clone(),
                                open_id,
                                crate::NavEnterFrom::Right,
                            )
                            .await
                        {
                            log::warn!("artists::section_enter open_artist({open_id}): {e}");
                            // Detail re-fetch failed (artist deleted while
                            // hidden); drop back to the grid rather than
                            // stranding the user on an empty detail page.
                            // Mirrors `wire_albums::on_section_active_changed`.
                            artists_ui_mod::clear_detail(&au);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                let g = ui.global::<ArtistDetail>();
                                g.set_artist_id(-1);
                                release_detail_hero_images!(g);
                            });
                        }
                    } else {
                        let au = au.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            au.prewarm_visible_covers();
                        })
                        .await;
                    }
                });
            } else {
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || {
                    au.release_section_state();
                    albums.release_grid_covers();
                });
            }
        });
    }

    // library_changed subscriber: re-fetch the grid + refresh an open
    // detail. Preserves sort + selection in the detail (uses
    // `refresh_detail`, not `open_artist`).
    {
        let s = state.clone();
        let au = artists_ui.clone();
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
                if !au.section_active() {
                    au.mark_dirty();
                    continue;
                }
                let open_id = au.detail_artist_id();
                {
                    let s = s.clone();
                    let au = au.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "artists::library_changed",
                        artists_ui_mod::fetch_grid(&s, &au, weak));
                }
                if open_id >= 0 {
                    let s = s.clone();
                    let au = au.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "artists::library_changed_detail",
                        artists_ui_mod::refresh_detail(&s, &au, weak, open_id));
                }
            }
        }));
    }
}
