//! Every `Search.*` callback, split by concern.

mod covers;
mod lifecycle;
mod query;
mod recent;
mod results;

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::persisted_sort;
use crate::ui::search::{SearchUi, fetch::push_recent_rows_to_slint};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Search};

/// This page's `Nav.selected-index`, and the single definition of it.
pub(super) const NAV_SEARCH: i32 = 0;

/// Runs after `wire_all`, from [`super::install`], which installs the models first.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    search_ui: &Arc<SearchUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    let weak = ui.as_weak();

    hydrate_sort_from_settings(view_state, search_ui, &ui.global::<Search>());
    hydrate_recent_on_install(state, search_ui, &weak);

    covers::wire(ui, search_ui);
    query::wire(ui, state, search_ui);
    recent::wire(ui, state, search_ui);
    results::wire(ui, state, search_ui, albums_ui, artists_ui);
    lifecycle::wire(ui, state, search_ui);
}

/// Seeds the persisted sort into the Rust cache and the Slint properties; one never persisted
/// keeps its default.
fn hydrate_sort_from_settings(
    view_state: Option<&ViewStateData>,
    search_ui: &SearchUi,
    g: &Search<'_>,
) {
    let Some(sort) = persisted_sort(view_state, view_id::SEARCH) else {
        return;
    };
    g.set_sort_field(SharedString::from(sort.field.as_str()));
    g.set_sort_dir(SharedString::from(sort.dir.as_str()));
    *search_ui.state().sort.lock() = sort.clone();
}

/// Loads the recent searches on the UI thread, `get_search_history` being a lock and a clone.
fn hydrate_recent_on_install(
    state: &AppState,
    search_ui: &SearchUi,
    weak: &slint::Weak<AppWindow>,
) {
    match library::search::get_search_history(state) {
        Ok(rows) => {
            (*search_ui.state().recent.lock()).clone_from(&rows);
            push_recent_rows_to_slint(weak, rows);
        }
        Err(e) => log::warn!("search::hydrate_recent: {}", describe(&e)),
    }
}
