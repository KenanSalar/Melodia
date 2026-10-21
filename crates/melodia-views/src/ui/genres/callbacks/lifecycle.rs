//! Genres section lifecycle: the `section-active-changed` enter/leave
//! handler (release + reload) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.
//!
//! Unlike `albums/lifecycle.rs` there are no cover caches to release or
//! prewarm on enter/leave — genres are procedural-gradient tiles — so the
//! leave path releases the grid and the detail's Slint state and hands the
//! shared hero colour set back to its floor.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::card_grid;
use crate::ui::detail_view::release_shared_hero;
use crate::ui::genres::grid::GenresGrid;
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::list_selection;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::signal::on_signal;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::ui::track_detail::{self, TrackDetail, restore};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, GenreDetail, Genres};

/// Wire the Genres section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    genres_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Genres));
    wire_section_gate(ui, state, genres_ui);
    install_library_refresher(ui, state, genres_ui);
}

/// On leave: synchronously release the grid +
/// `GenreDetail.{tracks,selected-ids}` on the UI thread so the
/// `SharedString` allocations drop, then off-thread call
/// `release_section_state` (`malloc_trim`), plus the `HeroBackdrop` reset.
/// No image properties here — genres are procedural-gradient tiles, no
/// `(cover, blur)` pair. On return: [`reenter`].
fn wire_section_gate(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let gu = genres_ui.clone();
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Genres>().on_section_active_changed(move |active| {
        gu.set_section_active(active);
        if !active {
            // Land synchronously before the release task spawns, so the
            // re-enter's `take_dirty()` never sees a grid not yet released.
            gu.mark_dirty();
        }
        if !active && let Some(ui) = weak.upgrade() {
            // Rewound on the same tick as the grid it numbers;
            // `Albums.total-count`'s declaration argues the sentinel.
            ui.global::<Genres>().set_total_count(UNFETCHED_COUNT);
            card_grid::release::<GenresGrid>(&ui, &gu);

            let d = ui.global::<GenreDetail>();
            track_detail::release_rows(&*gu, &d);
            list_selection::clear_row_selection(&d);
            // Six heroes share one colour set and one chip row; this one has
            // no images to release, so it takes the shared pair alone rather
            // than the full `release_detail_hero_images`.
            release_shared_hero(&ui);
            // The card grid's set goes back with the models it describes;
            // `card-selection.slint` argues why it cannot outlive the leave.
            ui.global::<CardSelection>().invoke_clear();
        }
        if active {
            restore::resume_deferred(&weak, &s, &gu, genres_ui_mod::restore_detail);
            if let Some(ui) = weak.upgrade() {
                reenter(&ui, &s, &gu);
            }
        } else {
            let gu = gu.clone();
            s.runtime.spawn_blocking(move || gu.release_section_state());
        }
    });
}

/// The section came back on screen: a reload if a leave released the grid,
/// else nothing (initial enter after boot's load — no covers to prewarm).
/// The detail the release emptied (if `GenreDetail.genre-id >= 0`) is
/// reopened beside the reload.
fn reenter(ui: &AppWindow, s: &AppState, gu: &Arc<GenresUi>) {
    if !gu.take_dirty() {
        return;
    }
    genres_ui_mod::reload_grid(ui, s, gu);
    let open_id = gu.detail_genre_id();
    if open_id < 0 {
        return;
    }
    let s = s.clone();
    let gu = gu.clone();
    let weak = ui.as_weak();
    s.runtime.clone().spawn(async move {
        let reopened = genres_ui_mod::open_genre(
            &s,
            &gu,
            weak.clone(),
            open_id,
            melodia_ui::NavEnterFrom::Right,
        )
        .await;
        let Err(e) = reopened else { return };
        log::warn!("genres::section_enter open_genre({open_id}): {}", describe(&e));
        // Detail re-fetch failed (genre removed while
        // hidden); drop back to the grid. Mirrors the
        // Albums / Artists slices. No Image
        // properties to clear — genres are
        // procedural-gradient tiles — but the hero colour
        // set and chip row are shared, so both still have
        // to be handed back.
        gu.clear_detail();
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<GenreDetail>().set_genre_id(-1);
            release_shared_hero(&ui);
        });
    });
}

/// Watcher / scan completion / folder add+remove all bump `library_changed`.
/// Reload the grid so new genres appear and removed ones disappear;
/// refresh an open detail too.
fn install_library_refresher(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let s = state.clone();
    let gu = genres_ui.clone();
    let installed =
        on_signal(&state.library_changed, ui.as_weak(), "genres-library-changed", move |ui| {
            // Skip the in-place refresh when the section is hidden, but
            // mark the grid dirty so the next section-enter reloads.
            // Without the `mark_dirty`, a `library_changed` arriving while
            // the section was never visited (e.g. the first scan after a
            // fresh-DB launch) would be lost. Mirrors the same gate in
            // `albums/callbacks/lifecycle.rs`.
            if !gu.section_active() {
                gu.mark_dirty();
                return;
            }
            genres_ui_mod::reload_grid(ui, &s, &gu);
            let open_id = gu.detail_genre_id();
            if open_id >= 0 {
                let s = s.clone();
                let gu = gu.clone();
                let weak = ui.as_weak();
                // `refresh_detail`, not `open_genre` — a watcher
                // tick must preserve the user's sort + selection.
                spawn_logged!(
                    s,
                    "genres::library_changed_detail",
                    genres_ui_mod::refresh_detail(&s, &gu, weak, open_id)
                );
            }
        });
    if let Err(e) = installed {
        log::warn!("Genres won't follow library changes: {}", describe(&e));
    }
}
