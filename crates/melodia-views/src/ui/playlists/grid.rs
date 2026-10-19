//! The Playlists grid as a [`CardGrid`], and what every playlist write refreshes.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, Weak};

use super::{PlaylistsUi, to_slint_playlist_row};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads, unfiltered_total};
use crate::ui::paged_rows::grid::PagedGrid;
use crate::ui::track_detail::TrackDetail;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::track_list::SortDir;
use melodia_core::error::{AppResult, describe};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{
    AppWindow, PlaylistGridRow as UiPlaylistGridRow, PlaylistRow as UiPlaylistRow, Playlists,
};

pub(super) struct PlaylistsGrid;

impl CardGrid for PlaylistsGrid {
    type Row = UiPlaylistGridRow;
    type Raw = PlaylistStats;
    /// The direction and the needle: the grid has one order, by when each playlist last changed.
    type Query = (SortDir, Needle);
    type Facts = i32;
    type Handle = PlaylistsUi;

    const LABEL: &'static str = "Playlists.grid-rows";

    fn loads(handle: &PlaylistsUi) -> &GridLoads {
        &handle.grid
    }

    fn model(ui: &AppWindow) -> ModelRc<UiPlaylistGridRow> {
        ui.global::<Playlists>().get_grid_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Playlists>().get_columns()
    }

    fn query(ui: &AppWindow, _: &PlaylistsUi) -> Self::Query {
        let g = ui.global::<Playlists>();
        (SortDir::from_token(&g.get_sort_dir()), fold_needle(&g.get_filter()))
    }

    async fn keyset(state: AppState, (dir, needle): Self::Query) -> AppResult<(Vec<i64>, i32)> {
        let ids = library::keyset::playlist_ids(&state.db, dir, &needle).await?;
        let total =
            unfiltered_total(&ids, &needle, library::keyset::playlist_count(&state.db)).await?;
        Ok((ids, total))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<PlaylistStats>> {
        library::keyset::playlists_by_ids(&state.db, &ids).await
    }

    fn card(_: &AppWindow, playlist: &PlaylistStats) -> (i64, Card<Self>) {
        (playlist.id, to_slint_playlist_row(playlist))
    }

    fn artwork(playlist: &PlaylistStats) -> Option<&str> {
        playlist.thumbnail_path.as_deref()
    }

    // The unfiltered total is the count the row menus read too, free here.
    fn show_facts(ui: &AppWindow, _: &PlaylistsUi, total: i32, _: usize) {
        let g = ui.global::<Playlists>();
        g.set_total_count(total);
        g.set_count(total);
    }

    fn covers_on_screen(handle: &PlaylistsUi) -> bool {
        handle.section.grid_on_screen(|| handle.detail_open())
    }
}

/// Reload the grid: what a launch, a re-entry and a library change ask for. UI thread.
pub fn reload_grid(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    card_grid::reload::<PlaylistsGrid>(ui, state, playlists_ui);
}

/// Refresh what a playlist write moves: the count every row menu's Add to Playlist reads, whatever
/// tab is up, and the grid where it is on screen. Every write, the smart ones included, comes
/// through here. Any thread.
pub fn refresh_after_write(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: &Weak<AppWindow>,
) {
    refresh_count(state, weak);
    if !playlists_ui.section_active() {
        playlists_ui.mark_dirty();
        return;
    }
    let s = state.clone();
    let pu = Arc::clone(playlists_ui);
    let _ = weak.upgrade_in_event_loop(move |ui| reload_grid(&ui, &s, &pu));
}

/// Write how many playlists there are into `Playlists.count`. Any thread.
pub fn refresh_count(state: &AppState, weak: &Weak<AppWindow>) {
    let db = state.db.clone();
    let weak = weak.clone();
    state.runtime.spawn(async move {
        match library::keyset::playlist_count(&db).await {
            Ok(count) => {
                let count = clamp_i64_to_i32(count);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<Playlists>().set_count(count);
                });
            }
            Err(e) => log::warn!("playlists: count failed: {}", describe(&e)),
        }
    });
}

/// The card on screen for playlist `id`, for a callback acting on one. None where it hasn't
/// landed yet.
pub(in crate::ui) fn shown_card(ui: &AppWindow, id: i64) -> Option<UiPlaylistRow> {
    let model = PlaylistsGrid::model(ui);
    PagedGrid::installed(&model, PlaylistsGrid::LABEL)?.cached(id)
}

/// The play-count flush's refresh: only a smart playlist whose rules read play stats can have
/// moved, so its card alone is read again, and the open detail where it is one. With none held,
/// nothing is. UI thread.
pub(in crate::ui) fn refresh_play_stats(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
) {
    let model = PlaylistsGrid::model(ui);
    let Some(grid) = PagedGrid::installed(&model, PlaylistsGrid::LABEL) else {
        return;
    };
    let open_id = playlists_ui.detail_playlist_id();
    let mut held = grid.ids().to_vec();
    if open_id >= 0 {
        held.push(open_id);
    }
    let s = state.clone();
    let pu = Arc::clone(playlists_ui);
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        let moved = match library::smart_playlists::reading_play_stats(&s.db, &held).await {
            Ok(moved) => moved,
            Err(e) => {
                log::warn!("playlists::stats_changed: {}", describe(&e));
                return;
            }
        };
        if moved.is_empty() {
            return;
        }
        if moved.contains(&open_id)
            && let Err(e) = super::refresh_detail(&s, &pu, weak.clone(), open_id).await
        {
            log::warn!("playlists::stats_changed_detail: {}", describe(&e));
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            card_grid::refresh_cards::<PlaylistsGrid>(&ui, &s, &pu, moved);
        });
    });
}
