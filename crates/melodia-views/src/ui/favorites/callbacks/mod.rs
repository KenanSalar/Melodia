//! Every `Favorites.*` callback, split by concern.

mod covers;
mod hero;
mod lifecycle;
mod subviews;
mod tracklist;

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::persisted_sort;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::view_id;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Favorites};

/// Runs after `wire_all`, from [`super::install`], which installs the models first.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    hydrate_sort_from_settings(view_state, fav_ui, &ui.global::<Favorites>());

    covers::wire(ui);
    hero::wire(ui, state);
    subviews::wire(ui, state, fav_ui, artists_ui);
    tracklist::wire(ui, state, fav_ui);
    lifecycle::wire(ui, state, fav_ui);
}

/// Seeds the Songs list's and the Favorite Artists grid's persisted sorts, each under its own
/// `view_sort` key; one never persisted keeps its default.
fn hydrate_sort_from_settings(
    view_state: Option<&ViewStateData>,
    fav_ui: &FavoritesUi,
    g: &Favorites<'_>,
) {
    if let Some(sort) = persisted_sort(view_state, view_id::FAVORITES) {
        g.set_sort_field(SharedString::from(sort.field.as_str()));
        g.set_sort_dir(SharedString::from(sort.dir.as_str()));
        favorites_ui_mod::set_sort(fav_ui, sort.field.clone(), sort.dir);
    }

    // The global is the grid's whole sort state, its loads reading it back.
    if let Some(sort) = persisted_sort(view_state, view_id::FAVORITE_ARTISTS) {
        g.set_artist_sort_field(SharedString::from(sort.field.as_str()));
        g.set_artist_sort_dir(SharedString::from(sort.dir.as_str()));
    }
}
