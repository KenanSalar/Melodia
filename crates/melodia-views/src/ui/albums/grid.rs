//! The Albums grid as a [`CardGrid`]: its keyset, its cards, and the count its header states.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc};

use super::{AlbumsUi, to_slint_album_row};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads, SortedQuery};
use crate::ui::paged_rows::unfiltered_total;
use crate::ui::track_detail::TrackDetail;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::grid_order::AlbumSort;
use melodia_core::error::AppResult;
use melodia_ui::{AlbumGridRow as UiAlbumGridRow, Albums, AppWindow};

// As visible as `GridTab`, whose `Grid` it is; the private `grid` module keeps it in the slice.
pub(in crate::ui) struct AlbumsGrid;

impl CardGrid for AlbumsGrid {
    type Row = UiAlbumGridRow;
    type Raw = AlbumStats;
    type Query = SortedQuery<AlbumSort>;
    type Facts = i32;
    type Handle = AlbumsUi;

    const LABEL: &'static str = "Albums.grid-rows";

    fn loads(handle: &AlbumsUi) -> &GridLoads {
        &handle.grid
    }

    fn model(ui: &AppWindow) -> ModelRc<UiAlbumGridRow> {
        ui.global::<Albums>().get_grid_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Albums>().get_columns()
    }

    fn query(ui: &AppWindow, _: &AlbumsUi) -> Self::Query {
        let g = ui.global::<Albums>();
        SortedQuery::new(
            AlbumSort::from_token(&g.get_sort_field()),
            &g.get_sort_dir(),
            &g.get_filter(),
        )
    }

    async fn keyset(state: AppState, query: Self::Query) -> AppResult<(Vec<i64>, i32)> {
        let ids =
            library::keyset::album_ids(&state.db, query.sort, query.dir, &query.needle).await?;
        let total =
            unfiltered_total(&ids, &query.needle, library::keyset::album_count(&state.db)).await?;
        Ok((ids, total))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<AlbumStats>> {
        library::keyset::albums_by_ids(&state.db, &ids).await
    }

    fn card(_: &AppWindow, album: &AlbumStats) -> (i64, Card<Self>) {
        (album.id, to_slint_album_row(album))
    }

    fn artwork(album: &AlbumStats) -> Option<&str> {
        album.artwork_path.as_deref()
    }

    fn show_facts(ui: &AppWindow, _: &AlbumsUi, total: i32, _: usize) {
        ui.global::<Albums>().set_total_count(total);
    }

    fn covers_on_screen(handle: &AlbumsUi) -> bool {
        handle.section.grid_on_screen(|| handle.detail_open())
    }
}

/// Reload the grid: what a launch, a re-entry and a library change ask for. UI thread.
pub fn reload_grid(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    card_grid::reload::<AlbumsGrid>(ui, state, albums_ui);
}
