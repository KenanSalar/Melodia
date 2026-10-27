//! The Genres grid as a [`CardGrid`]: its keyset, its cards, and the count its header states.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc};

use super::{GenresUi, to_slint_genre_row};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads, SortedQuery};
use crate::ui::paged_rows::unfiltered_total;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::grid_order::GenreSort;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, GenreGridRow as UiGenreGridRow, Genres};

// As visible as `GridTab`, whose `Grid` it is; the private `grid` module keeps it in the slice.
pub(in crate::ui) struct GenresGrid;

impl CardGrid for GenresGrid {
    type Row = UiGenreGridRow;
    type Raw = GenreStats;
    type Query = SortedQuery<GenreSort>;
    type Facts = i32;
    type Handle = GenresUi;

    const LABEL: &'static str = "Genres.grid-rows";

    fn loads(handle: &GenresUi) -> &GridLoads {
        &handle.grid
    }

    fn model(ui: &AppWindow) -> ModelRc<UiGenreGridRow> {
        ui.global::<Genres>().get_grid_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Genres>().get_columns()
    }

    fn query(ui: &AppWindow, _: &GenresUi) -> Self::Query {
        let g = ui.global::<Genres>();
        SortedQuery::new(
            GenreSort::from_token(&g.get_sort_field()),
            &g.get_sort_dir(),
            &g.get_filter(),
        )
    }

    async fn keyset(state: AppState, query: Self::Query) -> AppResult<(Vec<i64>, i32)> {
        let ids =
            library::keyset::genre_ids(&state.db, query.sort, query.dir, &query.needle).await?;
        let total =
            unfiltered_total(&ids, &query.needle, library::keyset::genre_count(&state.db)).await?;
        Ok((ids, total))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<GenreStats>> {
        library::keyset::genres_by_ids(&state.db, &ids).await
    }

    fn card(_: &AppWindow, genre: &GenreStats) -> (i64, Card<Self>) {
        (genre.id, to_slint_genre_row(genre))
    }

    fn artwork(_: &GenreStats) -> Option<&str> {
        None
    }

    fn show_facts(ui: &AppWindow, _: &GenresUi, total: i32, _: usize) {
        ui.global::<Genres>().set_total_count(total);
    }

    fn covers_on_screen(_: &GenresUi) -> bool {
        false
    }
}

/// Reload the grid: what a launch, a re-entry and a library change ask for. UI thread.
pub fn reload_grid(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    card_grid::reload::<GenresGrid>(ui, state, genres_ui);
}
