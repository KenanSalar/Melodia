//! Albums section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, VecModel};

use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::macros::{release_detail_hero_images, spawn_logged};
use crate::{
    AlbumDetail, AlbumGridRow as UiAlbumGridRow, Albums, AppWindow, Nav,
    TrackListRow as UiTrackListRow,
};

/// Wire the Albums section-lifecycle callbacks. See [`super::wire_albums`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let albums = ui.global::<Albums>();
    let weak = ui.as_weak();

    // section-active-changed: the Albums section entered / left the screen
    // (sidebar nav, or Now Playing opened over it). Seed the synchronous
    // shadow from the current nav state: the gate's `ChangeTracker` baselines
    // inside `AppWindow::new()` and fires only on a later difference, so a
    // section the boot doesn't land on gets no edge at all, and the one it
    // does land on gets its edge a frame late — after boot has already read
    // this shadow. See the `SectionActiveGate` bullet in
    // `.claude/rules/ui-patterns.md`.
    //
    // On leave: synchronously wipe the Slint `grid-rows` model (UI thread,
    // so its `AlbumGridRow` `SharedString`s drop), then off-thread call
    // `release_section_state` to drop the Rust-side caches + grid data +
    // detail state + `malloc_trim`. On return: full `fetch_grid` if data
    // was wiped, else just prewarm the visible covers (initial enter after
    // boot's pre-fetch). The detail re-fetch (if `AlbumDetail.album-id >=
    // 0`) runs after the grid fetch so the user lands back where they
    // were.
    albums_ui.set_section_active(ui.global::<Nav>().get_selected_index() == 4);
    {
        let au = albums_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        albums.on_section_active_changed(move |active| {
            au.set_section_active(active);
            if !active {
                // Land synchronously on the UI thread *before* the release
                // task is even spawned — the re-enter handler reads this
                // via `take_dirty()` and must never observe a still-warm
                // grid because release hadn't run yet. See
                // `AlbumsUi::data_dirty` for the race details.
                au.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // (UI thread) Slint-side drops. The Rust-side LRU clear in
                // `release_section_state` only drops the LRU's ref; the
                // detail's `slint::Image` properties hold their own refs to
                // the `SharedPixelBuffer` Arcs and the `tracks` /
                // `grid-rows` `VecModel`s carry `SharedString` allocations
                // — neither releases until we explicitly drop the
                // properties / clear the models here. `AlbumDetail.album-id`
                // is left untouched so the section-enter handler can
                // re-run `open_album` and the user lands back where they
                // were. Mirrors the per-property teardown in
                // `on_close_detail`.
                let g = ui.global::<Albums>();
                let m = g.get_grid_rows();
                if let Some(vm) = m.as_any().downcast_ref::<VecModel<UiAlbumGridRow>>() {
                    vm.set_vec(Vec::new());
                }

                let d = ui.global::<AlbumDetail>();
                release_detail_hero_images!(ui, d);
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
            let au = au.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    // `take_dirty()` returns true iff a prior leave set the
                    // flag — i.e. our cached state was wiped (or scheduled
                    // for wipe). Race-free because `mark_dirty` is set on
                    // the UI thread before the release task is spawned, so
                    // any subsequent enter sees `true`.
                    if au.take_dirty() {
                        // Snapshot the open-detail id from the Rust mirror
                        // *before* the wipe lands — `release_section_state`
                        // leaves `detail.album_id` untouched precisely so
                        // we can re-open it here.
                        let open_id = au.detail_album_id();
                        if let Err(e) =
                            albums_ui_mod::fetch_grid(&s, &au, weak.clone()).await
                        {
                            log::warn!("albums::section_enter fetch_grid: {e}");
                        }
                        // The preserved `AlbumDetail.album-id` points at a
                        // detail the wipe just emptied — re-run `open_album`
                        // so the detail view paints with real data instead
                        // of the cleared `Vec<TrackListRow>`.
                        if open_id >= 0
                            && let Err(e) = albums_ui_mod::open_album(
                                &s,
                                &au,
                                weak.clone(),
                                open_id,
                                crate::NavEnterFrom::Right,
                            )
                            .await
                        {
                            log::warn!("albums::section_enter open_album({open_id}): {e}");
                            // The detail re-fetch failed — most likely the
                            // album was deleted on disk while the section
                            // was hidden (the `library_changed` subscriber
                            // is gated on `section_active()`, so the
                            // detail's `album_id` mirror wasn't pruned).
                            // Drop the user back on the grid instead of
                            // stranding them on an empty detail page they
                            // can only escape via the back button.
                            albums_ui_mod::clear_detail(&au);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                let g = ui.global::<AlbumDetail>();
                                g.set_album_id(-1);
                                release_detail_hero_images!(ui, g);
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
                runtime.spawn_blocking(move || au.release_section_state());
            }
        });
    }

    // library_changed subscriber: watcher / scan completion / folder
    // add+remove all bump this counter. Re-fetch the grid so new albums
    // appear and removed ones disappear; refresh an open detail too.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        let mut rx = state.library_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                // Skip the in-place refresh when the section is hidden, but
                // mark the cached data dirty so the next section-enter
                // re-fetches from scratch. `release_section_state` already
                // wiped (or is about to wipe) the grid + detail caches, and
                // re-fetching now would just repopulate state the user can't
                // see. Without the `mark_dirty`, a `library_changed` arriving
                // while the section was never visited (e.g. the first scan
                // after a fresh-DB launch) would be lost — `take_dirty()` on
                // the first enter would be false and only prewarm would run.
                if !au.section_active() {
                    au.mark_dirty();
                    continue;
                }
                let open_id = au.detail_album_id();
                {
                    let s = s.clone();
                    let au = au.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "albums::library_changed",
                        albums_ui_mod::fetch_grid(&s, &au, weak));
                }
                if open_id >= 0 {
                    let s = s.clone();
                    let au = au.clone();
                    let weak = weak.clone();
                    // `refresh_detail`, not `open_album` — a watcher tick
                    // must preserve the user's sort + selection.
                    spawn_logged!(s, "albums::library_changed_detail",
                        albums_ui_mod::refresh_detail(&s, &au, weak, open_id));
                }
            }
        }));
    }
}
