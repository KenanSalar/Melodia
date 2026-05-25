//! `Genres.*` / `GenreDetail.*` callbacks: grid, detail, sort, play,
//! favorite, selection, library-changed re-fetch.
//!
//! Mirror of `callbacks/albums.rs` minus everything cover-related:
//! genres have no intrinsic artwork (see the `Genres` global comment
//! in `ui/globals.slint`), so there's no `request-cover` handler, no
//! grid-cover release / prewarm, no `(cover, blur)` pair to clear on
//! detail close. The `section-active-changed` handler is kept for
//! symmetry — it only mirrors the active-flag today.

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, SharedString, VecModel};

use super::collect_track_ids;
use super::macros::{spawn_logged, spawn_logged_sync};
use crate::library;
use crate::state::AppState;
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{
    AppWindow, GenreDetail, GenreGridRow as UiGenreGridRow, Genres, Nav,
    TrackListRow as UiTrackListRow,
};

/// Wire every `Genres.*` / `GenreDetail.*` callback to its
/// `library::*` counterpart and the `genres_ui` shared state, plus a
/// `library_changed_tx` subscriber that re-fetches the grid (and
/// refreshes an open detail) on watcher / scan / folder events. Call
/// once after `wire_all` and after `genres::install_genres_models`.
pub fn wire_genres(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let genres = ui.global::<Genres>();
    let detail = ui.global::<GenreDetail>();
    let weak = ui.as_weak();

    // Seed the grid's sort pill from the persisted `view_sort["genres"]`.
    if let Some((field, dir)) = crate::ui::callbacks::persisted_sort(state, view_id::GENRES) {
        genres.set_sort_field(SharedString::from(field.as_str()));
        genres.set_sort_dir(SharedString::from(dir));
    }

    // section-active-changed: enter / leave the Genres section.
    //
    // On leave: synchronously wipe the Slint `grid-rows` model +
    // `GenreDetail.{tracks,selected-ids}` on the UI thread so the
    // `SharedString` allocations drop, then off-thread call
    // `release_section_state` (Rust-side grid data + detail tracks +
    // `malloc_trim`). No image properties here — genres are
    // procedural-gradient tiles, no `(cover, blur)` pair.
    //
    // On return: full `fetch_grid` if data was wiped, else no-op
    // (initial enter after boot's pre-fetch — no covers to prewarm).
    // The detail re-fetch (if `GenreDetail.genre-id >= 0`) runs after
    // the grid fetch.
    genres_ui.set_section_active(ui.global::<Nav>().get_selected_index() == 6);
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
                let m = g.get_grid_rows();
                if let Some(vm) = m.as_any().downcast_ref::<VecModel<UiGenreGridRow>>() {
                    vm.set_vec(Vec::new());
                }

                let d = ui.global::<GenreDetail>();
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
                            // procedural-gradient tiles.
                            genres_ui_mod::clear_detail(&gu);
                            let _ = weak.upgrade_in_event_loop(|ui| {
                                ui.global::<GenreDetail>().set_genre_id(-1);
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

    // --- Grid -----------------------------------------------------------

    // columns-changed: the view recomputed its integer column count and
    // already wrote `Genres.columns`. Re-chunk the cached list — no DB
    // hit.
    {
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::rebuild_grid(&ui, &gu);
        });
    }

    // apply-filter: client-side; `Genres.filter` is already updated
    // via the SearchBar's two-way binding, so just rebuild.
    {
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::rebuild_grid(&ui, &gu);
        });
    }

    // request-sort: clicking a sort pill. Same field flips dir; a new
    // field resets to ascending. Genres sort in-memory (the DB query
    // is fixed name-ASC) — no DB round-trip.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Genres>();
            let (new_field, new_dir) = if g.get_sort_field().as_str() == field.as_str() {
                let nd = if g.get_sort_dir().as_str() == "asc" { "desc" } else { "asc" };
                (field.to_string(), nd.to_string())
            } else {
                (field.to_string(), "asc".to_string())
            };
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            genres_ui_mod::rebuild_grid(&ui, &gu);
            crate::ui::callbacks::persist_view_sort(&s, view_id::GENRES, new_field, &new_dir);
        });
    }

    // open-genre: a card click. Fetches the detail header + track
    // list and flips `GenreDetail.genre-id >= 0`, swapping the grid
    // for the detail. Also stamps the Genres entry in
    // `views.json`'s `last_detail_ids` so a restart on the Genres tab
    // reopens this same detail page.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        genres.on_open_genre(move |genre_id| {
            let id = i64::from(genre_id);

            let s_fetch = s.clone();
            let gu_fetch = gu.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(s_fetch, "genres::open_genre",
                genres_ui_mod::open_genre(
                    &s_fetch, &gu_fetch, weak_fetch, id, crate::NavEnterFrom::Right));

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::GENRE_DETAIL,
                    Some(id),
                ) {
                    log::warn!("genres::open_genre persist: {e}");
                }
            });
        });
    }

    // --- Detail ---------------------------------------------------------

    // close-detail: the header's back button. Flip back to the grid
    // and drop the cached detail state. Clears the Genres entry in
    // `views.json`'s `last_detail_ids` so the next launch lands on the
    // grid (not the just-closed genre). `clear_detail` drops the
    // cached `Vec<TrackListRow>` + selection `HashSet`; the trim that
    // follows hands the now-empty arena pages back to glibc (mirrors
    // the `release_detail_artwork()` + trim that fires on Album /
    // Artist close-detail — there's no `(cover, blur)` pair to clear
    // here, but a long detail track list can still hold tens of
    // `SharedString`s per row, so the bulk-free + trim is worth doing).
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        detail.on_close_detail(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<GenreDetail>();
            // View-transition direction: `Left` = returning from a detail.
            // Set before any property write that flips the `if` branch
            // — for cross-tab origin restores that's the `selected-index`
            // write below; for same-tab back it's the `genre-id = -1`
            // write a few lines down. One up-front set covers both.
            crate::ui::nav_transition::mark_drill_back(&ui);

            // If cross-tab nav opened this detail (currently only
            // `cross_tab_nav::make_go_to_genre`), restore the
            // originating sidebar selection in the same UI-thread tick
            // as the `genre-id` reset so Slint reroutes straight to
            // the origin tab.
            let origin = g.get_origin_nav_index();
            if origin >= 0 {
                let nav = ui.global::<Nav>();
                nav.set_selected_index(origin);
                nav.invoke_persist_selected_index(origin);
                g.set_origin_nav_index(-1);
            }

            g.set_genre_id(-1);
            genres_ui_mod::clear_detail(&gu);

            let gu_trim = gu.clone();
            s.runtime.spawn_blocking(move || gu_trim.release_caches());

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::GENRE_DETAIL,
                    None,
                ) {
                    log::warn!("genres::close_detail persist: {e}");
                }
            });

            // Record the post-close state — see the matching call in
            // `albums/detail.rs::on_close_detail` for the rationale.
            crate::ui::nav_history::record_current(&s, &ui);
        });
    }

    // play-genre / shuffle-genre: play every track in display order
    // from the top. Shuffle plays the genre then turns shuffle on.
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        detail.on_play_genre(move || {
            let ids = gu.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "genres::play_genre",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)));
        });
    }

    {
        let s = state.clone();
        let gu = genres_ui.clone();
        detail.on_shuffle_genre(move || {
            let ids = gu.detail_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)).await
                {
                    log::warn!("genres::shuffle_genre play: {e}");
                    return;
                }
                if let Err(e) = library::queue::queue_set_shuffle(&s, true) {
                    log::warn!("genres::shuffle_genre set_shuffle: {e}");
                }
            });
        });
    }

    // play-row: double-click appends only that track to the queue
    // (skipping duplicates). Use `play-genre` to load every *visible*
    // track — when a search filter is active that is the filtered subset,
    // not the whole genre.
    {
        let s = state.clone();
        detail.on_play_row(move |track_id, _idx| {
            let s = s.clone();
            let id = i64::from(track_id);
            spawn_logged!(s, "genres::play_row",
                library::queue::queue_append_unique(&s, id));
        });
    }

    {
        let s = state.clone();
        detail.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "genres::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        detail.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "genres::add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite: write through, then surgically update each
    // affected row (no list re-fetch — scroll position holds and
    // there's no flash). Single-row and multi-select both arrive as
    // `[int]`.
    {
        let s = state.clone();
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let weak = weak.clone();
            let gu = gu.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s, id_vec.clone(), fav).await {
                    log::warn!("genres::set_favorite: {e}");
                    return;
                }
                for id in &id_vec {
                    gu.flip_detail_favorite(*id, fav);
                    genres_ui_mod::apply_detail_row_favorite(&weak, *id, fav);
                }
            });
        });
    }

    // select-row / clear-selection: modifier-aware selection,
    // mirroring the Tracks / Albums views. The new selected set is
    // computed in Rust.
    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::handle_select_row(&ui, &gu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::clear_selection(&ui, &gu);
        });
    }

    // request-sort: clicking a track-table header. Same field flips
    // dir; a new field resets to ascending. Genre detail sorts
    // in-memory; the new sort is persisted (shared by every genre).
    {
        let s = state.clone();
        let gu = genres_ui.clone();
        let weak = weak.clone();
        detail.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<GenreDetail>();
            let (new_field, new_dir) = if g.get_sort_field().as_str() == field.as_str() {
                let nd = if g.get_sort_dir().as_str() == "asc" { "desc" } else { "asc" };
                (field.to_string(), nd.to_string())
            } else {
                (field.to_string(), "asc".to_string())
            };
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            genres_ui_mod::resort_detail(&ui, &gu);
            crate::ui::callbacks::persist_view_sort(
                &s,
                view_id::GENRE_DETAIL,
                new_field,
                &new_dir,
            );
        });
    }

    // toggle-column: the popup already flipped the matching `show-*`
    // flag for instant feedback. Persist the new visible-column list
    // under the `"genre_detail"` settings key.
    {
        let s = state.clone();
        let weak = weak.clone();
        detail.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<GenreDetail>().snapshot_visible();
            let s = s.clone();
            spawn_logged_sync!(s, "genres::toggle_column",
                library::settings::update_view_columns(&s, "genre_detail".to_string(), columns));
        });
    }

    // filter-changed: re-walk the cached tracks through the new needle
    // and push a filtered Slint model. In-memory walk, no DB round-trip.
    // Mirrors `ArtistDetail.on_filter_changed`.
    {
        let weak = weak.clone();
        let gu = genres_ui.clone();
        detail.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            genres_ui_mod::set_filter(&gu, text.as_str());
            genres_ui_mod::apply_filtered_detail(&ui, &gu);
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
