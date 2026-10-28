//! The Playlists tab as [`grid_tab`] drives it through a section enter, a leave and a library
//! change, plus the `stats_changed` subscriber only this tab has.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::callbacks::grid_tab::{self, GridTab};
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::playlists::grid::PlaylistsGrid;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::signal::on_signal;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, NavEnterFrom, PlaylistDetail, Playlists};

pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    grid_tab::wire(ui, state, playlists_ui);
    install_stats_refresher(ui, state, playlists_ui);
}

impl GridTab for PlaylistsUi {
    type Grid = PlaylistsGrid;

    fn bind_section_active_changed(ui: &AppWindow, f: impl FnMut(bool) + 'static) {
        ui.global::<Playlists>().on_section_active_changed(f);
    }

    fn rewind_count(ui: &AppWindow) {
        ui.global::<Playlists>().set_total_count(UNFETCHED_COUNT);
    }

    fn release_hero(ui: &AppWindow, g: &PlaylistDetail<'static>) {
        release_detail_hero_images(ui, g);
    }

    /// The grid tier's pixels down to proxies, and the detail's artwork.
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
        playlists_ui_mod::open_playlist(state, view, weak, id, enter_from).await
    }

    async fn refresh(
        state: &AppState,
        view: &Arc<Self>,
        weak: Weak<AppWindow>,
        id: i64,
    ) -> AppResult<()> {
        playlists_ui_mod::refresh_detail(state, view, weak, id).await
    }

    fn restore(ui: &AppWindow, state: &AppState, view: &Arc<Self>, id: i64) {
        playlists_ui_mod::restore_detail(ui, state, view, id);
    }
}

/// Follows `stats_changed`, which a play-count flush bumps instead of `library_changed`, for the
/// smart playlists whose rules read play stats.
fn install_stats_refresher(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let installed =
        on_signal(&state.stats_changed, ui.as_weak(), "playlists-stats-changed", move |ui| {
            // A hidden section reloads on its enter.
            if pu.section_active() {
                playlists_ui_mod::refresh_play_stats(ui, &s, &pu);
            }
        });
    if let Err(e) = installed {
        log::warn!("Smart playlists won't follow play counts: {}", describe(&e));
    }
}
