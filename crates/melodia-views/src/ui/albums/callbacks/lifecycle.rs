//! The Albums tab as [`grid_tab`] drives it through a section enter, a leave and a library
//! change.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::albums::grid::AlbumsGrid;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::grid_tab::{self, GridTab};
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::{AlbumDetail, Albums, AppWindow, NavEnterFrom};

/// Wire the Albums section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    grid_tab::wire(ui, state, albums_ui);
}

impl GridTab for AlbumsUi {
    type Grid = AlbumsGrid;

    fn bind_section_active_changed(ui: &AppWindow, f: impl FnMut(bool) + 'static) {
        ui.global::<Albums>().on_section_active_changed(f);
    }

    fn rewind_count(ui: &AppWindow) {
        ui.global::<Albums>().set_total_count(UNFETCHED_COUNT);
    }

    fn release_hero(ui: &AppWindow, g: &AlbumDetail<'static>) {
        release_detail_hero_images(ui, g);
    }

    /// The detail's artwork, and the grid tier's pixels down to proxies.
    fn release_section_state(&self) {
        crate::ui::grid_prewarm::shrink_covers();
        self.detail_artwork.clear();
    }

    async fn open(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
        enter_from: NavEnterFrom,
    ) -> AppResult<()> {
        albums_ui_mod::open_album(state, view, weak, id, enter_from).await
    }

    async fn refresh(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
    ) -> AppResult<()> {
        albums_ui_mod::refresh_detail(state, view, weak, id).await
    }

    fn restore(ui: &AppWindow, state: &AppState, view: &Arc<Self>, id: i64) {
        albums_ui_mod::restore_detail(ui, state, view, id);
    }
}
