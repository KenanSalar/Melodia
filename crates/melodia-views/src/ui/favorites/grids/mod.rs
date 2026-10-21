//! The two grid tabs — Most Played and Favorite Artists — as `CardGrid`s.
//!
//! Albums are the Albums tab's; surfacing them again here was redundant.
//!
//! Each grid keeps its keyset and the cards near its viewport across a tab pick, as Songs does,
//! and hands both back on the section leave. Only the mounted tab loads: a tick reaching the page
//! with another tab up marks the grid dirty, and the pick that mounts it reloads.
//!
//! The counts each writes are the filtered ones, gating its `GridEmptyState`; the band states the
//! whole set, which each load answers beside its ids.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc};

use super::{FavoritesTab, FavoritesUi, to_slint_fav_artist_row};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads, SortedQuery};
use crate::ui::most_played::{self, to_slint_most_played_row};
use crate::ui::paged_rows::unfiltered_total;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::artist::FavoriteArtist;
use melodia_core::entities::grid_order::FavoriteArtistSort;
use melodia_core::entities::track::MostPlayedFavorite;
use melodia_core::entities::track_list::{MostPlayedTotals, SortDir};
use melodia_core::error::AppResult;
use melodia_core::utils::needle::Needle;
use melodia_ui::{AppWindow, EntityGridRow as UiEntityGridRow, Favorites};

/// The favourites ranked by play count.
pub(super) struct FavoriteMostPlayed;

impl CardGrid for FavoriteMostPlayed {
    type Row = UiEntityGridRow;
    type Raw = MostPlayedFavorite;
    type Query = Needle;
    type Facts = MostPlayedTotals;
    type Handle = FavoritesUi;

    const LABEL: &'static str = "Favorites.most-played-rows";

    fn loads(handle: &FavoritesUi) -> &GridLoads {
        &handle.most_played
    }

    fn model(ui: &AppWindow) -> ModelRc<UiEntityGridRow> {
        ui.global::<Favorites>().get_most_played_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Favorites>().get_columns()
    }

    fn query(_: &AppWindow, handle: &FavoritesUi) -> Needle {
        super::current_filter(handle)
    }

    async fn keyset(state: AppState, needle: Needle) -> AppResult<(Vec<i64>, MostPlayedTotals)> {
        let ids = library::keyset::most_played_favorite_ids(&state.db, &needle).await?;
        let totals = library::keyset::most_played_favorite_totals(&state.db).await?;
        Ok((ids, totals))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<MostPlayedFavorite>> {
        most_played::rows(state, ids).await
    }

    fn card(_: &AppWindow, track: &MostPlayedFavorite) -> (i64, Card<Self>) {
        (track.id, to_slint_most_played_row(track))
    }

    fn artwork(track: &MostPlayedFavorite) -> Option<&str> {
        track.artwork_path.as_deref()
    }

    fn show_facts(ui: &AppWindow, handle: &FavoritesUi, totals: MostPlayedTotals, shown: usize) {
        *handle.state().most_played_totals.lock() = totals;
        ui.global::<Favorites>().set_most_played_count(len_as_i32(shown));
        crate::ui::hero_chips::publish_favorites(ui, handle);
    }

    fn covers_on_screen(handle: &FavoritesUi) -> bool {
        handle.section_active() && handle.active_tab() == FavoritesTab::MostPlayed
    }

    fn load_failed(handle: &FavoritesUi) {
        handle.mark_tab_dirty(FavoritesTab::MostPlayed);
    }
}

/// Every artist credited on a favourite.
pub(super) struct FavoriteArtistsGrid;

impl CardGrid for FavoriteArtistsGrid {
    type Row = UiEntityGridRow;
    type Raw = FavoriteArtist;
    type Query = SortedQuery<FavoriteArtistSort>;
    /// How many artists are credited on a favourite, whatever the filter leaves.
    type Facts = i32;
    type Handle = FavoritesUi;

    const LABEL: &'static str = "Favorites.artist-rows";

    fn loads(handle: &FavoritesUi) -> &GridLoads {
        &handle.artists
    }

    fn model(ui: &AppWindow) -> ModelRc<UiEntityGridRow> {
        ui.global::<Favorites>().get_artist_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<Favorites>().get_columns()
    }

    fn query(ui: &AppWindow, handle: &FavoritesUi) -> Self::Query {
        let g = ui.global::<Favorites>();
        SortedQuery {
            sort: FavoriteArtistSort::from_token(&g.get_artist_sort_field()),
            dir: SortDir::from_token(&g.get_artist_sort_dir()),
            needle: super::current_filter(handle),
        }
    }

    async fn keyset(state: AppState, query: Self::Query) -> AppResult<(Vec<i64>, i32)> {
        let ids =
            library::keyset::favorite_artist_ids(&state.db, query.sort, query.dir, &query.needle)
                .await?;
        let total = unfiltered_total(
            &ids,
            &query.needle,
            library::keyset::favorite_artist_count(&state.db),
        )
        .await?;
        Ok((ids, total))
    }

    async fn rows(state: AppState, ids: Vec<i64>) -> AppResult<Vec<FavoriteArtist>> {
        library::keyset::favorite_artists_by_ids(&state.db, &ids).await
    }

    // The subtitle is a translated plural, which only the UI thread can resolve.
    fn card(ui: &AppWindow, artist: &FavoriteArtist) -> (i64, Card<Self>) {
        let subtitle =
            ui.global::<Favorites>().invoke_artist_favorite_subtitle(artist.favorite_count);
        (artist.id, to_slint_fav_artist_row(artist, subtitle))
    }

    fn artwork(artist: &FavoriteArtist) -> Option<&str> {
        artist.image_path.as_deref()
    }

    fn show_facts(ui: &AppWindow, handle: &FavoritesUi, total: i32, shown: usize) {
        *handle.state().artist_total.lock() = total;
        ui.global::<Favorites>().set_artist_count(len_as_i32(shown));
        crate::ui::hero_chips::publish_favorites(ui, handle);
    }

    fn covers_on_screen(handle: &FavoritesUi) -> bool {
        handle.section_active() && handle.active_tab() == FavoritesTab::Artists
    }

    fn load_failed(handle: &FavoritesUi) {
        handle.mark_tab_dirty(FavoritesTab::Artists);
    }
}

/// Reload the grid `tab` mounts: what a section enter, a library or stats tick and a pick onto a
/// tab that missed one ask for. Songs has no grid. UI thread.
pub fn reload_grid(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>, tab: FavoritesTab) {
    match tab {
        FavoritesTab::MostPlayed => card_grid::reload::<FavoriteMostPlayed>(ui, state, fav_ui),
        FavoritesTab::Artists => card_grid::reload::<FavoriteArtistsGrid>(ui, state, fav_ui),
        FavoritesTab::Songs => {}
    }
}

/// Load the mounted grid's ids under a new filter or sort. UI thread.
pub fn requery_grid(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    match fav_ui.active_tab() {
        FavoritesTab::MostPlayed => card_grid::requery::<FavoriteMostPlayed>(ui, state, fav_ui),
        FavoritesTab::Artists => card_grid::requery::<FavoriteArtistsGrid>(ui, state, fav_ui),
        FavoritesTab::Songs => {}
    }
}

/// Chunk both grids at `columns` cards a row, one property serving whichever is mounted. UI
/// thread.
pub fn set_columns(ui: &AppWindow, columns: i32) {
    card_grid::set_columns::<FavoriteMostPlayed>(ui, columns);
    card_grid::set_columns::<FavoriteArtistsGrid>(ui, columns);
}

/// Empty both grids for the section leave. UI thread.
pub fn release_grids(ui: &AppWindow, fav_ui: &FavoritesUi) {
    card_grid::release::<FavoriteMostPlayed>(ui, fav_ui);
    card_grid::release::<FavoriteArtistsGrid>(ui, fav_ui);
}

/// Warm the covers the `tab` grid's last load opened on. Blocking.
pub fn prewarm_tab_covers(fav_ui: &FavoritesUi, tab: FavoritesTab) {
    match tab {
        FavoritesTab::MostPlayed => card_grid::prewarm_seeded::<FavoriteMostPlayed>(fav_ui),
        FavoritesTab::Artists => card_grid::prewarm_seeded::<FavoriteArtistsGrid>(fav_ui),
        FavoritesTab::Songs => {}
    }
}
