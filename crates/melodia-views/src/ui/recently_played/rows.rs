//! The Slint models the view writes into.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::grid::RecentMostPlayed;
use crate::ui::card_grid;
use crate::ui::paged_rows::ScrollPoint;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Binds the page's models once, handing back where the grid is scrolled.
pub(super) fn install_recently_played_models(ui: &AppWindow, state: &AppState) -> ScrollPoint {
    let g = ui.global::<RecentlyPlayed>();

    let most_played = card_grid::install::<RecentMostPlayed>(state, ui);
    let scroll = most_played.scroll_point();
    g.set_most_played_rows(ModelRc::from(most_played));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    scroll
}
