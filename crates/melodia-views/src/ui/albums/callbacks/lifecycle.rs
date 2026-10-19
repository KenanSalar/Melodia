//! Albums section lifecycle: the `section-active-changed` enter/leave
//! handler (release + reload) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::albums::grid::AlbumsGrid;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::card_grid;
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::grid_prewarm;
use crate::ui::list_selection;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::signal::on_signal;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::ui::track_detail::{self, TrackDetail, restore};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AlbumDetail, Albums, AppWindow, CardSelection};

/// Wire the Albums section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    // section-active-changed: the Albums section entered / left the screen
    // (sidebar nav, or Now Playing opened over it). Seed the synchronous
    // shadow from the current nav state: the gate's `ChangeTracker` baselines
    // inside `AppWindow::new()` and fires only on a later difference, so a
    // section the boot doesn't land on gets no edge at all, and the one it
    // does land on gets its edge a frame late — after boot has already read
    // this shadow. See the `SectionActiveGate` bullet in
    // `.claude/rules/ui-patterns.md`.
    albums_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Albums));
    wire_section_gate(ui, state, albums_ui);
    install_library_refresher(ui, state, albums_ui);
}

/// On leave: synchronously release the grid and the detail's Slint state (UI
/// thread, so their `SharedString`s drop), then off-thread call
/// `release_section_state` for the covers, the detail artwork and
/// `malloc_trim`. On return: [`reenter`].
fn wire_section_gate(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let au = albums_ui.clone();
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Albums>().on_section_active_changed(move |active| {
        au.set_section_active(active);
        if !active {
            // Land synchronously on the UI thread *before* the release
            // task is even spawned — the re-enter handler reads this
            // via `take_dirty()` and must never observe a still-warm
            // grid because release hadn't run yet.
            au.mark_dirty();
        }
        if !active && let Some(ui) = weak.upgrade() {
            // (UI thread) Slint-side drops. The detail's `slint::Image`
            // properties hold their own refs to the `SharedPixelBuffer` Arcs,
            // and the grid and the detail's tracks carry `SharedString`s, so
            // neither releases until they are explicitly handed back here.
            // `AlbumDetail.album-id` is left untouched so the section-enter
            // handler can re-run `open_album` and the user lands back where
            // they were. Mirrors the per-property teardown in `on_close_detail`.
            // Rewound on the same tick as the grid it numbers;
            // `Albums.total-count`'s declaration argues the sentinel.
            ui.global::<Albums>().set_total_count(UNFETCHED_COUNT);
            card_grid::release::<AlbumsGrid>(&ui, &au);

            let d = ui.global::<AlbumDetail>();
            release_detail_hero_images(&ui, &d);
            track_detail::release_rows(&*au, &d);
            list_selection::clear_row_selection(&d);
            // The card grid's set goes back with the models it describes;
            // `card-selection.slint` argues why it cannot outlive the leave.
            ui.global::<CardSelection>().invoke_clear();
        }
        if active {
            restore::resume_deferred(&weak, &s, &au, albums_ui_mod::restore_detail);
            if let Some(ui) = weak.upgrade() {
                reenter(&ui, &s, &au);
            }
        } else {
            let au = au.clone();
            s.runtime.spawn_blocking(move || au.release_section_state());
        }
    });
}

/// The section came back on screen: a reload if a leave released the grid,
/// else just a prewarm of its covers (the initial enter after boot's load).
/// The detail the release emptied is reopened beside the reload, so the user
/// lands back where they were.
fn reenter(ui: &AppWindow, s: &AppState, au: &Arc<AlbumsUi>) {
    // `take_dirty()` returns true iff a prior leave set the flag, which it
    // does on the UI thread before the release task is spawned, so any later
    // enter sees it.
    if !au.take_dirty() {
        let au = au.clone();
        s.runtime.spawn_blocking(move || au.prewarm_visible_covers());
        return;
    }
    albums_ui_mod::reload_grid(ui, s, au);
    // `release_section_state` leaves the detail's id alone precisely so it
    // can be reopened here.
    let open_id = au.detail_album_id();
    if open_id < 0 {
        return;
    }
    let s = s.clone();
    let au = au.clone();
    let weak = ui.as_weak();
    s.runtime.clone().spawn(async move {
        let reopened = albums_ui_mod::open_album(
            &s,
            &au,
            weak.clone(),
            open_id,
            melodia_ui::NavEnterFrom::Right,
        )
        .await;
        let Err(e) = reopened else { return };
        log::warn!("albums::section_enter open_album({open_id}): {}", describe(&e));
        // The detail re-fetch failed — most likely the album was deleted on
        // disk while the section was hidden (the `library_changed` subscriber
        // is gated on `section_active()`, so the detail's `album_id` mirror
        // wasn't pruned). Drop the user back on the grid instead of stranding
        // them on an empty detail page they can only escape via the back button.
        au.clear_detail();
        // Handed back warm, as a failed restore's grid is.
        grid_prewarm::prewarm_off_thread(&au, AlbumsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            let g = ui.global::<AlbumDetail>();
            g.set_album_id(-1);
            release_detail_hero_images(&ui, &g);
        });
    });
}

/// Watcher / scan completion / folder add+remove all bump `library_changed`.
/// Reload the grid so new albums appear and removed ones disappear; refresh
/// an open detail too.
fn install_library_refresher(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let s = state.clone();
    let au = albums_ui.clone();
    let installed =
        on_signal(&state.library_changed, ui.as_weak(), "albums-library-changed", move |ui| {
            // Skip the in-place refresh when the section is hidden, but
            // mark the grid dirty so the next section-enter reloads. The
            // leave already released (or is about to release) the grid and
            // the detail, and reloading now would repopulate state the user
            // can't see. Without the `mark_dirty`, a `library_changed`
            // arriving while the section was never visited (e.g. the first
            // scan after a fresh-DB launch) would be lost — `take_dirty()` on
            // the first enter would be false and only the prewarm would run.
            if !au.section_active() {
                au.mark_dirty();
                return;
            }
            albums_ui_mod::reload_grid(ui, &s, &au);
            let open_id = au.detail_album_id();
            if open_id >= 0 {
                let s = s.clone();
                let au = au.clone();
                let weak = ui.as_weak();
                // `refresh_detail`, not `open_album` — a watcher tick
                // must preserve the user's sort + selection.
                spawn_logged!(
                    s,
                    "albums::library_changed_detail",
                    albums_ui_mod::refresh_detail(&s, &au, weak, open_id)
                );
            }
        });
    if let Err(e) = installed {
        log::warn!("Albums won't follow library changes: {}", describe(&e));
    }
}
