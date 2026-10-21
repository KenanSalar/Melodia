//! The Artists tab as [`grid_tab`] drives it through a section enter, a leave and a library
//! change.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::artists::grid::ArtistsGrid;
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::callbacks::grid_tab::{self, GridTab};
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::{AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, Artists, NavEnterFrom};

/// Wire the Artists section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    grid_tab::wire(ui, state, artists_ui);
}

impl GridTab for ArtistsUi {
    type Grid = ArtistsGrid;

    fn bind_section_active_changed(ui: &AppWindow, f: impl FnMut(bool) + 'static) {
        ui.global::<Artists>().on_section_active_changed(f);
    }

    fn rewind_count(ui: &AppWindow) {
        ui.global::<Artists>().set_total_count(UNFETCHED_COUNT);
    }

    fn release_hero(ui: &AppWindow, g: &ArtistDetail<'static>) {
        release_detail_hero_images(ui, g);
    }

    fn release_beside(&self, g: &ArtistDetail<'static>) {
        clear_vec_model::<UiAlbumRow>(&g.get_albums(), "artists: clear detail albums");
    }

    /// The detail's artwork and Albums strip and the grid tier's pixels down to proxies, then the
    /// freed pages. One tier serves the grid and the strip, so the shrink covers both.
    fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        self.detail_artwork.clear();
        self.detail.albums.lock().clear();
        melodia_platform::services::platform::allocator::trim();
    }

    async fn open(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
        enter_from: NavEnterFrom,
    ) -> AppResult<()> {
        artists_ui_mod::open_artist(state, view, weak, id, enter_from).await
    }

    async fn refresh(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
    ) -> AppResult<()> {
        artists_ui_mod::refresh_detail(state, view, weak, id).await
    }

    fn restore(ui: &AppWindow, state: &AppState, view: &Arc<Self>, id: i64) {
        artists_ui_mod::restore_detail(ui, state, view, id);
    }
}
