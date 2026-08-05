//! Genres section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.
//!
//! Unlike `albums/lifecycle.rs` there are no cover caches to release or
//! prewarm on enter/leave — genres are procedural-gradient tiles — so the
//! leave path wipes the Slint models + Rust-side detail state and hands the
//! shared hero colour set back to its floor.

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::model_diff::clear_vec_model;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::{
    AppWindow, GenreDetail, GenreGridRow as UiGenreGridRow, Genres, TrackListRow as UiTrackListRow,
};

/// Wire the Genres section-lifecycle callbacks. See [`super::wire_genres`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let genres = ui.global::<Genres>();
    let weak = ui.as_weak();

    // section-active-changed: enter / leave the Genres section.
    //
    // On leave: synchronously wipe the Slint `grid-rows` model +
    // `GenreDetail.{tracks,selected-ids}` on the UI thread so the
    // `SharedString` allocations drop, then off-thread call
    // `release_section_state` (Rust-side grid data + detail tracks +
    // `malloc_trim`), plus the `HeroBackdrop` reset. No image properties here
    // — genres are procedural-gradient tiles, no `(cover, blur)` pair.
    //
    // On return: full `fetch_grid` if data was wiped, else no-op
    // (initial enter after boot's pre-fetch — no covers to prewarm).
    // The detail re-fetch (if `GenreDetail.genre-id >= 0`) runs after
    // the grid fetch.
    genres_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Genres));
    // See the matching seed in `albums/lifecycle.rs`: a boot pre-fetch for a
    // section that isn't on screen can't publish the shared hero globals, so
    // its first enter has to re-fetch rather than take the cheap path.
    if !genres_ui.section_active() {
        genres_ui.mark_dirty();
    }
    {
        let gu = genres_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        genres.on_section_active_changed(move |active| {
            gu.set_section_active(active);
            if !active {
                // Land synchronously before the release task spawns — see
                // `GenresUi::data_dirty` for the race details.
                gu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                let g = ui.global::<Genres>();
                // Rewound on the same tick as the model it numbers;
                // `Albums.total-count`'s declaration argues the sentinel.
                g.set_total_count(UNFETCHED_COUNT);
                clear_vec_model::<UiGenreGridRow>(&g.get_grid_rows(), "genres: clear grid");

                let d = ui.global::<GenreDetail>();
                clear_vec_model::<UiTrackListRow>(&d.get_tracks(), "genres: clear detail tracks");
                clear_vec_model::<i32>(&d.get_selected_ids(), "genres: clear detail selection");
                d.set_selection_anchor(-1);
                // Six heroes share one colour set and one chip row, and this
                // one has no images to release — so what rides in
                // `release_detail_hero_images!` elsewhere is explicit here.
                crate::ui::hero_backdrop::reset(&ui);
                crate::ui::hero_chips::clear(&ui);
            }
            let gu = gu.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if gu.take_dirty() {
                        let open_id = gu.detail_genre_id();
                        if let Err(e) =
                            genres_ui_mod::fetch_grid(&s, &gu, weak.clone()).await
                        {
                            log::warn!("genres::section_enter fetch_grid: {e}");
                        }
                        if open_id >= 0
                            && let Err(e) = genres_ui_mod::open_genre(
                                &s,
                                &gu,
                                weak.clone(),
                                open_id,
                                crate::NavEnterFrom::Right,
                            )
                            .await
                        {
                            log::warn!("genres::section_enter open_genre({open_id}): {e}");
                            // Detail re-fetch failed (genre removed while
                            // hidden); drop back to the grid. Mirrors
                            // `wire_albums` / `wire_artists`. No Image
                            // properties to clear — genres are
                            // procedural-gradient tiles — but the hero colour
                            // set and chip row are shared, so both still have
                            // to be handed back.
                            genres_ui_mod::clear_detail(&gu);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                ui.global::<GenreDetail>().set_genre_id(-1);
                                crate::ui::hero_backdrop::reset(&ui);
                                crate::ui::hero_chips::clear(&ui);
                            });
                        }
                    }
                    // No prewarm branch — genres have no covers to warm.
                });
            } else {
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || gu.release_section_state());
            }
        });
    }

    // library_changed subscriber: watcher / scan completion / folder
    // add+remove all bump this counter. Re-fetch the grid so new
    // genres appear and removed ones disappear; refresh an open
    // detail too.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
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
                if !gu.section_active() {
                    gu.mark_dirty();
                    continue;
                }
                let open_id = gu.detail_genre_id();
                {
                    let s = s.clone();
                    let gu = gu.clone();
                    let weak = weak.clone();
                    spawn_logged!(s, "genres::library_changed",
                        genres_ui_mod::fetch_grid(&s, &gu, weak));
                }
                if open_id >= 0 {
                    let s = s.clone();
                    let gu = gu.clone();
                    let weak = weak.clone();
                    // `refresh_detail`, not `open_genre` — a watcher
                    // tick must preserve the user's sort + selection.
                    spawn_logged!(s, "genres::library_changed_detail",
                        genres_ui_mod::refresh_detail(&s, &gu, weak, open_id));
                }
            }
        }));
    }
}
