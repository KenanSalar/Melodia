//! The Most Played tab, the library's played tracks ranked by count, as a `CardGrid`. The count
//! it writes is the filtered one, gating its empty state; the band states the whole played set,
//! which each load answers beside its ids.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc};

use super::{RecentlyPlayedTab, RecentlyPlayedUi, current_filter};
use crate::ui::card_grid::{self, Card, CardGrid, GridLoads};
use crate::ui::most_played::{self, to_slint_most_played_row};
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::MostPlayedFavorite;
use melodia_core::entities::track_list::MostPlayedTotals;
use melodia_core::error::AppResult;
use melodia_core::utils::needle::Needle;
use melodia_ui::{AppWindow, EntityGridRow as UiEntityGridRow, RecentlyPlayed};

pub(super) struct RecentMostPlayed;

impl CardGrid for RecentMostPlayed {
    type Row = UiEntityGridRow;
    type Raw = MostPlayedFavorite;
    type Query = Needle;
    type Facts = MostPlayedTotals;
    type Handle = RecentlyPlayedUi;

    const LABEL: &'static str = "RecentlyPlayed.most-played-rows";

    fn loads(handle: &RecentlyPlayedUi) -> &GridLoads {
        &handle.most_played
    }

    fn model(ui: &AppWindow) -> ModelRc<UiEntityGridRow> {
        ui.global::<RecentlyPlayed>().get_most_played_rows()
    }

    fn columns(ui: &AppWindow) -> i32 {
        ui.global::<RecentlyPlayed>().get_columns()
    }

    fn query(_: &AppWindow, handle: &RecentlyPlayedUi) -> Needle {
        current_filter(handle)
    }

    async fn keyset(state: AppState, needle: Needle) -> AppResult<(Vec<i64>, MostPlayedTotals)> {
        let ids = library::keyset::most_played_ids(&state.db, &needle).await?;
        let totals = library::keyset::most_played_totals(&state.db).await?;
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

    fn show_facts(
        ui: &AppWindow,
        handle: &RecentlyPlayedUi,
        totals: MostPlayedTotals,
        shown: usize,
    ) {
        *handle.state().most_played_totals.lock() = totals;
        ui.global::<RecentlyPlayed>().set_most_played_count(len_as_i32(shown));
        crate::ui::hero_chips::publish_recently_played(ui, handle);
    }

    fn covers_on_screen(handle: &RecentlyPlayedUi) -> bool {
        handle.section_active() && handle.active_tab() == RecentlyPlayedTab::MostPlayed
    }

    fn load_failed(handle: &RecentlyPlayedUi) {
        handle.mark_grid_dirty();
    }
}

pub fn reload_grid(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    card_grid::reload::<RecentMostPlayed>(ui, state, rp_ui);
}

/// Reloads the grid's ids under a new filter.
pub fn requery_grid(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    card_grid::requery::<RecentMostPlayed>(ui, state, rp_ui);
}

pub fn set_columns(ui: &AppWindow, columns: i32) {
    card_grid::set_columns::<RecentMostPlayed>(ui, columns);
}

pub fn release_grid(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    card_grid::release::<RecentMostPlayed>(ui, rp_ui);
}

/// Warms the covers the grid last opened on, if it is the tab on screen. Blocking.
pub fn prewarm_covers(rp_ui: &RecentlyPlayedUi) {
    card_grid::prewarm_seeded::<RecentMostPlayed>(rp_ui);
}

/// The grid's track ids, in card order.
pub fn most_played_ids(ui: &AppWindow) -> Vec<i64> {
    card_grid::shown_ids::<RecentMostPlayed>(ui)
}
