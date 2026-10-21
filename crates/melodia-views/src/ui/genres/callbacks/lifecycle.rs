//! The Genres tab as [`grid_tab`] drives it through a section enter, a leave and a library
//! change.
//!
//! Genres are procedural-gradient tiles with no cover tier, so a leave hands back no images and
//! the detail's hero gives back only the colour set and chip row every hero shares.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::callbacks::grid_tab::{self, GridTab};
use crate::ui::detail_view::release_shared_hero;
use crate::ui::genres::grid::GenresGrid;
use crate::ui::genres::{self as genres_ui_mod, GenresUi};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, GenreDetail, Genres, NavEnterFrom};

/// Wire the Genres section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    grid_tab::wire(ui, state, genres_ui);
}

impl GridTab for GenresUi {
    type Grid = GenresGrid;

    fn bind_section_active_changed(ui: &AppWindow, f: impl FnMut(bool) + 'static) {
        ui.global::<Genres>().on_section_active_changed(f);
    }

    fn rewind_count(ui: &AppWindow) {
        ui.global::<Genres>().set_total_count(UNFETCHED_COUNT);
    }

    fn release_hero(ui: &AppWindow, _: &GenreDetail<'static>) {
        release_shared_hero(ui);
    }

    /// Only the freed pages: no LRU lives here to drop first.
    fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        melodia_platform::services::platform::allocator::trim();
    }

    async fn open(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
        enter_from: NavEnterFrom,
    ) -> AppResult<()> {
        genres_ui_mod::open_genre(state, view, weak, id, enter_from).await
    }

    async fn refresh(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
    ) -> AppResult<()> {
        genres_ui_mod::refresh_detail(state, view, weak, id).await
    }

    fn restore(ui: &AppWindow, state: &AppState, view: &Arc<Self>, id: i64) {
        genres_ui_mod::restore_detail(ui, state, view, id);
    }
}
