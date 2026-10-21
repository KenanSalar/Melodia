//! The Artists grid as a [`CardGrid`]: its keyset, its cards, and the count its header states.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc};

use super::{ArtistsUi, to_slint_artist_row};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads, SortedQuery};
use crate::ui::paged_rows::unfiltered_total;
use crate::ui::track_detail::TrackDetail;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::grid_order::ArtistSort;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, ArtistGridRow as UiArtistGridRow, Artists};

pub(super) struct ArtistsGrid;

impl CardGrid for ArtistsGrid {
    type Row = UiArtistGridRow;
    type Raw = ArtistStats;
    type Query = SortedQuery<ArtistSort>;
    type Facts = i32;
    type Handle = ArtistsUi;

    const LABEL: &'static str = "Artists.grid-rows";

    fn loads(handle: &ArtistsUi) -> &GridLoads {
        &handle.grid
    }

    fn model(ui: &AppWindow) -> ModelRc<UiArtistGridRow> {
        ui.global::<Artists>().get_grid_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Artists>().get_columns()
    }

    fn query(ui: &AppWindow, _: &ArtistsUi) -> Self::Query {
        let g = ui.global::<Artists>();
        SortedQuery::new(
            ArtistSort::from_token(&g.get_sort_field()),
            &g.get_sort_dir(),
            &g.get_filter(),
        )
    }

    async fn keyset(state: AppState, query: Self::Query) -> AppResult<(Vec<i64>, i32)> {
        let ids =
            library::keyset::artist_ids(&state.db, query.sort, query.dir, &query.needle).await?;
        let total =
            unfiltered_total(&ids, &query.needle, library::keyset::artist_count(&state.db)).await?;
        Ok((ids, total))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<ArtistStats>> {
        library::keyset::artists_by_ids(&state.db, &ids).await
    }

    fn card(_: &AppWindow, artist: &ArtistStats) -> (i64, Card<Self>) {
        (artist.id, to_slint_artist_row(artist))
    }

    fn artwork(artist: &ArtistStats) -> Option<&str> {
        artist.image_path.as_deref()
    }

    fn show_facts(ui: &AppWindow, _: &ArtistsUi, total: i32, _: usize) {
        ui.global::<Artists>().set_total_count(total);
    }

    fn covers_on_screen(handle: &ArtistsUi) -> bool {
        handle.section.grid_on_screen(|| handle.detail_open())
    }
}

/// Reload the grid: what a launch, a re-entry and a library change ask for. UI thread.
pub fn reload_grid(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    card_grid::reload::<ArtistsGrid>(ui, state, artists_ui);
}
