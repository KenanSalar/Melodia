//! `Search.*` callbacks, split by concern:
//!
//! * [`covers`] — the lazy cover-lookup callbacks (row, album-strip,
//!   artist-strip, Top Result).
//! * [`query`] — query debounce → FTS fetch commit + show-all toggle.
//! * [`recent`] — recent-searches pick / remove / clear.
//! * [`results`] — result-card cross-tab opens, Top Result routing,
//!   `TrackList` row actions, sort, column visibility, selection.
//! * [`lifecycle`] — section-active mirror + cache release.
//!
//! Cross-tab back-nav: clicking a Search result card sets
//! `*Detail.origin-nav-index = NAV_SEARCH` synchronously *before* the
//! async fetch yields, then `open_*_with` flips `Nav.selected-index` in
//! the same `upgrade_in_event_loop` closure that writes the detail-id.
//! The back arrow on the detail view reads the origin and flips Nav
//! back here.

mod covers;
mod lifecycle;
mod query;
mod recent;
mod results;

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::search::{SearchUi, fetch::push_recent_rows_to_slint};
use crate::{AppWindow, Search};

/// Nav-sidebar index of the Search tab. Used by the cross-tab
/// open-album / open-artist hand-offs to stamp
/// `*Detail.origin-nav-index` so the back arrow returns here, and by
/// the lifecycle module to seed the section-active shadow. Mirrors the
/// `NAV_*` consts in `callbacks/cross_tab_nav.rs`.
pub(super) const NAV_SEARCH: i32 = 0;

/// Wire every `Search.*` callback. Call once after
/// `search::install_search_models` and after both `AlbumsUi` + `ArtistsUi`
/// exist (the results module borrows them for the cross-tab open
/// hand-offs).
pub fn wire_search(
    ui: &AppWindow,
    state: &AppState,
    search_ui: &Arc<SearchUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    let weak = ui.as_weak();

    hydrate_sort_from_settings(state, search_ui, &ui.global::<Search>());
    hydrate_recent_on_install(state, search_ui, &weak);

    covers::wire(ui, search_ui);
    query::wire(ui, state, search_ui);
    recent::wire(ui, state, search_ui);
    results::wire(ui, state, search_ui, albums_ui, artists_ui);
    lifecycle::wire(ui, state, search_ui);
}

/// Read the persisted sort from settings and seed both the Rust cache
/// and the Slint properties. `None` (never persisted) leaves the
/// defaults in place.
fn hydrate_sort_from_settings(state: &AppState, search_ui: &SearchUi, g: &Search<'_>) {
    let Some(sort) = library::settings::get_view_sort(state, "search") else {
        return;
    };
    g.set_sort_field(SharedString::from(sort.field.as_str()));
    g.set_sort_dir(SharedString::from(match sort.dir {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    }));
    *search_ui.state().sort.lock() = ViewSort {
        field: sort.field,
        dir: match sort.dir {
            SortDir::Asc => SortDir::Asc,
            SortDir::Desc => SortDir::Desc,
        },
    };
}

/// Synchronously load the persisted recent-searches list and push it
/// into `Search.recent-rows`. `library::search::get_search_history` is
/// a cheap `parking_lot::Mutex::lock + clone` so this is safe to do
/// on the UI thread at install time (no DB hit, no `.await`).
fn hydrate_recent_on_install(
    state: &AppState,
    search_ui: &SearchUi,
    weak: &slint::Weak<AppWindow>,
) {
    match library::search::get_search_history(state) {
        Ok(rows) => {
            (*search_ui.state().recent.lock()).clone_from(&rows);
            // Push synchronously via invoke_from_event_loop — we may
            // already be on the UI thread, but the wrapper handles
            // that case cleanly.
            push_recent_rows_to_slint(weak, rows);
        }
        Err(e) => log::warn!("search::hydrate_recent: {e}"),
    }
}
